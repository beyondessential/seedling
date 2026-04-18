# Web interface for Seedling

## Context

Seedling currently has one operator surface: the `seedling-ctl` CLI, which opens a QUIC + mTLS-RPK connection to the daemon's OI (Operator Interface) on `[::1]:7891` and sends newline-delimited JSON requests over bidirectional streams.

We want a second operator surface — a browser-based UI — served by a new binary (`seedling-web`) that sits beside the daemon and proxies the OI. Because Seedling's OI is already a QUIC-streams-of-JSON protocol, the natural browser wire protocol is **WebTransport**: the same streaming abstractions, natively available in the browser, identical wire format. The web binary therefore becomes a thin shim:

```
browser ──WebTransport (H3)──▶ seedling-web ──OI QUIC (RPK)──▶ seedlingd
           │                        │                              │
 Svelte SPA over HTTPS              │                              │
                                    ├── own ClientIdentity         └── authorised_keys
                                    ├── session-cookie auth
                                    ├── tailscale header extractor
                                    └── injects actor into each OI request
```

Three cross-cutting concerns:

1. **Subclient context / actor identity.** The daemon currently only knows callers by their SPKI fingerprint (logged, not threaded into handlers/events/audit). A web binary fronting many humans behind one QUIC identity must tell the daemon *who* initiated each call.
2. **Listening on non-loopback interfaces.** Today the daemon is hardcoded to `[::1]:7891`. We want both binaries to bind configurable interfaces (`lo`, `tailscale0`) or explicit addresses, loopback-only by default.
3. **Authentication.** Tailscale identity headers when present (gated on an explicit `--trust-tailscale-headers` flag); shared Argon2 password + signed session cookie otherwise. Cookies flow on the WebTransport handshake because it's an HTTP/3 request.

Phase 1 scope: list apps, register new, view/update script, show status, install/actions. Event/log/shell/forward streaming deferred.

## Decisions locked in

- Frontend: **React + TypeScript + Vite**, with **MUI (Material UI)** for components and **React Router v7** for routing. Served as a static SPA.
- Theme: green/grass palette — MUI custom theme with a green primary and earthy secondary.
- Wire protocol: **WebTransport**, carrying the existing OI JSON wire format 1:1 (web binary injects the `actor` field and proxies streams).
- Fallback auth: Argon2id shared password + signed session cookie. Session cookies authenticate the WebTransport handshake request.
- TLS cert strategy: see "TLS & cert-hash pinning" below.
- Listen: `--interface NAME[,NAME…]` and `--listen ADDR:PORT` for both daemon and web binary; loopback-only defaults.
- No streaming endpoints in phase 1 (but the WT plumbing naturally supports them when we add them).

## Architecture

### Web binary = WebTransport ↔ OI shim + static asset server + auth

`seedling-web` runs **two distinct listeners**:

1. **Plain HTTP (TCP)** via axum+hyper: serves the Svelte SPA bundle and `POST /connect`. Designed to sit behind a TLS terminator (Caddy, nginx, Tailscale Serve) in production. On loopback it's served directly and still qualifies as a secure context in browsers so WebTransport works.
2. **HTTP/3 + WebTransport (UDP)** via `wtransport`: always served directly — reverse-proxying WebTransport is not standardised. Uses an ephemeral self-signed ECDSA-P256 cert; the SPA pins it via `serverCertificateHashes` returned by `POST /connect`. No real cert or CA trust needed on this listener.

Production deployment shape:

```
browser ──HTTPS──▶ Caddy ──HTTP──▶ seedling-web:8080  (SPA + /connect)
        ──H3/WT (direct)──────────▶ seedling-web:7893 (ephemeral cert, hash-pinned)
```

Dev / loopback deployment shape:

```
browser ──HTTP (localhost)──▶ seedling-web:8080
        ──H3/WT──────────────▶ seedling-web:7893
```

The cross-port WebTransport connection is fine: cookies set for the hostname are sent port-independently on the WT handshake; WebTransport has no same-origin restriction.

Inside the web binary, a single long-lived `OiClient` (from `seedling-protocol`) connects to the daemon. Each incoming WebTransport bidi stream triggers a new `conn.open_bi()` on the daemon connection; the web binary reads the request JSON from the browser, parses enough to inject `actor`, re-serialises, and splices the remaining bytes bidirectionally. Server-initiated unidirectional streams from the daemon (events, logs) are mirrored back as WT uni streams when we add them in a later phase.

