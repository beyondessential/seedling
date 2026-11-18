// Package seedlingdtls is a Caddy app that terminates DTLS for Seedling's
// ingresses and forwards the plaintext datagrams inside each session to a
// service over a UDP flow of the session's own.
//
// It owns its UDP sockets instead of being a layer4 handler: caddy-l4 splits a
// UDP listener into one flow per remote address before any handler runs, so a
// handler inside it could neither follow a connection-ID session to a new
// address nor keep a session for the idle period the requirements give.
package seedlingdtls

import (
	"context"
	"crypto/tls"
	"fmt"
	"net"
	"time"

	"github.com/caddyserver/caddy/v2"
	"github.com/caddyserver/caddy/v2/modules/caddytls"
	"go.uber.org/zap"
)

func init() {
	caddy.RegisterModule(App{})
}

// idleTimeout is how long a session may carry no datagram in either direction
// before the proxy ends it.
const idleTimeout = 5 * time.Minute

// pool holds one listener per UDP port across configuration reloads, so that a
// reload which leaves a server as it was adopts its sessions instead of
// ending them.
var pool = caddy.NewUsagePool()

// App is the `seedling_dtls` Caddy app.
type App struct {
	// Servers are the DTLS-terminating ingresses, by name.
	Servers map[string]*Server `json:"servers,omitempty"`

	ctx     caddy.Context
	logger  *zap.Logger
	getCert certFunc
	ports   []uint16
}

// Server is one DTLS-terminating ingress.
type Server struct {
	// Listen is the UDP address to accept sessions on, as `:port`.
	Listen []string `json:"listen,omitempty"`
	// Hostname is the name whose certificate every session is given,
	// whatever name the client sends.
	Hostname string `json:"hostname,omitempty"`
	// Upstream is where the plaintext datagrams go, as `host:port`.
	Upstream string `json:"upstream,omitempty"`
	// Ingress names the ingress, which a refused session is reported against.
	Ingress string `json:"ingress,omitempty"`

	port uint16
}

// CaddyModule returns the Caddy module information.
func (App) CaddyModule() caddy.ModuleInfo {
	return caddy.ModuleInfo{
		ID:  "seedling_dtls",
		New: func() caddy.Module { return new(App) },
	}
}

// Provision checks the configuration and finds the certificates. It binds
// nothing, since validating a configuration provisions it too.
func (a *App) Provision(ctx caddy.Context) error {
	a.ctx = ctx
	a.logger = ctx.Logger(a)

	for name, srv := range a.Servers {
		if len(srv.Listen) != 1 {
			return fmt.Errorf("server %q: exactly one listen address is required", name)
		}
		addr, err := caddy.ParseNetworkAddressWithDefaults(srv.Listen[0], "udp", 0)
		if err != nil {
			return fmt.Errorf("server %q: %w", name, err)
		}
		if addr.Network != "udp" || addr.StartPort == 0 || addr.StartPort != addr.EndPort {
			return fmt.Errorf("server %q: listen address must be one UDP port", name)
		}
		srv.port = uint16(addr.StartPort)
		if srv.Hostname == "" || srv.Ingress == "" {
			return fmt.Errorf("server %q: hostname and ingress are required", name)
		}
		if _, err := net.ResolveUDPAddr("udp", srv.Upstream); err != nil {
			return fmt.Errorf("server %q: upstream: %w", name, err)
		}
	}

	if len(a.Servers) > 0 {
		tlsApp, err := ctx.AppIfConfigured("tls")
		if err != nil {
			return fmt.Errorf("the tls app, which holds the certificates, is not configured: %w", err)
		}
		a.getCert = certificateFromTLSApp(ctx, tlsApp.(*caddytls.TLS))
	}
	return nil
}

// Start binds the servers' ports, or adopts the listeners of a configuration
// being replaced.
func (a *App) Start() error {
	for _, srv := range a.Servers {
		cfg := serverConfig{Hostname: srv.Hostname, Upstream: srv.Upstream, Ingress: srv.Ingress}
		port := srv.port
		value, loaded, err := pool.LoadOrNew(port, func() (caddy.Destructor, error) {
			return newListener(a.ctx, a.logger, port, cfg, a.getCert, idleTimeout)
		})
		if err != nil {
			return fmt.Errorf("listening on udp/%d: %w", port, err)
		}
		if loaded {
			value.(*listener).reconfigure(cfg, a.getCert)
		}
		a.ports = append(a.ports, port)
	}
	return nil
}

// Stop releases the servers' listeners. One that a replacing configuration
// still uses stays open, with its sessions.
func (a *App) Stop() error {
	var firstErr error
	for _, port := range a.ports {
		if _, err := pool.Delete(port); err != nil && firstErr == nil {
			firstErr = err
		}
	}
	a.ports = nil
	return firstErr
}

// certFunc finds the certificate to present for a session whose client sent
// `hello`, naming `hostname`.
type certFunc func(ctx context.Context, hostname string, hello clientHello) (*tls.Certificate, error)

var (
	_ caddy.App         = (*App)(nil)
	_ caddy.Provisioner = (*App)(nil)
)
