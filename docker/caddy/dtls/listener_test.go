package seedlingdtls

import (
	"context"
	"crypto/tls"
	"errors"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/pion/dtls/v4"
	"github.com/pion/dtls/v4/pkg/crypto/selfsign"
	"github.com/pion/dtls/v4/pkg/protocol"
	"go.uber.org/zap/zaptest"
)

// echoUpstream is a UDP service that echoes each datagram back, and records
// the source address of every datagram it receives.
type echoUpstream struct {
	conn *net.UDPConn
	mu   sync.Mutex
	from []*net.UDPAddr
}

func newEchoUpstream(t *testing.T) *echoUpstream {
	t.Helper()
	conn, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	u := &echoUpstream{conn: conn}
	t.Cleanup(func() { _ = conn.Close() })
	go func() {
		buf := make([]byte, 2048)
		for {
			n, from, err := conn.ReadFromUDP(buf)
			if err != nil {
				return
			}
			u.mu.Lock()
			u.from = append(u.from, from)
			u.mu.Unlock()
			_, _ = conn.WriteToUDP(buf[:n], from)
		}
	}()
	return u
}

func (u *echoUpstream) addr() string { return u.conn.LocalAddr().String() }

func (u *echoUpstream) sources() []*net.UDPAddr {
	u.mu.Lock()
	defer u.mu.Unlock()
	return append([]*net.UDPAddr(nil), u.from...)
}

// served records the server names a certificate was asked for.
type served struct {
	mu    sync.Mutex
	names []string
}

func (s *served) get() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]string(nil), s.names...)
}

func testCert(t *testing.T, s *served) certFunc {
	t.Helper()
	cert, err := selfsign.GenerateSelfSigned()
	if err != nil {
		t.Fatal(err)
	}
	return func(_ context.Context, hostname string) (*tls.Certificate, error) {
		s.mu.Lock()
		s.names = append(s.names, hostname)
		s.mu.Unlock()
		return &cert, nil
	}
}

type fixture struct {
	l      *listener
	addr   *net.UDPAddr
	up     *echoUpstream
	served *served
}

func newFixture(t *testing.T, idle time.Duration) *fixture {
	t.Helper()
	up := newEchoUpstream(t)
	pc, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	s := &served{}
	cfg := serverConfig{Hostname: "voip.example.com", Upstream: up.addr(), Ingress: "voip.example.com:5684"}
	l, err := listenOn(pc, zaptest.NewLogger(t), 5684, cfg, testCert(t, s), idle)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = l.Destruct() })
	return &fixture{l: l, addr: pc.LocalAddr().(*net.UDPAddr), up: up, served: s}
}

func (f *fixture) client(t *testing.T, opts ...dtls.ClientOption) *dtls.Conn {
	t.Helper()
	opts = append([]dtls.ClientOption{dtls.WithInsecureSkipVerify(true)}, opts...)
	conn, err := dtls.Dial("udp", f.addr, opts...)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := conn.HandshakeContext(ctx); err != nil {
		t.Fatalf("handshake: %v", err)
	}
	return conn
}

func roundTrip(t *testing.T, c *dtls.Conn, msg string) {
	t.Helper()
	if _, err := c.Write([]byte(msg)); err != nil {
		t.Fatal(err)
	}
	_ = c.SetReadDeadline(time.Now().Add(5 * time.Second))
	buf := make([]byte, 2048)
	n, err := c.Read(buf)
	if err != nil {
		t.Fatalf("reading the echo of %q: %v", msg, err)
	}
	if string(buf[:n]) != msg {
		t.Fatalf("echo = %q, want %q", buf[:n], msg)
	}
}

// spec: PRX#dtls-terminating-ingresses
func TestBothDTLSVersionsAreAcceptedAndCarryDatagrams(t *testing.T) {
	for name, version := range map[string]protocol.Version{
		"1.2": protocol.Version1_2,
		"1.3": protocol.Version1_3,
	} {
		t.Run(name, func(t *testing.T) {
			f := newFixture(t, time.Minute)
			c := f.client(t, dtls.WithMinVersion(version), dtls.WithMaxVersion(version))
			roundTrip(t, c, "hello "+name)
		})
	}
}

// spec: ING#termination
func TestTheCertificateIsTheIngressesWhateverNameTheClientSends(t *testing.T) {
	f := newFixture(t, time.Minute)
	roundTrip(t, f.client(t), "no sni")
	roundTrip(t, f.client(t, dtls.WithServerName("something.else.example")), "another name")
	for _, name := range f.served.get() {
		if name != "voip.example.com" {
			t.Fatalf("certificate looked up for %q, want the ingress's own hostname", name)
		}
	}
	if len(f.served.get()) < 2 {
		t.Fatalf("expected a certificate lookup per handshake, got %v", f.served.get())
	}
}

