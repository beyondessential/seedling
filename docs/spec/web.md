The Seedling Web Interface is a browser-based operator surface for a running Seedling instance.
It exposes the same capabilities as the CLI through a graphical interface, making the full OI surface accessible to human operators without requiring a terminal.

Absent specification bugs, anything not defined here is either defined in another spec document or is implicitly not allowed.

# Transport

> w[transport.webtransport]
> The web interface proxies OI requests over WebTransport.
> The wire format over WebTransport streams is identical to the OI wire format defined in the interface spec: each request/response cycle uses one client-initiated bidirectional stream carrying newline-terminated JSON.
> The web interface injects the caller's actor (see [wire.actor](interface.md#i--wire.actor)) into each proxied request.

> w[transport.http]
> The web interface provides a plain-HTTP endpoint for SPA delivery and the `POST /connect` handshake.
> This endpoint is designed to sit behind a TLS-terminating reverse proxy in non-loopback deployments.
> Browsers treat loopback origins as secure contexts, so TLS is not required for local development.

# Authentication

> w[auth.connect]
> `POST /connect` is the single entry point for authentication and WebTransport session initiation.
> The request body is a JSON object containing zero or one credential field: `token` (a previously issued session token), or `password` (a plaintext password for initial login).
> The server checks for a valid credential in the following order: Tailscale identity headers (if trusted, see [auth.tailscale](#w--auth.tailscale)), dev bypass (see [auth.dev](#w--auth.dev)), Bearer token from the `token` field, password from the `password` field.
> On success, the response is a JSON object with the following fields:
>
> - `token`: a session token the client must present on subsequent `POST /connect` calls.
> - `actor`: the resolved actor identity — an object with `kind`, `id`, and `display` string fields.
> - `wt_url`: a fully-qualified URL the client must use to open the WebTransport session. The URL includes a short-lived single-use token (see [wt.token](#w--wt.token)).
> - `cert_hashes`: an array of one or two SHA-256 hashes (hex-encoded) of the WebTransport endpoint's current certificate(s), for use with `serverCertificateHashes`.
>
> On failure, the response is HTTP 401 with a JSON body:
> ```json
> { "auth_required": "password" }
> ```
> The `auth_required` field names the credential type the client must collect and supply in a subsequent `POST /connect` call.

> w[auth.tailscale]
> When the web interface is explicitly configured to trust Tailscale identity headers, requests carrying those headers are authenticated without a password and the Tailscale user identity is used as the actor.
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

# WebTransport Session

> w[wt.cert]
> The WebTransport endpoint uses a self-signed ECDSA-P256 certificate.
> The certificate validity period must not exceed 14 days.
> The SHA-256 hash of the current certificate is included in every successful `POST /connect` response so the client can open WebTransport using `serverCertificateHashes` without requiring CA trust.

> w[wt.cert.rotation]
> The web interface rotates the WebTransport certificate before it expires.
> During rotation the web interface maintains an overlap window of at least 24 hours during which both the expiring certificate and the new certificate are accepted.
> The hashes of both certificates are included in `POST /connect` responses during the overlap window.
> Clients that reconnect after a rotation automatically receive the new hash by calling `POST /connect` again.

> w[wt.token]
> The `wt_url` returned by `POST /connect` embeds a short-lived single-use token.
> The WebTransport handshake is rejected if the token is absent, expired, or has already been consumed.
> Tokens exist solely to bridge the gap between `POST /connect` and the WebTransport handshake; they carry no long-term session state.

> w[wt.actor]
> The web interface resolves the caller's actor identity during `POST /connect` and associates it with the WebTransport session.
> Every OI request proxied through the session carries that actor.
> The actor does not change for the lifetime of a WebTransport session.

# SPA Delivery

> w[spa.delivery]
> The web interface serves a static single-page application from the plain-HTTP endpoint.
> The SPA must be loaded from a secure context — either a loopback address or a TLS-terminating reverse proxy — so that the browser permits WebTransport connections.
> Static assets are served with long-lived cache headers; the entry point (`index.html`) must not be cached beyond the current deployment.

# Operator Capabilities

> w[routes.apps]
> The web interface exposes the full app management surface of the OI:
> listing registered apps and their statuses; showing detailed app status including resources, faults, params, install requirements, and actions; registering new apps with a BSL script; updating an app's BSL script; deregistering apps; setting and unsetting parameters; scaling deployments; viewing generation history; planning proposed changes; invoking lifecycle actions; and invoking the install action with its requirements.

> w[routes.logs]
> The web interface exposes log streaming for app workload containers and infrastructure components.

> w[routes.events]
> The web interface exposes the OI event feed, delivering a live stream of runtime events to the operator.

> w[routes.shells]
> The web interface exposes interactive shell sessions, allowing operators to open a terminal session within an app's context directly from the browser.

> w[routes.keys]
> The web interface exposes OI key management: listing authorized client keys, authorising new keys, and revoking existing keys.

> w[routes.registries]
> The web interface exposes the container registry allowlist: listing, adding, and removing registry hostnames.

> w[routes.backups]
> The web interface exposes backup management: registering and deregistering backup apps; creating, listing, showing, updating, and deleting backup strategies; triggering immediate backups; listing snapshots; and restoring snapshots.

> w[routes.sessions]
> The web interface must provide a connected-clients view showing all active web UI sessions, open CLI shell sessions, and active port forwards. Each entry must show at minimum the client identity, the connected or opened timestamp, and — for shells and forwards — the associated app.

# Bind Configuration

> w[bind]
> The web interface may be configured to listen on one or more addresses.
> The plain-HTTP listener and the WebTransport listener share the same set of configured addresses but use independent ports.
> When no addresses are explicitly configured, both listeners bind to a loopback address only.
> Failure to bind any configured address at startup is a fatal error.
