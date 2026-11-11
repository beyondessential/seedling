---
id: TRN
---

# Transport

## Transport

<!-- from interface: Transport -->

> i[transport.quic]
> The OI uses QUIC as its wire transport protocol.

> i[transport.alpn]
> The TLS handshake negotiates an Application-Layer Protocol Negotiation (ALPN) identifier.
> Both client and server must offer the same identifier; a connection where the peer does not offer the expected identifier is rejected at the TLS layer.
> The current identifier is `bes.seedling/1`. Future incompatible protocol revisions are introduced by adding a new identifier; the negotiation result selects the variant.

> i[transport.server-identity]
> The server authenticates using an RFC 7250 raw public key (SPKI).
> The server's key pair is generated at first startup and persisted to the data directory so that clients can pin the SPKI fingerprint across restarts.
> Clients verify the server by its SPKI fingerprint; certificate chain validation is not used.

> i[transport.server-identity.published]
> On startup the server writes its SPKI fingerprint, hex-encoded, to `$data_dir/oi.fingerprint`.
> This allows a co-located process to pin the server identity without performing a fingerprint probe or reading logs.

> i[transport.listen]
> The server may be configured to listen on one or more addresses at startup.
> All configured addresses share the same server identity (key pair and SPKI fingerprint) and the same authorized key set.
> When no addresses are explicitly configured, the server listens on a single loopback address on the default port.
> A configured address that cannot be resolved at startup is a startup failure.
> The server must not start serving on the subset that did resolve, and must not start with no listeners at all: a management plane that is silently absent is indistinguishable from one that is present but unreachable.

> i[transport.fingerprint-probe]
> When a client connects to a server whose fingerprint is not yet in its known-hosts store, it must
> first capture the server's SPKI fingerprint without revealing its real identity to the server.
> The probe connection must present a raw public key as its mTLS client certificate, but the key
> used must be a freshly-generated, single-use key that is discarded immediately after the probe.
> The server will reject the probe connection (the ephemeral key is not authorised), but the
> server's SPKI fingerprint is captured during the TLS handshake before that rejection occurs.
> After capturing the fingerprint the client must confirm it with the user before proceeding with
> an authenticated connection using the real client identity.
> The probe connection must be structurally indistinguishable from a normal authenticated
> connection: a network observer or the server itself cannot determine whether a given connection
> is a probe or a real session.

> i[transport.client-auth]
> Every client connection must present a raw public key (RFC 7250 SPKI) as its mTLS certificate.
> The server maintains a set of authorized client SPKI fingerprints in persistent storage.
> A connection whose client certificate fingerprint is not in the authorized set is rejected at the TLS layer.
>
> On startup the server reads `$data_dir/authorized_keys` and imports any entries not already in
> persistent storage. Each non-comment line in that file has the form:
> ```
> <sha256-hex-fingerprint> <label>
> ```
> This file is the initial bootstrap mechanism: an operator with write access to the data directory
> can authorize a client key without requiring a prior authenticated connection.
>
> Authorization holds for the life of a connection, not only at its handshake. When a key leaves the
> authorized set, the server must close every connection that was authenticated with it, ending the
> event streams, shell sessions and port forwards carried on them. A revoked client cannot
> reconnect, so revoking a key cuts off the machine holding it rather than waiting for its session
> to end on its own.