// spec: ING#termination
func TestADatagramThatIsNotDTLSGetsNoReplyAndOpensNoFlow(t *testing.T) {
	f := newFixture(t, time.Minute)
	raw, err := net.DialUDP("udp", nil, f.addr)
	if err != nil {
		t.Fatal(err)
	}
	defer raw.Close()
	for _, payload := range [][]byte{[]byte("not dtls at all"), make([]byte, 100), {22, 0xfe, 0xfd}} {
		if _, err := raw.Write(payload); err != nil {
			t.Fatal(err)
		}
	}
	_ = raw.SetReadDeadline(time.Now().Add(500 * time.Millisecond))
	if n, err := raw.Read(make([]byte, 2048)); err == nil {
		t.Fatalf("got a %d byte reply to a datagram that was not DTLS", n)
	}
	if got := f.up.sources(); len(got) != 0 {
		t.Fatalf("a flow reached the upstream before any handshake: %v", got)
	}
}

// spec: PRX#dtls-terminating-ingresses
func TestAHandshakeBegunFromASpoofedSourceNeverGetsAFlow(t *testing.T) {
	f := newFixture(t, time.Minute)
	// A client that sends its hello but never receives the cookie, as one
	// whose source address was forged would not.
	raw, err := net.DialUDP("udp", nil, f.addr)
	if err != nil {
		t.Fatal(err)
	}
	defer raw.Close()
	client, err := dtls.Client(newSendOnly(raw), f.addr, dtls.WithInsecureSkipVerify(true))
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 1500*time.Millisecond)
	defer cancel()
	defer client.Close()
	if err := client.HandshakeContext(ctx); err == nil {
		t.Fatal("a handshake completed without the client receiving anything")
	}
	if got := f.up.sources(); len(got) != 0 {
		t.Fatalf("a flow reached the upstream: %v", got)
	}
}

// sendOnly is a packet connection that never delivers a datagram, and honours
// read deadlines as the library's handshake relies on to give up.
type sendOnly struct {
	c        *net.UDPConn
	stop     chan struct{}
	once     sync.Once
	mu       sync.Mutex
	deadline time.Time
}

func newSendOnly(c *net.UDPConn) *sendOnly { return &sendOnly{c: c, stop: make(chan struct{})} }

type timeoutError struct{}

func (timeoutError) Error() string   { return "i/o timeout" }
func (timeoutError) Timeout() bool   { return true }
func (timeoutError) Temporary() bool { return true }

func (s *sendOnly) ReadFrom([]byte) (int, net.Addr, error) {
	for {
		select {
		case <-s.stop:
			return 0, nil, net.ErrClosed
		case <-time.After(5 * time.Millisecond):
		}
		s.mu.Lock()
		d := s.deadline
		s.mu.Unlock()
		if !d.IsZero() && !time.Now().Before(d) {
			return 0, nil, timeoutError{}
		}
	}
}
func (s *sendOnly) WriteTo(p []byte, _ net.Addr) (int, error) { return s.c.Write(p) }
func (s *sendOnly) Close() error {
	s.once.Do(func() { close(s.stop) })
	return s.c.Close()
}
func (s *sendOnly) LocalAddr() net.Addr              { return s.c.LocalAddr() }
func (s *sendOnly) SetDeadline(t time.Time) error    { return s.SetReadDeadline(t) }
func (s *sendOnly) SetWriteDeadline(time.Time) error { return nil }
func (s *sendOnly) SetReadDeadline(t time.Time) error {
	s.mu.Lock()
	s.deadline = t
	s.mu.Unlock()
	return nil
}

// spec: PRX#dtls-terminating-ingresses
func TestEachSessionGetsAFlowOfItsOwnAndRepliesReturnToIt(t *testing.T) {
	f := newFixture(t, time.Minute)
	a, b := f.client(t), f.client(t)
	roundTrip(t, a, "from a")
	roundTrip(t, b, "from b")
	roundTrip(t, a, "from a again")

	ports := map[int]bool{}
	for _, src := range f.up.sources() {
		ports[src.Port] = true
	}
	if len(ports) != 2 {
		t.Fatalf("expected two distinct source ports toward the service, got %v", ports)
	}
}

func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func (l *listener) sessionCount() int {
	l.mu.Lock()
	defer l.mu.Unlock()
	return len(l.sessions)
}

// spec: PRX#dtls-terminating-ingresses
func TestASessionEndsWhenItsClientClosesIt(t *testing.T) {
	f := newFixture(t, time.Minute)
	c := f.client(t)
	roundTrip(t, c, "hi")
	waitFor(t, "the session", func() bool { return f.l.sessionCount() == 1 })
	if err := c.Close(); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "the session to end", func() bool { return f.l.sessionCount() == 0 })
}

