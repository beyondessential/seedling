---
id: WEB
---

# Web interface server

Applies to the service that delivers the [console](console.md#console) to browsers and relays its requests to a [host](../overview.md#host)'s [Operator Interface](../overview.md#operator-interface).

## Borrowed terms

| term | meaning |
| --- | --- |
| secure context | An environment in which a browser exposes features such as WebTransport, as defined by [Secure Contexts](https://www.w3.org/TR/secure-contexts/). |
| WebTransport session | A session between a browser and a server carrying streams, as defined by [WebTransport](https://www.w3.org/TR/webtransport/) and [WebTransport over HTTP/3](https://datatracker.ietf.org/doc/draft-ietf-webtrans-http3/). |
| bidirectional stream, unidirectional stream | A stream within a WebTransport session carrying data in both directions, or from its opener only, as defined by [WebTransport](https://www.w3.org/TR/webtransport/). |
| `serverCertificateHashes` | The WebTransport option through which a page names the hashes of the certificates it accepts in place of certificate authority validation, as defined by [WebTransport](https://www.w3.org/TR/webtransport/#web-transport-configuration). |
| encoded-word | A MIME encoding of non-ASCII text in a header value, as defined by [RFC 2047](https://www.rfc-editor.org/rfc/rfc2047). |
| PHC string | A self-describing encoding of a password hash and its parameters, as defined by the [PHC string format](https://github.com/P-H-C/phc-string-format/blob/master/phc-sf-spec.md). |

## Terminology

### Web interface

The service that serves the console to browsers and relays their requests to a host's daemon.

### Web session

One WebTransport session between a client and the web interface, from its acceptance until it closes or is reaped.

### Session token

A credential the web interface issues on a successful `/connect`, which a client presents on a later `/connect` in place of the credential it first used.

### WebTransport token

A short-lived, single-use credential embedded in the URL of a web session, binding that session to the actor resolved by `/connect`.

## Configuration

The web interface MUST accept the following command-line options.

| option | meaning | default |
| --- | --- | --- |
| `--interface <name>,…` | Network interfaces whose addresses both listeners bind | none |
| `--listen <address>` | A socket address both listeners bind; repeatable | none |
| `--http-port <port>` | The HTTP port for addresses taken from `--interface` | `7894` |
| `--wt-interface <name>,…` | Network interfaces whose addresses the WebTransport listener binds, in place of the shared ones | none |
| `--wt-listen <address>` | A socket address the WebTransport listener binds, in place of the shared ones; repeatable | none |
| `--wt-port <port>` | The WebTransport port for addresses taken from an interface | `7893` |
| `--trust-tailscale-headers` | Authenticate by [Tailscale identity](#tailscale-identity) | off |
| `--dev-no-auth` | Enable the [development bypass](#development-bypass) | off |
| `--daemon-addr <address>` | The socket address of the daemon's Operator Interface | `[::1]:7891` |
| `--daemon-fingerprint <hex>` | The daemon's SPKI fingerprint to pin | none |
| `--daemon-fingerprint-file <path>` | A file holding the daemon's SPKI fingerprint to pin | none |
| `--key-file <path>` | The web interface's persistent client key | a file in the user's state directory |
| `--config <path>` | The configuration file | none |

The web interface MUST NOT start when `--listen` is given together with `--http-port` or `--wt-port`.
The web interface MUST NOT start when both `--daemon-fingerprint` and `--daemon-fingerprint-file` are given.

The configuration file is a [TOML](https://toml.io/en/v1.0.0) document.
Its `[auth]` table holds these keys, each optional.

| key | type | meaning | default |
| --- | --- | --- | --- |
| `password_hash` | string | An Argon2id PHC string of the operator password; password login is disabled while it is unset | none |
| `session_lifetime_secs` | integer | The lifetime of a session token, in seconds | `86400` |

A configuration file that cannot be read or parsed MUST stop the web interface from starting.
A missing `[auth]` table MUST take the same defaults as an empty one.

## Listen addresses

The web interface MUST serve an HTTP endpoint over TCP, without TLS, and a WebTransport endpoint over UDP.

Each listener binds a set of socket addresses.
An address given explicitly carries its own port.
An address taken from an interface name pairs each address of that interface with the listener's port.
When neither addresses nor interfaces are configured for a listener, it MUST bind `127.0.0.1` on its port.
The WebTransport listener MUST use the shared `--interface` and `--listen` configuration unless `--wt-interface` or `--wt-listen` is given, in which case it MUST use those alone.

An interface name that matches no interface on the host contributes no addresses, and the web interface SHOULD log a warning naming it.
If a listener's configuration resolves to no address, the web interface MUST NOT start.
A failure to bind any resolved address MUST stop the web interface.

Every address the WebTransport listener binds MUST share one port.
The web interface MUST NOT start if they do not.
That port is the one the web interface advertises in each `wt_url` and in its [content security policy](#security-headers).

> [!NOTE]
> WebTransport cannot be carried by a reverse proxy that terminates HTTP.
> A deployment that fronts the HTTP endpoint with such a proxy on loopback binds the WebTransport listener separately, on an address browsers reach directly.

## Daemon connection

The web interface MUST connect to the daemon's Operator Interface as a client, as [TRN](../interface/transport.md) specifies.
It MUST authenticate with a persistent client key, generating it on first start, so that an [operator](../overview.md#operator) authorises the web interface once.
It SHOULD log the key's fingerprint when it starts.

The web interface MUST verify the daemon by pinning its [SPKI fingerprint](../interface/transport.md#server-identity).
The fingerprint MAY be given directly, or as the path of a file holding it, such as the [file the daemon publishes](../interface/transport.md#server-identity).
The web interface MUST NOT start when it is given neither.
When given a file, the web interface MUST read it afresh on every connection attempt and ignore surrounding whitespace.
A file that is absent, unreadable or holds only whitespace MUST count as a failed attempt.

A connection attempt succeeds only once the daemon has answered a request on the connection.
An error response counts as an answer.

> [!NOTE]
> In TLS 1.3 the server verifies the client's certificate after the client considers the handshake complete, so a daemon that does not authorise the web interface's key is detected only by the first request.

If an attempt fails, the web interface MUST try again, waiting one second before the second attempt and doubling the wait after each further failure, up to thirty seconds.
It MUST log a warning for each failed attempt, and MUST keep trying until an attempt succeeds.
A failed attempt MUST NOT make the web interface exit.

When a request finds the daemon connection broken, the web interface MUST open a replacement, verified as at startup, and use it for that request and those after it.
Requests that found the same broken connection MUST share one replacement attempt.
The web interface MUST NOT close any daemon connection other than the one found broken.

Requests the web interface makes on its own behalf MUST carry its own [actor](../interface/wire.md#actor-attribution), with `kind` `web`, `id` the first eight hexadecimal digits of its client key's fingerprint, and `display` `seedling-web`.

## Console delivery

The HTTP endpoint MUST serve the console, [`POST /connect`](#authentication), and `GET /healthz`.
`GET /healthz` MUST answer `200 OK` with the JSON body `{"status":"ok"}`.

The web interface MUST serve the console's files for `GET` requests to their paths.
A request for a path under `/assets/` that names no file MUST be answered `404 Not Found`.
Any other request for a path that names no file MUST be answered with the console's entry point, `index.html`, so that the console handles its own routes.
Each file MUST be served with the `Content-Type` its file extension implies.

A file under `/assets/` MUST be served with `Cache-Control: public, max-age=31536000, immutable` ([RFC 9111](https://www.rfc-editor.org/rfc/rfc9111), [RFC 8246](https://www.rfc-editor.org/rfc/rfc8246)).
A file's path under `/assets/` MUST change whenever its content changes.
Every other file, including the entry point, MUST be served with `Cache-Control: no-cache, no-store, must-revalidate`.

A deployment that serves the console other than over loopback MUST serve it to browsers through a reverse proxy that terminates TLS.

> [!NOTE]
> A browser reaches WebTransport only from a secure context.
> A browser treats `http://127.0.0.1` and `http://localhost` as potentially trustworthy origins, so the console can run over plain HTTP on loopback.

## Security headers

Every response from the HTTP endpoint MUST carry these headers.

| header | required content |
| --- | --- |
| `Content-Security-Policy` | `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'`, followed by the `connect-src` directive below |
| `X-Frame-Options` | `DENY` |
| `X-Content-Type-Options` | `nosniff` |
| `Permissions-Policy` | `accelerometer=(), autoplay=(), camera=(), display-capture=(), encrypted-media=(), fullscreen=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), midi=(), payment=(), usb=()` |
| `Cross-Origin-Embedder-Policy` | `require-corp` |
| `Cross-Origin-Opener-Policy` | `same-origin` |
| `Cross-Origin-Resource-Policy` | `same-origin` |

The policy's `connect-src` directive MUST be `connect-src 'self' https://<host>:<port>`, where `<host>` is the host the request names in its `Host` header without its port, and `<port>` is the WebTransport listener's advertised port.
An IPv6 literal MUST keep its brackets.
For a request without a `Host` header, `<host>` MUST be `localhost`, as for the [`wt_url`](#session-establishment) the web interface then gives.
The policy MUST NOT name the WebTransport endpoint by a scheme alone.

The headers are defined by [Content Security Policy Level 3](https://www.w3.org/TR/CSP3/), [RFC 7034](https://www.rfc-editor.org/rfc/rfc7034) (`X-Frame-Options`), [Fetch](https://fetch.spec.whatwg.org/) (`X-Content-Type-Options`, `Cross-Origin-Resource-Policy`), [Permissions Policy](https://www.w3.org/TR/permissions-policy/), and [HTML](https://html.spec.whatwg.org/) (`Cross-Origin-Embedder-Policy`, `Cross-Origin-Opener-Policy`).

> [!NOTE]
> The console's interface toolkit injects style elements at run time, so the policy admits inline styles.
> `X-Frame-Options` covers browsers that predate `frame-ancestors`.

## Authentication

`POST /connect` authenticates a client and issues the credentials it needs to open a web session.
Its body MUST be a JSON object ([RFC 8259](https://www.rfc-editor.org/rfc/rfc8259)), sent as `application/json`, with these members.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `token` | string | no | A session token from an earlier `/connect` |
| `password` | string | no | The operator password |

A client MUST send at most one of `token` and `password`.
A client holding a session token MUST send it as `token` on each later `/connect`.

The web interface MUST try these credential sources in order, and authenticate the request by the first that succeeds:

1. [Tailscale identity](#tailscale-identity), when trusted.
2. The [development bypass](#development-bypass), when enabled.
3. The `token` member.
4. The `password` member, checked as [password login](#password-login) specifies.

The web interface MUST resolve an [actor](../interface/wire.md#actor-attribution) for the request from the source that authenticated it.

| source | `kind` | `id` | `display` | `session` |
| --- | --- | --- | --- | --- |
| Tailscale identity | `tailscale` | the `Tailscale-User-Login` value | the decoded `Tailscale-User-Name` value, or the login when that header is absent | new |
| development bypass | `dev` | `dev` | `dev` | new |
| session token | as issued | as issued | as issued | as issued |
| password | `password` | `admin` | `admin` | new |

A new `session` value MUST be unique to the `/connect` call that creates it.

On success, the web interface MUST answer `200 OK` with a JSON object of these members.

| member | type | meaning |
| --- | --- | --- |
| `token` | string | A new session token for the resolved actor |
| `actor` | object | The resolved actor |
| `wt_url` | string | The URL at which the client opens its web session, as [session establishment](#session-establishment) specifies |
| `cert_hashes` | array of string | The SHA-256 hashes of the WebTransport certificates, as the [WebTransport certificate](#webtransport-certificate) section specifies |

The web interface MUST issue a new session token on every successful `/connect`, including one authenticated by a session token.
A session token MUST be unpredictable.
A session token MUST stop authenticating once the configured session lifetime has passed since it was issued.

If no source authenticates the request, the web interface MUST answer `401 Unauthorized` ([RFC 9110](https://www.rfc-editor.org/rfc/rfc9110)) with a body whose `auth_required` member names the credential the client collects from the operator for its next `/connect`.
`auth_required` MUST be `"password"` while [password login](#password-login) can authenticate a password, and `"none"` otherwise, when no credential the operator could give would authenticate the request.

### Password login

The web interface MUST verify a password against the configured `password_hash` as Argon2id ([RFC 9106](https://www.rfc-editor.org/rfc/rfc9106)), with the parameters its PHC string carries.
While no `password_hash` is configured, the web interface MUST NOT authenticate any password.
If the configured value is not a valid PHC string, the web interface MUST NOT authenticate any password, and SHOULD log a warning.

### Tailscale identity

The web interface MUST NOT read Tailscale identity headers unless `--trust-tailscale-headers` is given.
When it is, a request whose `Tailscale-User-Login` header holds a visible ASCII value MUST authenticate as that Tailscale user.
A request without that header falls through to the next credential source.

The web interface MUST decode a `Tailscale-User-Name` value made of encoded-words, and MUST use a value that is not so encoded, or does not decode, as it stands.

> [!NOTE]
> This trust is intended for deployments where [Tailscale Serve](https://tailscale.com/kb/1312/serve) fronts the HTTP endpoint and enforces access control at the network level.
> Tailscale Serve sets these headers on the requests it proxies and strips them from incoming requests, so a request that bypasses it can carry any identity.

### Development bypass

When `--dev-no-auth` is given, every `/connect` request MUST authenticate.
The web interface MUST NOT start with `--dev-no-auth` if any address either listener binds is not a loopback address.

## WebTransport certificate

The WebTransport endpoint MUST present a self-signed certificate that meets the [custom certificate requirements](https://www.w3.org/TR/webtransport/#custom-certificate-requirements) of WebTransport.
Its key MUST be an ECDSA key on the P-256 curve ([FIPS 186-5](https://csrc.nist.gov/pubs/fips/186-5/final)).

> [!NOTE]
> P-256 is the one key algorithm WebTransport requires every browser to accept for certificate hashes.

Each entry of `cert_hashes` MUST be the SHA-256 hash ([FIPS 180-4](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)) of a certificate's DER encoding, as lowercase hexadecimal.
`cert_hashes` MUST hold the hash of the certificate the endpoint presents, and MAY hold one more.

The web interface MUST stop presenting a certificate before its validity period ends, and present a new one to new sessions in its place.
From at least 24 hours before it begins presenting a new certificate, the web interface MUST include that certificate's hash in `cert_hashes`.
Changing the presented certificate MUST NOT release the endpoint's addresses or end established web sessions.

## Session establishment

`wt_url` MUST have the form `https://<host>:<port>/wt?t=<token>`.
`<host>` is the host the `/connect` request names in its `Host` header without its port, keeping the brackets of an IPv6 literal, or `localhost` when the request has no `Host` header.
`<port>` is the WebTransport listener's advertised port, and `<token>` a new WebTransport token.

A WebTransport token MUST be unpredictable and MUST expire 30 seconds after it is issued.
A client MUST open a web session at `wt_url` with `serverCertificateHashes` set to the hashes in `cert_hashes`.

The web interface MUST answer a session request whose path is not `/wt` with `404 Not Found`.
It MUST answer `403 Forbidden` to a session request whose `t` query parameter is absent, names no WebTransport token, or names one that has expired or been presented before.
Presenting a WebTransport token MUST consume it.

The web interface MUST bind each web session to the actor the `/connect` call that issued its token resolved.
That actor MUST NOT change for the lifetime of the web session.

## Request relay

A client MUST send each request on its own bidirectional stream of its web session.
The stream MUST begin with a request object as [WIRE](../interface/wire.md) defines, terminated by a newline or by the end of the client's direction of the stream.

The web interface MUST NOT relay a request line longer than 4 MiB (4,194,304 bytes), or one that is not a JSON object.
It MUST answer such a request with `requirements_invalid`, as the daemon answers a [control request](../interface/wire.md#control-requests) it cannot parse.

The web interface MUST set the request's top-level `actor` to the web session's actor, replacing any actor the client supplied.

The web interface handles these methods itself.

| method | handling |
| --- | --- |
| `/connected-clients/heartbeat` | [Session tracking](#session-tracking) |
| `/connected-clients/list` | [Connected clients](#connected-clients) |
| `/events/subscribe` | [Event relay](#event-relay) |
| `/logs/stream` | Subscription, below |
| `/shells/start`, `/volumes/shell` | [Shell relay](#shell-relay) |

For any other method, the web interface MUST open a bidirectional stream to the daemon on its shared connection and write the request to it, followed by a newline.
It MUST then relay the rest of the client's stream to the daemon, and the daemon's stream to the client, each unchanged, and finish each direction when its source ends.

For `/events/subscribe` and `/logs/stream`, the web interface MUST answer on the client's stream with a response followed by a newline.
After a successful response it MUST write the subscribed records to the same stream as newline-delimited JSON objects.
After an error response it MUST finish the stream.

The web interface MUST relay each log stream over a daemon connection of its own, and MUST close that connection when the stream ends.
When the daemon refuses a log stream, the web interface MUST relay the daemon's error `code` and `message`.
When the daemon ends a log stream, the web interface MUST finish the client's stream.

The web interface returns these error codes in addition to those [WIRE](../interface/wire.md#error-codes) defines.

| code | meaning |
| --- | --- |
| `daemon_unavailable` | The web interface could not open a stream or connection to the daemon |
| `protocol_error` | The daemon's response to a shell request could not be read |

## Event relay

The web interface MUST hold one subscription to the daemon's [event feed](../interface/events.md) and relay its events to every client subscribed through it.
It MUST relay the events it originates, as [session tracking](#session-tracking) defines, on the same feed.

When its subscription ends or fails, the web interface MUST subscribe again.
It MUST wait one second after a subscription that ended cleanly, and otherwise double the previous wait, up to thirty seconds.

A client subscribing through the web interface MUST first receive the most recent events the web interface relayed, up to 200, and then live events from that point on.
Each event MUST reach a subscriber once, in the order the web interface relayed it, with no gap or overlap between the replayed and the live events.

The web interface MUST relay the daemon's `EventsDropped` events unchanged.
When a subscriber falls behind the web interface's own feed and the web interface skips events for it, the web interface MUST write an `EventsDropped` event to that subscriber, with the meaning [event gap](../interface/events.md#gaps) gives it.
When the web interface subscribes to the daemon again after losing a subscription, it MUST relay an `EventsDropped` event without a `skipped` member to every subscriber.

## Shell relay

For a `/shells/start` ([SHL](../interface/shells.md#opening-an-app-shell)) or `/volumes/shell` ([SHL](../interface/shells.md#volume-shells)) request, the web interface MUST open a bidirectional stream to the daemon on its shared connection and write the request to it.
It MUST relay the daemon's first line to the client unchanged, whether a handshake response or an error response.
After an error response it MUST finish the client's stream.
If the daemon's first line cannot be read, or exceeds 64 KiB, the web interface MUST answer the client with a `protocol_error` error response.

After a handshake response, the web interface MUST open two unidirectional streams to the client, one carrying the session's stdout stream and one its stderr stream.
Each MUST begin with eight bytes holding the handshake response's `stdout_stream_id` or `stderr_stream_id`, as an unsigned big-endian integer, followed by the bytes of the daemon's corresponding stream.
Output streams open at the same time on one web session MUST begin with different eight bytes.
A client MUST route each incoming unidirectional stream to its shell by those eight bytes, and MUST accept a stream that arrives before it has read the handshake response.

The web interface MUST relay the bytes the client writes after its request to the daemon unchanged, and finish the daemon stream when the client's direction ends.
It MUST relay the daemon's [exit frame](../interface/shells.md#session-end) to the client and then finish the client's stream.
If the daemon's stream ends without a complete, newline-terminated exit frame, the web interface MUST stop the shell with [`/shells/stop`](../interface/shells.md#stopping-a-session).

The web interface MUST take a shell's output streams from the daemon connection that carried its handshake.
When the web interface replaces its daemon connection, shells started on the old connection end, and the old connection's closing MUST NOT affect shells started on the new one.

## Session tracking

On accepting a web session, the web interface MUST record it with a new session id, its `connected_at` time, a `last_seen` time equal to `connected_at`, its actor, and the safety mode `read`.

A client reports that its web session is alive with `/connected-clients/heartbeat`, whose params have one optional member.

| member | type | meaning |
| --- | --- | --- |
| `safety_mode` | string | The client's current [safety mode](console.md#safety-mode) |

The web interface MUST set the session's `last_seen` to the time it handles the heartbeat.
A `safety_mode` of `write` or `dangerous` MUST be recorded as such, and any other value present MUST be recorded as `read`.
A heartbeat without `safety_mode` MUST leave the recorded mode unchanged.
The recorded safety mode MUST NOT affect how the web interface handles any request.

The web interface MUST answer a heartbeat with a result of these members, followed by a newline.

| member | type | meaning |
| --- | --- | --- |
| `alive` | boolean | Whether the session was still recorded |
| `now` | string | The time the heartbeat was handled, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |
| `session_id` | string | The web session's id |

A client MUST send a heartbeat at least once every 200 seconds while its web session is open.
A client MUST include its current safety mode in every heartbeat.
A client SHOULD send a heartbeat as soon as the session opens.
A client MUST send a heartbeat as soon as its safety mode changes.
On the page's `pagehide` event ([HTML](https://html.spec.whatwg.org/)), a client MUST attempt to send a heartbeat or to close its web session, without waiting for the attempt to complete.

A web session is stale once ten minutes have passed since its `last_seen`.
The web interface MUST stop recording a stale web session, and SHOULD do so within a minute of it becoming stale.
A heartbeat for a session no longer recorded MUST NOT record it again.
A client answered with `alive` false MUST close that web session and open a new one.

The web interface MUST emit these events on the feed it relays.

| `type` | when | additional members |
| --- | --- | --- |
| `WebSessionStarted` | A web session is accepted | `session_id`, `safety_mode` |
| `WebSessionModeChanged` | A heartbeat changes a session's recorded safety mode | `session_id`, `safety_mode` |
| `WebSessionStopped` | A web session closes or is reaped as stale | `session_id` |

Each event carries `type` and an RFC 3339 `timestamp`, and no `actor`.
The web interface MUST emit exactly one `WebSessionStopped` event for each web session.

## Connected clients

`/connected-clients/list` takes no parameters, and its result has these members.

| member | type | meaning |
| --- | --- | --- |
| `web` | array of object | Every recorded web session |
| `shells` | array of object | The daemon's [shell records](../interface/shells.md#listing-sessions) |
| `forwards` | array of object | The daemon's [port forwards](../interface/port-forwards.md#listing-forwards) |
| `actors` | array of object | The actors recently active on the host |

Each `web` entry has these members.

| member | type | meaning |
| --- | --- | --- |
| `id` | string | The session id |
| `connected_at` | string | When the session was accepted, as an RFC 3339 timestamp |
| `last_seen` | string | The session's `last_seen`, as an RFC 3339 timestamp |
| `actor_kind`, `actor_id`, `actor_display` | string or null | The session actor's `kind`, `id` and `display` |
| `safety_mode` | string | The recorded safety mode |

If the web interface cannot read the shells or the forwards from the daemon, it MUST fail the request with `daemon_unavailable`, and MUST NOT report them as an empty array.

`actors` MUST hold one entry for each pair of non-empty actor `kind` and `id` carried by an event on the relayed feed in the past ten minutes.

| member | type | meaning |
| --- | --- | --- |
| `actor_kind`, `actor_id` | string | The actor's `kind` and `id` |
| `actor_display` | string or null | The `display` of the most recent such event that carried one |
| `last_seen` | string | The `timestamp` of the most recent such event, as an RFC 3339 timestamp |
| `last_action` | string | A short human-readable summary of that event |