### Wire protocol (browser ↔ web binary)

Identical to OI wire format at `docs/spec/interface.md` §Streams / §Wire Format:

- Control: one client-initiated WT bidi stream per request. Client writes JSON + newline, half-closes write. Server writes response JSON, closes.
- Events (future phase): a server-initiated WT uni stream carrying newline-JSON.
- Logs (future phase): a server-initiated WT uni stream carrying newline-JSON.
- No port forwards / shells over WT in the foreseeable roadmap — those stay CLI-only unless someone asks.

The web binary injects `actor` into the request JSON before forwarding. Responses pass through unchanged.

### Actor context (new, applies to all OI callers)

The OI protocol gains an optional `actor` field on every request:

```json
{ "method": "/apps/create",
  "actor": { "kind": "tailscale"|"password"|"ctl"|"dev", "id": "...", "display": "...", "session": "..." },
  "params": { ... } }
```

- `actor.kind` = auth source type. `id` = stable identifier (email / username / fingerprint). `display` = human label. `session` = opaque correlator.
- Optional. When absent, the daemon synthesises `Actor { kind: "ctl", id: <client_fingerprint_prefix>, display: <authorised_keys label or fingerprint> }` from the mTLS identity. Existing CLI callers keep working unchanged.
- The resolved actor rides through `handler::Ctx` and is recorded on every `OiEvent` variant + in the JSONL audit log.

### TLS & cert-hash pinning (WT listener only)

The SPA's HTTPS origin is the reverse proxy's problem: in prod the operator points Caddy / Tailscale Serve / nginx at the plain-HTTP listener and that front-end terminates TLS with whatever cert they already manage. On loopback there's no HTTPS at all — browsers treat `http://localhost` as a secure context so WebTransport is still allowed.

The WebTransport listener owns its own cert lifecycle, independent of anything external:

- `wtransport` serves H3 with a **self-signed ECDSA-P256 cert** generated at startup.
- The cert is constrained to the WebTransport `serverCertificateHashes` rules: ECDSA secp256r1, SHA-256, validity ≤ 14 days.
- Browsers accept the cert because the SPA pins it by SHA-256 in the WT constructor, not by CA trust.
- Rotation: the web binary keeps two overlapping certs (`current` and `next`). At T-24h before `current` expires, generate `next`; both hashes are included in subsequent `POST /connect` responses. Swap the WT listener to `next` when `current` expires. A browser with an in-flight session re-calls `POST /connect` on reconnect and picks up the new hash automatically.

### Listen on interfaces (shared daemon + web)

Both binaries accept:

```
--interface NAME[,NAME…]     # resolved via if-addrs; all v4+v6 addrs of each interface
--listen ADDR:PORT           # may be repeated; explicit bind addresses
```

For the web binary, the HTTP and WebTransport listeners have independent port settings (`--http-port` default 8080, `--wt-port` default 7893). Both bind on every resolved address from `--interface`/`--listen`. Defaults: daemon `[::1]:7891`; web binary HTTP `[::1]:8080`, WT `[::1]:7893`. Without `--interface` or `--listen`, all three bind loopback only. Failure to resolve a named interface at startup is fatal.

## New crate: `crates/web` → `seedling-web` binary

Workspace member alongside `core`/`ctl`/`daemon`/`protocol`. Depends on `seedling-protocol`. New deps:

- `axum` — plain-HTTP surface (`/connect`, static SPA serve).
- `wtransport` — WebTransport server, quinn-based (matches our existing quinn 0.11 pin).
- `rcgen` — ephemeral ECDSA cert generation for the WT listener.
- `tower-http` — static file serving, compression.
- `argon2` — password hashing.
- `if-addrs` — interface-name resolution.
- `rust-embed` (or similar) — embed the built SPA bundle.

Crate layout:

