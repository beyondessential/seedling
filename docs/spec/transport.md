The shared seedling transport is a QUIC-based duplex channel that carries one or more application-layer protocols, each identified by a distinct ALPN value. Different protocols (operator interface, grove, …) share the same listener, server identity, and TLS handshake; they differ in the ALPN they negotiate and the per-stream framing they use after the handshake completes.

Each application protocol defines its own spec. The items below describe what is common to all of them.

# Handshake

> t[quic]
> The shared transport uses QUIC as its wire protocol.

> t[alpn]
> The TLS handshake negotiates an Application-Layer Protocol Negotiation (ALPN) identifier.
> Both client and server must offer the same identifier; a connection where the peer does not offer an expected identifier is rejected at the TLS layer.
> Each application protocol on the shared transport defines its own ALPN identifier. Future incompatible revisions of a protocol are introduced by adding a new identifier; the negotiation result selects the variant.

> t[server-identity]
> The server authenticates using an RFC 7250 raw public key (SPKI).
> The server's key pair is generated at first startup and persisted to the data directory so that clients can pin the SPKI fingerprint across restarts. The same key pair is used for every ALPN registered on the transport.
> Clients verify the server by its SPKI fingerprint; certificate chain validation is not used.

> t[server-identity.published]
> On startup the OI writes its SPKI fingerprint, hex-encoded, to `$data_dir/oi.fingerprint`.
> This allows a co-located process to pin the server identity without performing a fingerprint probe or reading logs.

> t[client-auth]
> Every client connection must present a raw public key (RFC 7250 SPKI) as its mTLS certificate.
> The server maintains a per-protocol authorised-keys set: each ALPN registers its own set of authorised client SPKI fingerprints. A client fingerprint may appear in zero, one, or more such sets — protocols are independent trust grants.
> Per-protocol authorisation is enforced after the TLS handshake completes, against the negotiated ALPN: a connection whose client certificate fingerprint is not in the trust set for the negotiated ALPN is rejected immediately, before any application data is exchanged.
> At TLS handshake time the verifier accepts any fingerprint trusted by any registered protocol. (TLS client-cert verification runs before the negotiated ALPN is observable to the verifier; the per-ALPN gate is therefore enforced post-handshake.)

> t[fingerprint-probe]
> When a client connects to a server whose fingerprint is not yet in its known-hosts store, it must first capture the server's SPKI fingerprint without revealing its real identity to the server.
> The probe connection must present a raw public key as its mTLS client certificate, but the key used must be a freshly-generated, single-use key that is discarded immediately after the probe.
> The server will reject the probe connection (the ephemeral key is not authorised for any registered protocol), but the server's SPKI fingerprint is captured during the TLS handshake before that rejection occurs.
> After capturing the fingerprint the client must confirm it with the user before proceeding with an authenticated connection using the real client identity.
> The probe connection must be structurally indistinguishable from a normal authenticated connection: a network observer or the server itself cannot determine whether a given connection is a probe or a real session.

> t[listen]
> The server may be configured to listen on one or more addresses at startup.
> All configured addresses share the same server identity (key pair and SPKI fingerprint), the same set of registered ALPNs, and the same per-ALPN authorised-keys sets.
> When no addresses are explicitly configured, the server listens on a single loopback address on the default port.
