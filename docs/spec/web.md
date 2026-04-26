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

> w[daemon.connect-retry]
> If the initial connection to the seedling daemon fails at startup, the web interface must retry with exponential backoff rather than exiting.
> The retry interval starts at one second and doubles on each attempt up to a maximum of thirty seconds.
> Each failed attempt must produce a warning log message.
> The web interface keeps retrying indefinitely until a connection is established and verified.

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

> w[routes.apps.healthcheck-indicator]
> When a container resource (Deployment or Job) has a declared [healthcheck](language.md#l--deployment.healthcheck), the app detail page must show a small indicator alongside the resource's lifecycle state.
> The indicator must convey:
>
> - that a healthcheck is declared,
> - the configured `on_failure` response,
> - the current check state derived from recent observations: passing, failing, or in start-period.
>
> The indicator's colour must align with the existing status palette: a healthy state uses the success colour, a failing state uses the error colour, and the start-period state uses the warning or neutral colour.
> Hovering or focusing the indicator must reveal the declared `kind`, `on_failure`, and (for `kind: "command"`) a truncated form of the `cmd`.

> w[routes.logs]
> The web interface exposes log streaming for app workload containers and infrastructure components.

> w[routes.events]
> The web interface exposes the OI event feed, delivering a live stream of runtime events to the operator.

> w[routes.shells]
> The web interface exposes interactive shell sessions, allowing operators to open a terminal session within an app's context directly from the browser.
> See [shells.wire](#w--shells.wire), [shells.ui](#w--shells.ui), and [shells.resize](#w--shells.resize) for the wire protocol and UI details.

> w[shells.wire]
> The browser opens a single WT bidirectional stream and writes one newline-terminated JSON request line `{"method":"/shells/start","params":{"app","name","rows","cols"}}`.
> The gateway writes back one newline-terminated JSON line — either `{"result":{"session_id","stdout_stream_id","stderr_stream_id",...}}` or `{"error":{...}}` — then the stream carries raw stdin bytes upstream and the final `{"exit_code":N}\n` frame downstream, followed by FIN.
> The gateway also opens two server-initiated WT unidirectional streams, one for stdout and one for stderr.
> Each uni stream begins with an 8-byte big-endian QUIC stream ID that matches the corresponding `stdout_stream_id` or `stderr_stream_id` in the handshake response; subsequent bytes are raw PTY output.
> The browser reads the 8-byte prefix to route each uni stream to the correct shell session.
> Since the gateway uses a shared QUIC connection to the daemon, stream IDs are unique across all concurrent shells on a given browser session; no rewriting or additional multiplexing is required.

> w[shells.resize]
> Terminal resize is sent as a standard `/shells/resize` OI request over the browser's shared WebTransport session.
> The browser coalesces resize events to at most one in-flight request at a time.

> w[shells.exit]
> When the daemon session ends, the gateway forwards the final `{"exit_code":N}\n` frame from the daemon's server-initiated bidi to the browser's bidi stream and closes the downstream half.
> The browser reads this frame to surface the exit code in the UI per [shells.ui](#w--shells.ui).

> w[volumes.shell-ui]
> Each site volume row in the Volumes page and each volume resource row in the App Detail page expose an "Open shell" button.
> Clicking the button immediately opens a volume shell session (via `/volumes/shell`) for that single volume, using the existing shell sidebar.
> The tab label is the volume's display name.

> w[shells.ui]
> The app detail page exposes each shell defined in the app (from `/apps/show`'s `actions[].kind=="shell"`) as an "Open shell" button.
> If the shell declares `params`, clicking the button first shows a params dialog (identical in structure to the action invoke dialog) to collect param values before opening the session.
> Shells open in a persistent tabbed sidebar on the left of the main layout.
> Multiple shell sessions may be open simultaneously, each in its own tab.
> The sidebar must fit the terminal to the available space and send resize events when the sidebar or window size changes.
> On clean exit, an overlay must show the exit code with options to close or reopen the shell.
> Closing a tab before a clean exit must trigger `/shells/stop` to tear down the daemon session.

> w[routes.keys]
> The web interface exposes OI key management: listing authorized client keys, authorising new keys, and revoking existing keys.

> w[routes.registries]
> The web interface exposes the container registry allowlist: listing, adding, and removing registry hostnames.

> w[routes.images]
> The web interface provides a dedicated Images page at `/images` showing:
>
> - A table of every container image in local storage — with its references, size, last-used timestamp, and pin/in-use status — and a per-row remove action.
> - A separate table of current image pins — with the pinning app, reference, and pinned-at timestamp — and a per-row clear action.
> - A "clear unused" button that removes every image that is not currently backing a running container, issued as a batch of individual remove calls.
>
> The page must not expose the force-remove option, nor a generic pull action; both remain OI/CLI-only affordances to keep the browser surface low-risk.

> w[routes.images.app-detail]
> Each app's detail page must show a table of the images that the app is directly concerned with — every image currently in-use by one of the app's running containers, together with every image the app has pinned. Each row exposes a remove action (non-forceful) and a single "clear all pins" button is offered for the app as a whole.

> w[routes.images.confirm]
> Both the images page and the app-detail images table must require confirmation before issuing an image remove, and the confirmation must state that remove fails if the image is currently in use.

> w[routes.images.discover]
> The app detail Images section must offer a "Discover from handlers" action that calls [`/apps/images/discover`](interface.md#i--image.discover) with `lenient: true`, merging the discovered image references into the displayed table as a third state — _potentially used_ — distinct from _in use_ and _pinned_.
> When any handler's probe reports an error or is skipped, that must be surfaced inline near the discover results so the operator can decide to re-run the probe with explicit param values.
> A "Warm all discovered" button must enqueue a pull-and-pin for each discovered image reference that is not already present or pinned.

> w[routes.backups]
> The web interface exposes backup management: registering and deregistering backup apps; creating, listing, showing, updating, and deleting backup strategies; triggering immediate backups; listing snapshots; and restoring snapshots.

> w[routes.sessions]
> The web interface must provide a connected-clients view showing all active web UI sessions, open CLI shell sessions, and active port forwards. Each entry must show at minimum the client identity, the connected or opened timestamp, and — for shells and forwards — the associated app.

> w[routes.volumes]
> The web interface exposes volume management: listing, creating, and deleting site volumes (managed, bind-mount, and snapshot kinds); listing volumes exported by apps; listing, adding, remapping, and removing external volume mappings; and listing and confirming deletion of held volumes.

> w[routes.volumes.delete-confirm]
> Destructive volume actions — confirming permanent deletion of a held volume, and deleting a managed, snapshot, or bind site volume — must present a confirmation dialog before the request is issued. The dialog must state the concrete consequence: that held volume deletion removes the underlying data permanently, that managed site volume deletion places the data in the held state awaiting operator confirmation, that snapshot site volume deletion permanently removes the snapshot's data while leaving the source volume untouched, and that bind site volume deletion only drops the reference and leaves the host path untouched.

> w[routes.volumes.held-count]
> The navbar's held-volumes badge must reflect the current count of held volumes without requiring a page reload, both when new held volumes are created and when the operator confirms their deletion.

> w[sessions.events]
> The web interface must emit `WebSessionStarted` and `WebSessionStopped` events on the event feed when a WebTransport session is established or closed. Clients must use these events, together with the OI events `ShellStarted`, `ShellExited`, `ForwardStarted`, and `ForwardStopped`, to keep the connected-clients count up to date without polling.

# Bind Configuration

> w[bind]
> The web interface may be configured to listen on one or more addresses.
> The plain-HTTP listener and the WebTransport listener share the same set of configured addresses but use independent ports.
> When no addresses are explicitly configured, both listeners bind to a loopback address only.
> Failure to bind any configured address at startup is a fatal error.