```
crates/web/
  Cargo.toml
  build.rs                  # runs `pnpm --filter ./frontend build`; checks dist exists
  src/
    main.rs                 # CLI args, wiring, bind/listen
    config.rs               # TOML: [auth] password_hash, session_secret
    wt_cert.rs              # generate ephemeral ECDSA cert, rotation task, publish hashes
    auth.rs                 # /connect logic: check all credential sources in order
    auth/
      password.rs           # Argon2 verify, long-lived token generation/verify
      tailscale.rs          # Tailscale-User-Login/Name parsing
    actor.rs                # Actor struct (re-exported from protocol)
    http.rs                 # axum router: /connect, /healthz, static SPA
    wt.rs                   # WebTransport server: accept sessions, proxy streams
    proxy.rs                # per-stream proxy: parse JSON head, inject actor, splice
    interfaces.rs           # --interface resolution
  frontend/
    package.json, tsconfig.json, vite.config.ts
    src/
      main.tsx
      App.tsx
      lib/
        wt.ts               # WebTransport client: OI wire format
        session.ts          # POST /connect, cert-hash handling, reconnect
        api.ts              # typed wrappers over wt.ts for each OI method
        types.ts            # OiRequest / OiResponse / OiEvent mirrors
      hooks/
        useSession.ts       # connection state, actor, reconnect logic
        useOi.ts            # typed OI request hook
      routes/
        Apps.tsx            # list + register new
        AppDetail.tsx       # status, resources, faults, params, actions, script editor
        Login.tsx
      components/           # shared MUI-based components
      theme.ts              # MUI green/grass palette (primary: green, secondary: earthy brown/amber)
    dist/                   # build output (gitignored)
```

### OiClient wrapper for the web binary

A thin wrapper over `seedling_protocol::client::OiClient` that owns a single long-lived connection to the daemon and exposes:

- `open_proxy_stream() -> (SendStream, RecvStream)` — opens a daemon bi-stream, returns its halves so `proxy.rs` can splice.
- `request_with_actor(method, params, actor) -> Result<Value>` — convenience for the few non-proxied requests the web binary might issue itself (health, server status).

### Plain-HTTP routes (axum)

| Method & path | Auth | Purpose |
|---|---|---|
| `GET /` | none | Serves `index.html` from the embedded SPA bundle. |
| `GET /assets/*` | none | SPA bundle static assets (hashed filenames, cache-forever). |
| `GET /healthz` | none | Liveness. |
| `POST /connect` | none | Auth and WT connection endpoint — see below. |

#### `POST /connect`

The SPA calls this on startup and on every WT reconnect. Request body is a JSON object with zero or one credential field:

```json
{}                          // Tailscale or dev mode; no credential needed
{ "token": "<bearer>" }    // reconnect: present the previously issued token
{ "password": "<pw>" }     // first login when no token is held
```

The server checks credentials in order: Tailscale identity headers (if `--trust-tailscale-headers`), dev bypass (if `--dev-no-auth` on loopback), Bearer token, password.

**Success (200):**
```json
{
  "token": "<long-lived-bearer>",
  "actor": { "kind": "password"|"tailscale"|"dev", "id": "...", "display": "..." },
  "wt_url": "https://<host>:<wt_port>/wt?t=<single-use-token>",
  "cert_hashes": ["<sha256>", "<sha256-next>"]
}
```

**Auth required (401):**
```json
{ "auth_required": "password" }
```

The SPA stores `token` and `actor` in memory after the first successful response. On reconnect it sends the stored `token`; the server re-validates and issues a fresh `wt_url`. On 401 the SPA shows the login form.

### WebTransport handshake auth

The WT handshake URL includes a single-use token issued by `POST /connect` as a query param. The web binary validates the token from the URL on the handshake, builds `Actor` from the associated session, stores it in the per-session state. Rejected with 401 if the token is missing, invalid, or already used. No per-stream re-auth.

Tokens are short-lived (e.g. 30 seconds) and single-use — they exist solely to bridge the gap between `POST /connect` and the WT handshake, so there is no value in persisting them.

### Actor injection on proxied streams

`proxy.rs` reads bytes from the browser's WT stream into a small buffer until the first `\n`, JSON-parses that line, merges in `actor: <session actor>`, re-serialises as a newline-terminated line, writes to the daemon stream, then byte-splices the remainder of the stream in both directions until close. Phase-1 requests are all single-JSON-line control requests, so only the first line is rewritten.

## Daemon changes

### Bind configuration

Replace fixed port in `crates/daemon/src/main.rs` with:

```rust
#[arg(long)]  interface: Vec<String>,
#[arg(long)]  listen: Vec<SocketAddr>,
#[arg(long, default_value_t = 7891)]  port: u16,     // used when only --interface is given
```

Default to `[::1]:7891` when both are empty. `crates/core/src/oi/server.rs:70-131` (`run`) is modified to accept `addrs: &[SocketAddr]`, open one `quinn::Endpoint` per address sharing the same `ServerConfig` and `Semaphore`.

### Actor plumbing

