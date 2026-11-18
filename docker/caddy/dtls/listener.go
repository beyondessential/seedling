package seedlingdtls

import (
	"context"
	"crypto/rand"
	"crypto/tls"
	"errors"
	"net"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/caddyserver/caddy/v2"
	"github.com/pion/dtls/v4"
	"github.com/pion/dtls/v4/pkg/protocol"
	"go.uber.org/zap"
)

const (
	handshakeTimeout = 30 * time.Second
	cidLength        = 8
	// maxRecord is the most plaintext one DTLS record carries, which bounds
	// a datagram in either direction of a session.
	maxRecord = 1 << 14
)

// sessionSlots bounds the sessions open across every listener. Each holds a
// socket toward its service, so without a bound a flood of sessions would
// spend the descriptors every other route in the proxy needs. Half the
// process's limit leaves the rest to them.
var sessionSlots = newSlots(descriptorLimit() / 2)

// descriptorLimit is the process's soft limit on open files, or a
// conservative stand-in where it cannot be read.
func descriptorLimit() int {
	var limit syscall.Rlimit
	if err := syscall.Getrlimit(syscall.RLIMIT_NOFILE, &limit); err != nil || limit.Cur == 0 {
		return 1024
	}
	if limit.Cur > 1<<20 {
		return 1 << 20
	}
	return int(limit.Cur)
}

// slots counts open sessions against a bound.
type slots struct {
	mu   sync.Mutex
	used int
	max  int
}

func newSlots(max int) *slots { return &slots{max: max} }

func (s *slots) take() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.used >= s.max {
		return false
	}
	s.used++
	return true
}

func (s *slots) release() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.used--
}

var errNoSlot = errors.New("the proxy holds as many DTLS sessions as it has room for")

// serverConfig is what a listener forwards its sessions with. A change of any
// of it ends the sessions made under the old one.
type serverConfig struct {
	Hostname string
	Upstream string
	Ingress  string
}

// listener is the DTLS listener of one UDP port, with the sessions it holds.
type listener struct {
	port   uint16
	logger *zap.Logger
	idle   time.Duration

	conn net.PacketConn
	ln   net.Listener

	mu      sync.Mutex
	cfg     serverConfig
	getCert certFunc
	// dial opens a flow to the upstream; a field so tests can fail it.
	dial     func(string) (*net.UDPConn, error)
	slots    *slots
	sessions map[*session]struct{}
	closed   bool
}

func newListener(
	ctx caddy.Context,
	logger *zap.Logger,
	port uint16,
	cfg serverConfig,
	getCert certFunc,
	idle time.Duration,
) (*listener, error) {
	addr := caddy.NetworkAddress{Network: "udp", StartPort: uint(port), EndPort: uint(port)}
	any, err := addr.Listen(ctx, 0, net.ListenConfig{})
	if err != nil {
		return nil, err
	}
	conn, ok := any.(net.PacketConn)
	if !ok {
		return nil, errors.New("not a packet connection")
	}
	return listenOn(conn, logger, port, cfg, getCert, idle)
}

// listenOn serves DTLS on an open packet connection.
func listenOn(
	conn net.PacketConn,
	logger *zap.Logger,
	port uint16,
	cfg serverConfig,
	getCert certFunc,
	idle time.Duration,
) (*listener, error) {
	l := &listener{
		port:     port,
		logger:   logger,
		idle:     idle,
		conn:     conn,
		cfg:      cfg,
		getCert:  getCert,
		dial:     dialUpstream,
		slots:    sessionSlots,
		sessions: make(map[*session]struct{}),
	}
	ln, err := dtls.Listen(
		conn,
		// Both versions are named, so a change of the library's defaults
		// cannot narrow what an ingress accepts.
		dtls.WithMinVersion(protocol.Version1_2),
		dtls.WithMaxVersion(protocol.Version1_3),
		// A session survives its client's address changing, once the new
		// address has shown it receives datagrams there.
		dtls.WithConnectionID(newConnectionID, dtls.CIDPathMigrationRRC),
		dtls.WithGetCertificate(l.certificate),
	)
	if err != nil {
		_ = conn.Close()
		return nil, err
	}
	l.ln = ln
	go l.acceptLoop()
	return l, nil
}

func newConnectionID() []byte {
	id := make([]byte, cidLength)
	if _, err := rand.Read(id); err != nil {
		panic("reading random bytes: " + err.Error())
	}
	return id
}

// certificate presents the certificate for the ingress's own hostname, whatever
// name the client sent, and whether or not it sent one.
func (l *listener) certificate(hello *dtls.ClientHelloInfo) (*tls.Certificate, error) {
	l.mu.Lock()
	hostname, getCert := l.cfg.Hostname, l.getCert
	l.mu.Unlock()

	ctx := hello.Context()
	if ctx == nil {
		ctx = context.Background()
	}
	return getCert(ctx, hostname)
}

// reconfigure adopts a configuration for a listener that is already serving.
// The sessions are left as they are when the server is unchanged, and ended
// when it is not, since their flows go to the old upstream.
func (l *listener) reconfigure(cfg serverConfig, getCert certFunc) {
	l.mu.Lock()
	changed := l.cfg != cfg
	l.cfg = cfg
	l.getCert = getCert
	var ended []*session
	if changed {
		for s := range l.sessions {
			ended = append(ended, s)
		}
	}
	l.mu.Unlock()
	for _, s := range ended {
		s.close()
	}
}

