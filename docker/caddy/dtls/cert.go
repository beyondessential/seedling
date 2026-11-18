package seedlingdtls

import (
	"context"
	"crypto/tls"
	"fmt"

	"github.com/caddyserver/caddy/v2"
	"github.com/caddyserver/caddy/v2/modules/caddytls"
)

// clientHello is what a certificate choice may depend on, in the types the
// proxy's TLS machinery takes.
type clientHello struct {
	signatureSchemes  []tls.SignatureScheme
	cipherSuites      []uint16
	supportedCurves   []tls.CurveID
	supportedVersions []uint16
	supportedProtos   []string
}

// certificateFromTLSApp finds certificates the way an HTTPS ingress's
// handshake does: through a default connection policy of the proxy's TLS app,
// so certificates the runtime manages (fetched at handshake time) and ones the
// proxy holds both apply. The name looked up is the ingress's hostname, not
// the client's SNI.
func certificateFromTLSApp(ctx caddy.Context, _ *caddytls.TLS) certFunc {
	policies := caddytls.ConnectionPolicies{new(caddytls.ConnectionPolicy)}
	var provisionErr error
	if provisionErr = policies.Provision(ctx); provisionErr == nil {
		config := policies.TLSConfig(ctx)
		return func(_ context.Context, hostname string, hello clientHello) (*tls.Certificate, error) {
			info := &tls.ClientHelloInfo{
				ServerName:        hostname,
				SignatureSchemes:  hello.signatureSchemes,
				CipherSuites:      hello.cipherSuites,
				SupportedCurves:   hello.supportedCurves,
				SupportedVersions: hello.supportedVersions,
				SupportedProtos:   hello.supportedProtos,
			}
			inner, err := config.GetConfigForClient(info)
			if err != nil {
				return nil, fmt.Errorf("selecting TLS configuration for %q: %w", hostname, err)
			}
			if inner == nil {
				inner = config
			}
			return inner.GetCertificate(info)
		}
	}
	return func(context.Context, string, clientHello) (*tls.Certificate, error) {
		return nil, fmt.Errorf("provisioning the connection policy: %w", provisionErr)
	}
}