- `crates/protocol/src/actor.rs` — new module with `Actor { kind, id, display, session }` (all string fields, all optional).
- `crates/protocol/src/client.rs` — `OiClient::request` grows an optional `actor` parameter (ergonomic default = `None`; ctl passes `None`).
- `crates/core/src/oi/server.rs:228` (`handle_bidi_stream`) — on reading the first JSON line, extract `actor` field, fall back to synthesising from fingerprint/label, stash on a `RequestCtx` passed through dispatch.
- `crates/core/src/oi/handler.rs` — `dispatch` and all handler signatures take `&RequestCtx`. Large mechanical change.
- `crates/protocol/src/events.rs` — every `OiEvent` variant gains `actor: Option<Actor>`; every emit helper gains an `actor` parameter.
- Every handler call site that emits an event — thread the ctx's actor through. Identifiable via grep on `app_registered`, `operation_started`, etc.
- `crates/core/src/runtime/audit.rs` — unchanged structurally; actor rides through serde.

Existing CLI behaviour is preserved: no actor in request → synthesised from fingerprint → identical to today's logging apart from the new `actor` field on events.

## Spec changes

Per `AGENTS.md`: spec first, then implement, then test.

1. **`docs/spec/interface.md`** — additions:
   - `i[wire.actor]`: optional `actor` field, shape, synthesis rule.
   - `i[transport.listen]`: server may bind multiple addresses; loopback-only default.
   - Update `i[event.types]`: every event carries `actor`.
2. **`docs/spec/web.md`** (new, prefix letter `w`):
   - `w[transport.webtransport]`: browser wire transport is WebTransport; wire format is identical to the OI wire format at `i[wire.*]`; actor is injected by the web binary.
   - `w[transport.http]`: a companion plain-HTTP surface serves the SPA bundle and the `POST /connect` endpoint. Expected to sit behind a TLS terminator for non-loopback deployments.
   - `w[auth.tailscale]`: Tailscale identity header extraction; only trusted when the operator explicitly opted in.
   - `w[auth.connect]`: `POST /connect` is the single auth + bootstrap endpoint. It accepts a credential (password, Bearer token, or nothing for Tailscale/dev), returns either a success with `token`, `actor`, `wt_url`, and `cert_hashes`, or a 401 with `auth_required` naming the credential type to collect. The SPA calls this on startup and on every WT reconnect.
   - `w[auth.password]`: password credential is validated with Argon2id; on success a long-lived Bearer token is issued and returned in the `/connect` response.
   - `w[auth.wt-token]`: `wt_url` from a successful `/connect` embeds a short-lived single-use token; the WT handshake is rejected if the token is missing, expired, or already consumed.
   - `w[spa.delivery]`: SPA is served from a secure-context origin (loopback, or a TLS-terminating front-end).
   - `w[tls.hashes]`: WebTransport listener serves a self-signed ECDSA-P256 cert pinned by SHA-256 via `serverCertificateHashes`. Validity ≤ 14 days. Rotation uses a two-cert overlap window.
   - `w[routes.phase-one]`: enumerate the phase-1 route surface and map each to the OI method it invokes.
3. **`.config/tracey/config.styx`** — register `web` spec scanning `crates/web/src/**/*.rs`.

Spec items describe **what** the system requires, not **how**. Keep implementation specifics (`wtransport`, `axum`, `rcgen`, flag names) out of spec text.

## Rollout phases

**Phase 0 — specs.** Write interface additions + new `docs/spec/web.md`. Register `web` tracey spec.

**Phase 1a — actor plumbing in protocol/daemon.** `Actor` type, optional on request, server-side synthesis, threaded through handlers, present on all events and audit records. CLI unchanged.

**Phase 1b — multi-address bind for daemon.** `--interface` / `--listen` flags on `seedlingd`; multi-endpoint spawn. Default preserved.

**Phase 2 — web binary skeleton.**
- Crate created; depends on `seedling-protocol`.
- `main.rs` wiring; CLI args; config parse.
- Ephemeral WT cert generator + rotation task + hash publishing.
- Axum plain HTTP: `/healthz`, `POST /connect`, static SPA.
- `/connect` handler: credential checking in order, token issuance, single-use WT token generation.
- Parse `Tailscale-User-Login`/`Tailscale-User-Name` when the trust flag is set.
- WebTransport server accepting sessions, authenticating handshake via single-use token in URL.
- Opening a WT stream returns `not_implemented` until phase 4.

