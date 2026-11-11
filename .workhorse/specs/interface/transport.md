---
id: TRN
---

# Transport

Applies to the connections between a [client](overview.md#client) and a [server](overview.md#server): the transport, how each end proves its identity, and where the server listens.

## Borrowed terms

| term | meaning |
| --- | --- |
| connection | a QUIC connection, as defined in [RFC 9000](https://www.rfc-editor.org/rfc/rfc9000) |
| handshake | the TLS 1.3 handshake carried in QUIC, as defined in [RFC 9001](https://www.rfc-editor.org/rfc/rfc9001) and [RFC 9846](https://www.rfc-editor.org/rfc/rfc9846) |
| ALPN identifier | an application protocol name negotiated in the handshake, as defined in [RFC 7301](https://www.rfc-editor.org/rfc/rfc7301) |
| raw public key | a public key presented in the handshake in place of a certificate, as defined in [RFC 7250](https://www.rfc-editor.org/rfc/rfc7250) |
| SubjectPublicKeyInfo | the DER-encoded public key structure defined in [RFC 5280](https://www.rfc-editor.org/rfc/rfc5280) section 4.1 |

## Terminology

### Fingerprint

The SHA-256 digest ([FIPS 180-4](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)) of a key's DER-encoded SubjectPublicKeyInfo, written as 64 lowercase hexadecimal digits.

### Authorised key set

The fingerprints of the client keys a server accepts, each with a label.

## Connection

A client and a server MUST communicate over QUIC version 1, secured with TLS 1.3 as RFC 9001 specifies.

Both ends MUST offer the ALPN identifier `bes.seedling/1`, and MUST abort the handshake if the other end does not offer it.
A revision of this protocol that an implementation of the current revision cannot interoperate with MUST use a different ALPN identifier.

The server MUST support the QUIC datagram extension ([RFC 9221](https://www.rfc-editor.org/rfc/rfc9221)).
A client that forwards UDP traffic MUST support it too.

The server SHOULD keep an idle connection open for as long as its client remains reachable, by sending keep-alive packets often enough that the connection does not reach its idle timeout.

> [!NOTE]
> Event subscriptions and port forwards hold a connection open through long quiet periods.
> The reference server sends a keep-alive every 10 seconds and uses an idle timeout of 30 seconds.

## Server identity

The server MUST authenticate itself with a raw public key.
A client MUST verify the server by comparing the fingerprint of the key it presents with a fingerprint the client already trusts, and MUST abort the handshake if they differ.
A client MUST NOT rely on certificate chain validation to authenticate a server.

A client MUST support Ed25519 keys as [RFC 8410](https://www.rfc-editor.org/rfc/rfc8410) encodes them.

The server MUST generate its key pair the first time it starts, and MUST persist the private key in the file `oi.key` in its [data directory](../overview.md#data-directory).
On every later start the server MUST use the key in that file, so its fingerprint stays the same across restarts.
The key file is subject to the [private key file permissions](../runtime/infrastructure.md#file-permissions).

Each time it starts, the server MUST write its fingerprint, followed by a line feed, to the file `oi.fingerprint` in its data directory.
A failure to write that file MUST NOT stop the server serving.

> [!NOTE]
> The published fingerprint lets a process on the same host, such as the [web interface](../overview.md#web-interface), pin the server without a fingerprint probe.

## Listen addresses

The server MUST listen on every address it is configured with, and on no other.
The server MUST present the same server key and accept the same authorised key set on every address.

The daemon's `--listen` option names a socket address to listen on, and MAY be given more than once.
The `--interface` option names network interfaces, separated by commas.
Each named interface contributes every IPv4 and IPv6 address it holds, at the port the `--port` option gives.
The default port is 7891.
With neither `--listen` nor `--interface` given, the server MUST listen on the IPv6 loopback address `::1` alone, at that port.

If a named interface does not exist or holds no address when the daemon starts, the daemon MUST fail to start.
It MUST NOT serve on the addresses that did resolve, and MUST NOT start with no listener at all.

> [!NOTE]
> An interface that has not come up yet looks the same as one that resolved to nothing.
> A daemon that started without its listener would report itself ready while no operator could reach it.

## Client authentication

A client MUST present a raw public key as its client certificate on every connection.
The server MUST support Ed25519 client keys.
The server MUST abort the handshake of a client whose key's fingerprint is not in its authorised key set.

> [!NOTE]
> In QUIC a client can see its own handshake complete before the server has verified the client's key.
> The server's refusal then arrives as the connection closing.

The server MUST store its authorised key set durably.

Each time it starts, the server MUST read the file `authorized_keys` in its data directory, if the file exists, and add to its authorised key set every entry not already in it.
An entry already in the set MUST keep the label it has.
Each line of the file, with surrounding whitespace removed, is blank, a comment beginning with `#`, or an entry.
An entry is a fingerprint, then optionally a single space and a label running to the end of the line.
An entry without a label MUST be given the label `bootstrap`.
If a line of the file is not blank, a comment or an entry with an acceptable fingerprint, the daemon MUST fail to start, with an error naming the line.

An operator supplies fingerprints in the file `authorized_keys` and through [key management](keys.md).
The server MUST accept an operator-supplied fingerprint with surrounding whitespace, in upper or lower case, and with or without a `sha256:` or `SHA256:` prefix.
The server MUST store and compare an operator-supplied fingerprint as 64 lowercase hexadecimal digits.
The server MUST refuse an operator-supplied fingerprint that is not 64 hexadecimal digits once surrounding whitespace and any prefix are removed.

The server MUST enforce authorisation for the life of a connection, not only at its handshake.
When a fingerprint leaves the authorised key set, the server MUST close every connection authenticated with that key, with application error code 0 and the reason `key revoked`.
Closing the connection ends every event subscription, shell session and port forward carried on it.

## Fingerprint probe

A client that does not yet trust any fingerprint for a server MUST learn the server's fingerprint with a probe connection before it connects with its own key.

A probe connection MUST present a freshly generated key that the client uses for no other connection and discards once the probe ends.
It MUST offer the same ALPN identifier and the same connection parameters as an authenticated connection, so that it differs from one only in the key it presents.
The client MUST record the fingerprint of the key the server presents during the probe's handshake, whether or not the server then refuses the connection.

The client MUST ask the operator to confirm the recorded fingerprint before it opens an authenticated connection with it.