// spec: PRX#dtls-terminating-ingresses
func TestAnIdleSessionEndsAndItsFlowIsClosed(t *testing.T) {
	f := newFixture(t, 300*time.Millisecond)
	c := f.client(t)
	roundTrip(t, c, "hi")
	waitFor(t, "the session", func() bool { return f.l.sessionCount() == 1 })

	var s *session
	f.l.mu.Lock()
	for s = range f.l.sessions {
	}
	f.l.mu.Unlock()

	waitFor(t, "the idle session to end", func() bool { return f.l.sessionCount() == 0 })
	if _, err := s.upstream.Write([]byte("x")); err == nil || !errors.Is(err, net.ErrClosed) {
		t.Fatalf("the session's flow is still open after it ended: %v", err)
	}
}

// spec: PRX#dtls-terminating-ingresses
func TestTrafficKeepsASessionFromGoingIdle(t *testing.T) {
	f := newFixture(t, 600*time.Millisecond)
	c := f.client(t)
	for i := 0; i < 6; i++ {
		roundTrip(t, c, "keepalive")
		time.Sleep(200 * time.Millisecond)
	}
	if f.l.sessionCount() != 1 {
		t.Fatal("a session that carried traffic was ended as idle")
	}
}

// spec: PRX#dtls-terminating-ingresses
func TestReconfiguringWithTheServerUnchangedKeepsTheSessionAndChangingItEndsIt(t *testing.T) {
	f := newFixture(t, time.Minute)
	c := f.client(t)
	roundTrip(t, c, "before")

	same := f.l.cfg
	f.l.reconfigure(same, f.l.getCert)
	roundTrip(t, c, "after an apply that left the ingress as it was")
	if f.l.sessionCount() != 1 {
		t.Fatal("the session did not survive")
	}

	other := newEchoUpstream(t)
	changed := same
	changed.Upstream = other.addr()
	f.l.reconfigure(changed, f.l.getCert)
	waitFor(t, "the session to end", func() bool { return f.l.sessionCount() == 0 })

	// A new session goes to the new upstream.
	c2 := f.client(t)
	roundTrip(t, c2, "to the new upstream")
	if len(other.sources()) == 0 {
		t.Fatal("the new session did not reach the new upstream")
	}
}

// spec: PRX#dtls-terminating-ingresses
func TestASessionRefusedForWantOfAFlowIsLoggedRecordedAndOthersContinue(t *testing.T) {
	f := newFixture(t, time.Minute)
	ok := f.client(t)
	roundTrip(t, ok, "first")

	setDial := func(d func(string) (*net.UDPConn, error)) {
		f.l.mu.Lock()
		f.l.dial = d
		f.l.mu.Unlock()
	}
	before := f.l.dial
	setDial(func(string) (*net.UDPConn, error) { return nil, errors.New("no source ports left") })

	c, err := dtls.Dial("udp", f.addr, dtls.WithInsecureSkipVerify(true))
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = c.HandshakeContext(ctx)

	waitFor(t, "the refusal to be recorded", func() bool {
		_, ok := refusals.snapshot()["voip.example.com:5684"]
		return ok
	})
	setDial(before)
	roundTrip(t, ok, "still served")
}

// switchingConn is the client's packet connection, whose datagrams can be
// moved to another local socket, as a client changing network would.
type switchingConn struct {
	mu   sync.Mutex
	cur  *net.UDPConn
	in   chan packet
	stop chan struct{}
}

type packet struct {
	b    []byte
	from net.Addr
}

func newSwitchingConn(t *testing.T) *switchingConn {
	s := &switchingConn{in: make(chan packet, 64), stop: make(chan struct{})}
	s.switchTo(t)
	return s
}

func (s *switchingConn) switchTo(t *testing.T) {
	c, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	s.mu.Lock()
	old := s.cur
	s.cur = c
	s.mu.Unlock()
	if old != nil {
		_ = old.Close()
	}
	go func() {
		buf := make([]byte, 65535)
		for {
			n, from, err := c.ReadFrom(buf)
			if err != nil {
				return
			}
			select {
			case s.in <- packet{append([]byte(nil), buf[:n]...), from}:
			case <-s.stop:
				return
			}
		}
	}()
}

func (s *switchingConn) ReadFrom(p []byte) (int, net.Addr, error) {
	select {
	case pk := <-s.in:
		return copy(p, pk.b), pk.from, nil
	case <-s.stop:
		return 0, nil, net.ErrClosed
	}
}