**Phase 3 — SPA skeleton.**
- `frontend/` React + TypeScript + Vite project with MUI.
- `lib/wt.ts`: WebTransport client mirroring OI wire format.
- `lib/session.ts`: `POST /connect`, open WebTransport with returned `wt_url` + `cert_hashes`, reconnect on drop.
- Login page; protected route wrapper via React Router v7.
- MUI theme with green/grass palette (`theme.ts`).
- `build.rs` runs Vite build; bundle embedded via `rust-embed`.

**Phase 4 — WebTransport stream proxy.** `proxy.rs`: actor injection on first JSON line; bidirectional splice for remaining bytes. Plumb through to the daemon OiClient. End-to-end: browser → web binary → daemon → JSON response.

**Phase 5 — Phase-1 UI features.** Svelte routes + API wrappers for:
- App list (`/apps/list`).
- App detail — status, resources, faults, params, install requirements, actions, script (`/apps/show` + `/apps/script`).
- Register new app (`/apps/create`).
- Update script (`/apps/update`).
- Invoke install (`/apps/install/invoke`) with dynamic form from `install_requirements`.
- Invoke action (`/apps/action/invoke`) with typed-param form.

Future (out of scope for this plan):
- Event feed + log tail over WT uni streams.
- Shells via WT + xterm.js.
- Tailscale service app grants.
- OIDC as an alternative fallback.

## Critical files

Modify:
- `Cargo.toml` (root) — add `crates/web`; add `axum`, `wtransport`, `rcgen`, `argon2`, `tower-http`, `if-addrs`, `rust-embed` to workspace deps.
- `crates/daemon/src/main.rs` — `--interface` / `--listen` / `--port`; replace fixed-port wiring at line ~627.
- `crates/core/src/oi/server.rs:70-131` and `handle_bidi_stream` at `:228` — multi-address endpoints; actor extraction from request.
- `crates/core/src/oi/handler.rs` — ctx-threaded dispatch.
- `crates/protocol/src/client.rs` — optional `actor` on `request`.
- `crates/protocol/src/events.rs` — `actor` field on every variant + helpers.
- Every event-emit site across `crates/core/src/oi/handler/*.rs` and `crates/core/src/runtime/*`.
- `docs/spec/interface.md` — wire.actor, transport.listen, event.types update.
- `.config/tracey/config.styx` — register `web` spec.

Create:
- `crates/protocol/src/actor.rs` — `Actor` struct.
- `crates/web/` — full crate (Rust + frontend).
- `docs/spec/web.md`.

## Verification

**Build & lint:** `cargo clippy --workspace --all-targets` clean; `cargo fmt`. Frontend: `pnpm --filter ./frontend check` (svelte-check + tsc) clean.

**Unit:**
- Argon2 login success/failure.
- Token issuance and verification; tampered/expired token rejected.
- Actor JSON round-trip (request with and without actor).
- Server-side actor synthesis from fingerprint.
- Cert rotation: two-cert overlap; hash set includes both during overlap window.
- Interface resolution: `--interface lo` → loopback v4+v6; unknown interface → fatal error.

**Integration (daemon-only):**
- Start daemon with `--listen 127.0.0.1:7891 --listen [::1]:7891`; CLI connects on either.
- Start daemon with `--interface lo`; CLI connects; server logs show multiple endpoints.
- CLI without actor → audit log shows synthesised actor (`kind: "ctl"`, `id: <fingerprint-prefix>`, `display: <label>`).

**End-to-end (web):**
- Authorise web binary's fingerprint.
- Start `seedling-web --interface lo --config web.toml`.
- In Chrome, visit `http://localhost:8080/`. SPA does `POST /connect {}` → 401 `{ auth_required: "password" }`; shows login form.
- User enters password; SPA does `POST /connect { password }` → 200 with `token`, `actor`, `wt_url`, `cert_hashes`; stores token + actor in memory, opens WebTransport.
- Register `test.seed.rhai` via the SPA form; confirm it appears in CLI `apps list` and the audit log carries `actor.kind="password"`, `actor.display="admin"`.
- With `--dev-no-auth` on loopback: no login prompt; audit shows `actor.kind="dev"`.
- With `--dev-no-auth` on a non-loopback bind: process refuses to start.
- Force cert rotation with a short-lived cert; confirm the SPA reconnects transparently.

**Tracey:**
- `tracey query status` clean.
- `tracey query uncovered --spec-impl interface/main` and `web/main` show zero uncovered on newly added items.