func (l *listener) acceptLoop() {
	for {
		conn, err := l.ln.Accept()
		if err != nil {
			l.mu.Lock()
			closed := l.closed
			l.mu.Unlock()
			if closed || errors.Is(err, net.ErrClosed) {
				return
			}
			l.logger.Warn("accepting a DTLS session failed", zap.Uint16("port", l.port), zap.Error(err))
			continue
		}
		go l.serve(conn.(*dtls.Conn))
	}
}

// serve carries one session: it finishes the handshake, opens the session's
// flow to the upstream, and copies datagrams both ways until the session ends.
func (l *listener) serve(conn *dtls.Conn) {
	ctx, cancel := context.WithTimeout(context.Background(), handshakeTimeout)
	err := conn.HandshakeContext(ctx)
	cancel()
	if err != nil {
		// Nothing is sent back about a datagram that was not a valid
		// handshake.
		_ = conn.Close()
		return
	}

	l.mu.Lock()
	cfg := l.cfg
	dial := l.dial
	room := l.slots
	closed := l.closed
	l.mu.Unlock()
	if closed {
		_ = conn.Close()
		return
	}

	// The flow is opened only now: a client that never completes the
	// handshake spends none of the proxy's source ports.
	if !room.take() {
		l.refuse(cfg, errNoSlot)
		_ = conn.Close()
		return
	}
	defer room.release()
	upstream, err := dial(cfg.Upstream)
	if err != nil {
		l.refuse(cfg, err)
		_ = conn.Close()
		return
	}

	s := &session{listener: l, client: conn, upstream: upstream}
	s.touch()
	l.mu.Lock()
	if l.closed || l.cfg != cfg {
		l.mu.Unlock()
		_ = conn.Close()
		_ = upstream.Close()
		return
	}
	l.sessions[s] = struct{}{}
	l.mu.Unlock()

	s.run(l.idle)

	l.mu.Lock()
	delete(l.sessions, s)
	l.mu.Unlock()
}

func dialUpstream(addr string) (*net.UDPConn, error) {
	raddr, err := net.ResolveUDPAddr("udp", addr)
	if err != nil {
		return nil, err
	}
	return net.DialUDP("udp", nil, raddr)
}

// refuse records that a session was refused for want of a flow to its
// service, which the runtime reads to hold a fault against the ingress.
func (l *listener) refuse(cfg serverConfig, cause error) {
	l.logger.Warn("refused a DTLS session: no flow to the service could be opened",
		zap.String("ingress", cfg.Ingress),
		zap.String("upstream", cfg.Upstream),
		zap.Error(cause),
	)
	refusals.record(cfg.Ingress, time.Now())
}

// Destruct closes the listener and every session, which is what the usage
// pool calls once no configuration uses the port.
func (l *listener) Destruct() error {
	l.mu.Lock()
	l.closed = true
	var ended []*session
	for s := range l.sessions {
		ended = append(ended, s)
	}
	l.mu.Unlock()
	err := l.ln.Close()
	_ = l.conn.Close()
	for _, s := range ended {
		s.close()
	}
	return err
}

// session is one DTLS session and the flow to the upstream that carries it.
type session struct {
	listener *listener
	client   *dtls.Conn
	upstream *net.UDPConn

	last      atomic.Int64
	closeOnce sync.Once
	done      chan struct{}
	doneOnce  sync.Once
}

func (s *session) touch() { s.last.Store(time.Now().UnixNano()) }

func (s *session) close() {
	s.closeOnce.Do(func() {
		_ = s.client.Close()
		_ = s.upstream.Close()
	})
}

// run copies datagrams between the client and the upstream until either ends,
// the client closes the session, or it has been idle for `idle`.
func (s *session) run(idle time.Duration) {
	s.done = make(chan struct{})
	finish := func() {
		s.doneOnce.Do(func() { close(s.done) })
		s.close()
	}

	go func() {
		defer finish()
		buf := make([]byte, maxRecord)
		for {
			n, err := s.client.Read(buf)
			if err != nil {
				return
			}
			s.touch()
			if _, err := s.upstream.Write(buf[:n]); err != nil && !isRefused(err) {
				return
			}
		}
	}()
	go func() {
		defer finish()
		// One byte over a record, so a datagram too big for one is seen
		// whole rather than cut short.
		buf := make([]byte, maxRecord+1)
		for {
			n, err := s.upstream.Read(buf)
			if err != nil {
				if isRefused(err) {
					continue
				}
				return
			}
			s.touch()
			// A datagram that does not fit in a record cannot be carried
			// inside the session. It is dropped, as the network would drop
			// one too large for the path, and the session goes on.
			if n > maxRecord {
				continue
			}
			if _, err := s.client.Write(buf[:n]); err != nil {
				return
			}
		}
	}()

	tick := idle / 4
	if tick > 10*time.Second {
		tick = 10 * time.Second
	}
	if tick < time.Millisecond {
		tick = time.Millisecond
	}
	ticker := time.NewTicker(tick)
	defer ticker.Stop()
	for {
		select {
		case <-s.done:
			return
		case <-ticker.C:
			if time.Since(time.Unix(0, s.last.Load())) >= idle {
				finish()
				return
			}
		}
	}
}

// isRefused is an ICMP port-unreachable reported on a connected UDP socket:
// the service is not listening yet, which the session outlasts.
func isRefused(err error) bool {
	return errors.Is(err, syscall.ECONNREFUSED)
}