func (s *switchingConn) WriteTo(p []byte, a net.Addr) (int, error) {
	s.mu.Lock()
	c := s.cur
	s.mu.Unlock()
	return c.WriteTo(p, a)
}
func (s *switchingConn) Close() error {
	select {
	case <-s.stop:
	default:
		close(s.stop)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cur.Close()
}
func (s *switchingConn) LocalAddr() net.Addr {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cur.LocalAddr()
}
func (s *switchingConn) SetDeadline(time.Time) error      { return nil }
func (s *switchingConn) SetReadDeadline(time.Time) error  { return nil }
func (s *switchingConn) SetWriteDeadline(time.Time) error { return nil }

// spec: PRX#dtls-terminating-ingresses
func TestASessionWithAConnectionIDContinuesWhenTheClientsAddressChanges(t *testing.T) {
	for name, version := range map[string]protocol.Version{
		"1.2": protocol.Version1_2,
		"1.3": protocol.Version1_3,
	} {
		t.Run(name, func(t *testing.T) {
			f := newFixture(t, time.Minute)
			pc := newSwitchingConn(t)
			defer pc.Close()
			c, err := dtls.Client(pc, f.addr,
				dtls.WithInsecureSkipVerify(true),
				dtls.WithMinVersion(version),
				dtls.WithMaxVersion(version),
				dtls.WithConnectionID(newConnectionID, dtls.CIDPathMigrationRRC),
			)
			if err != nil {
				t.Fatal(err)
			}
			defer c.Close()
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			if err := c.HandshakeContext(ctx); err != nil {
				t.Fatal(err)
			}
			roundTrip(t, c, "before the move")
			flows := len(f.up.sources())

			pc.switchTo(t)
			// Datagrams from the new address may be held or dropped while
			// the proxy confirms the client receives there, so the client
			// keeps sending until a reply arrives.
			replied := make(chan error, 1)
			go func() {
				buf := make([]byte, 2048)
				_ = c.SetReadDeadline(time.Now().Add(8 * time.Second))
				_, err := c.Read(buf)
				replied <- err
			}()
			deadline := time.Now().Add(8 * time.Second)
		retry:
			for time.Now().Before(deadline) {
				_, _ = c.Write([]byte("after the move"))
				select {
				case err := <-replied:
					if err != nil {
						t.Fatalf("no reply after the move: %v", err)
					}
					break retry
				case <-time.After(200 * time.Millisecond):
				}
			}

			if f.l.sessionCount() != 1 {
				t.Fatalf("the move left %d sessions, want the one", f.l.sessionCount())
			}
			ports := map[int]bool{}
			for _, src := range f.up.sources() {
				ports[src.Port] = true
			}
			if len(ports) != 1 || len(f.up.sources()) <= flows {
				t.Fatalf("the service should see the same flow throughout, got ports %v", ports)
			}
		})
	}
}

// spec: PRX#dtls-terminating-ingresses
func TestSessionsBeyondTheProxysRoomAreRefusedAndTheOpenOnesContinue(t *testing.T) {
	f := newFixture(t, time.Minute)
	f.l.mu.Lock()
	f.l.slots = newSlots(1)
	f.l.mu.Unlock()

	first := f.client(t)
	roundTrip(t, first, "first")

	refused, err := dtls.Dial("udp", f.addr, dtls.WithInsecureSkipVerify(true))
	if err != nil {
		t.Fatal(err)
	}
	defer refused.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = refused.HandshakeContext(ctx)

	waitFor(t, "the refusal to be recorded", func() bool {
		_, ok := refusals.snapshot()["voip.example.com:5684"]
		return ok
	})
	roundTrip(t, first, "still served")
	if got := len(f.up.sources()); got != 2 {
		t.Fatalf("the refused session reached the service: %d datagrams", got)
	}

	// Ending the open session makes room for another.
	_ = first.Close()
	waitFor(t, "the session to end", func() bool { return f.l.sessionCount() == 0 })
	roundTrip(t, f.client(t), "after room was made")
}

// spec: PRX#dtls-terminating-ingresses
func TestADatagramTooBigForARecordIsDroppedAndTheSessionGoesOn(t *testing.T) {
	f := newFixture(t, time.Minute)
	// Answer each datagram with one too big for a record, then the echo.
	big, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer big.Close()
	go func() {
		buf := make([]byte, 2048)
		for {
			n, from, err := big.ReadFromUDP(buf)
			if err != nil {
				return
			}
			_, _ = big.WriteToUDP(make([]byte, maxRecord+100), from)
			_, _ = big.WriteToUDP(buf[:n], from)
		}
	}()
	cfg := f.l.cfg
	cfg.Upstream = big.LocalAddr().String()
	f.l.reconfigure(cfg, f.l.getCert)

	c := f.client(t)
	roundTrip(t, c, "first")
	roundTrip(t, c, "second")
	if f.l.sessionCount() != 1 {
		t.Fatal("the session ended")
	}
}
