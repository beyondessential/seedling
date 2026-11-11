---
id: WEB
---

# Web interface serving

## Preamble

<!-- from web: None -->

The Seedling Web Interface is a browser-based operator surface for a running Seedling instance.
It exposes the same capabilities as the CLI through a graphical interface, making the full OI surface accessible to human operators without requiring a terminal.

Absent specification bugs, anything not defined here is either defined in another spec document or is implicitly not allowed.

## Transport

<!-- from web: Transport -->

> w[transport.webtransport]
> The web interface proxies OI requests over WebTransport.
> The wire format over WebTransport streams is identical to the OI wire format defined in the interface spec: each request/response cycle uses one client-initiated bidirectional stream carrying newline-terminated JSON.
> The web interface injects the caller's actor (see [wire.actor](rule:i/wire.actor)) into each proxied request.

> w[transport.http]
> The web interface provides a plain-HTTP endpoint for SPA delivery and the `POST /connect` handshake.
> This endpoint is designed to sit behind a TLS-terminating reverse proxy in non-loopback deployments.
> Browsers treat loopback origins as secure contexts, so TLS is not required for local development.

> w[transport.http.security-headers]
> Every response from the plain-HTTP endpoint carries a baseline set of browser security headers:
>
> - A Content Security Policy that denies framing, forbids plugin/object content, and restricts scripts to same-origin. The policy must still permit the SPA to load its own styles (which the UI toolkit injects inline at runtime) and `data:` images and fonts. It must permit WebTransport sessions to the WebTransport endpoint, named as a precise origin (the request's host on the WebTransport port) rather than a scheme wildcard.
> - An explicit anti-clickjacking header denying framing, for agents that predate Content Security Policy `frame-ancestors`.
> - Content-type sniffing disabled.
> - A permissions policy that denies access to browser features the interface does not use.
> - Cross-origin isolation headers that require embedded resources to be same-origin (or to explicitly opt in) and sever the opener relationship with cross-origin windows; the console embeds no third-party content.
>
> Transport-security headers (such as HSTS) are owned by the TLS-terminating reverse proxy, not this endpoint.

> w[daemon.connect-retry]
> If the initial connection to the seedling daemon fails at startup, the web interface must retry with exponential backoff rather than exiting.
> The retry interval starts at one second and doubles on each attempt up to a maximum of thirty seconds.
> Each failed attempt must produce a warning log message.
> The web interface keeps retrying indefinitely until a connection is established and verified.

> w[daemon.pin]
> The web interface verifies the daemon by pinning its SPKI fingerprint.
> The expected fingerprint may be supplied directly, or read from a file that the daemon publishes (see [transport.server-identity.published](rule:i/transport.server-identity.published)).
> When read from a file, the fingerprint is resolved on each connection attempt, so a daemon that has not yet written the file does not cause the web interface to fail at startup — it retries per [daemon.connect-retry](rule:w/daemon.connect-retry) until the file appears.

## Authentication

<!-- from web: Authentication -->

> w[auth.connect]
> `POST /connect` is the single entry point for authentication and WebTransport session initiation.
> The request body is a JSON object containing zero or one credential field: `token` (a previously issued session token), or `password` (a plaintext password for initial login).
> The server checks for a valid credential in the following order: Tailscale identity headers (if trusted, see [auth.tailscale](rule:w/auth.tailscale)), dev bypass (see [auth.dev](rule:w/auth.dev)), Bearer token from the `token` field, password from the `password` field.
> On success, the response is a JSON object with the following fields:
>
> - `token`: a session token the client must present on subsequent `POST /connect` calls.
> - `actor`: the resolved actor identity — an object with `kind`, `id`, and `display` string fields.
> - `wt_url`: a fully-qualified URL the client must use to open the WebTransport session. The URL includes a short-lived single-use token (see [wt.token](rule:w/wt.token)).
>   The URL must name a port the WebTransport endpoint is actually bound to, for every supported combination of listen configuration; a default port must not be advertised in place of a configured one.
> - `cert_hashes`: an array of one or two SHA-256 hashes (hex-encoded) of the WebTransport endpoint's current certificate(s), for use with `serverCertificateHashes`.
>
> On failure, the response is HTTP 401 with a JSON body:
> ```json
> { "auth_required": "password" }
> ```
> The `auth_required` field names the credential type the client must collect and supply in a subsequent `POST /connect` call.

> w[auth.tailscale]
> When the web interface is explicitly configured to trust Tailscale identity headers, requests carrying those headers are authenticated without a password and the Tailscale user identity is used as the actor.
> The user's display name is decoded from any MIME encoded-word (RFC 2047) form the proxy uses to carry non-ASCII names in an ASCII header; a value that is not so encoded is used as-is.
> The web interface must not honour Tailscale identity headers unless this trust is explicitly enabled by the operator.
> This mode is intended for deployments where the plain-HTTP endpoint is fronted by Tailscale Serve, which injects identity headers and enforces network-level access control.

> w[auth.password]
> Password-based authentication is supported.
> The configured password is validated using Argon2id.
> On success a session token is issued and returned in the `POST /connect` response.
> Session tokens have a bounded lifetime; the lifetime must be configurable.

> w[auth.dev]
> The web interface may be configured to bypass all authentication.
> This mode must be rejected at startup if any configured bind address is not a loopback address.

## WebTransport Session

<!-- from web: WebTransport Session -->

> w[wt.cert]
> The WebTransport endpoint uses a self-signed ECDSA-P256 certificate.
> The certificate validity period must not exceed 14 days.
> The SHA-256 hash of the current certificate is included in every successful `POST /connect` response so the client can open WebTransport using `serverCertificateHashes` without requiring CA trust.

> w[wt.cert.rotation]
> The web interface rotates the WebTransport certificate before it expires.
> During rotation the web interface maintains an overlap window of at least 24 hours during which both the expiring certificate and the new certificate are accepted.
> The hashes of both certificates are included in `POST /connect` responses during the overlap window.
> Clients that reconnect after a rotation automatically receive the new hash by calling `POST /connect` again.
> Rotation changes the certificate the listener presents to new sessions without releasing the listener's address, and established sessions continue across it.

> w[wt.token]
> The `wt_url` returned by `POST /connect` embeds a short-lived single-use token.
> The WebTransport handshake is rejected if the token is absent, expired, or has already been consumed.
> Tokens exist solely to bridge the gap between `POST /connect` and the WebTransport handshake; they carry no long-term session state.

> w[wt.actor]
> The web interface resolves the caller's actor identity during `POST /connect` and associates it with the WebTransport session.
> Every OI request proxied through the session carries that actor.
> The actor does not change for the lifetime of a WebTransport session.

## SPA Delivery

<!-- from web: SPA Delivery -->

> w[spa.delivery]
> The web interface serves a static single-page application from the plain-HTTP endpoint.
> The SPA must be loaded from a secure context — either a loopback address or a TLS-terminating reverse proxy — so that the browser permits WebTransport connections.
> Static assets are served with long-lived cache headers; the entry point (`index.html`) must not be cached beyond the current deployment.

## Bind Configuration

<!-- from web: Bind Configuration -->

> w[bind]
> The web interface may be configured to listen on one or more addresses.
> The plain-HTTP listener and the WebTransport listener use independent ports.
> The WebTransport listener may be configured to bind a different set of addresses than the HTTP listener: WebTransport cannot be carried by an HTTP-terminating reverse proxy, so a deployment that fronts the HTTP listener with such a proxy bound to loopback must still be able to expose the WebTransport listener directly to clients.
> When the WebTransport addresses are not separately configured, both listeners share the same set of addresses.
> When no addresses are explicitly configured, both listeners bind to a loopback address only.
> Failure to bind any configured address at startup is a fatal error.
