package seedlingdtls

import (
	"context"
	"crypto/tls"
	"errors"
	"fmt"
	"net"
	"sync"
	"time"

	"github.com/caddyserver/caddy/v2"
	"github.com/caddyserver/caddy/v2/modules/caddytls"
)

// certFunc finds the certificate to present for a session of the ingress whose
// hostname is `hostname`.
type certFunc func(ctx context.Context, hostname string) (*tls.Certificate, error)

var errCaptured = errors.New("certificate captured")

// certificateFromTLSApp finds certificates the way an HTTPS ingress's
// handshake does: through a default connection policy of the proxy's TLS app,
// so certificates the runtime manages (fetched at handshake time) and ones the
// proxy holds both apply. The name looked up is the ingress's hostname, not
// the client's SNI.
func certificateFromTLSApp(ctx caddy.Context) (certFunc, error) {
	policies := caddytls.ConnectionPolicies{new(caddytls.ConnectionPolicy)}
	if err := policies.Provision(ctx); err != nil {
		return nil, fmt.Errorf("provisioning the connection policy: %w", err)
	}
	config := policies.TLSConfig(ctx)
	return func(ctx context.Context, hostname string) (*tls.Certificate, error) {
		return captureCertificate(ctx, config, hostname)
	}, nil
}

// captureCertificate asks `config` for the certificate it would present to a
// client naming `hostname`.
//
// The proxy's certificate lookup takes a client hello that only the TLS
// library can construct, carrying a context and a connection the lookup reads.
// So the hello comes from a handshake run in memory: the server side takes the
// certificate in its first callback and aborts, and the client side only
// supplies the name.
func captureCertificate(ctx context.Context, config *tls.Config, hostname string) (*tls.Certificate, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	clientEnd, serverEnd := net.Pipe()
	defer clientEnd.Close()
	defer serverEnd.Close()

	var (
		cert    *tls.Certificate
		certErr error
	)
	server := tls.Server(serverEnd, &tls.Config{
		GetConfigForClient: func(hello *tls.ClientHelloInfo) (*tls.Config, error) {
			inner, err := config.GetConfigForClient(hello)
			if err != nil {
				certErr = err
				return nil, errCaptured
			}
			if inner == nil {
				inner = config
			}
			cert, certErr = inner.GetCertificate(hello)
			return nil, errCaptured
		},
	})
	client := tls.Client(clientEnd, &tls.Config{ServerName: hostname, InsecureSkipVerify: true})

	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		_ = server.HandshakeContext(ctx)
		_ = serverEnd.Close()
	}()
	go func() {
		defer wg.Done()
		_ = client.HandshakeContext(ctx)
		_ = clientEnd.Close()
	}()
	wg.Wait()

	if certErr != nil {
		return nil, fmt.Errorf("finding the certificate for %q: %w", hostname, certErr)
	}
	if cert == nil {
		return nil, fmt.Errorf("finding the certificate for %q: no certificate was selected", hostname)
	}
	return cert, nil
}
