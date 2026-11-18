# Terminate DTLS on a DTLS ingress, and share ports by hostname

TLS termination on a `Terminate.Tls` ingress already landed on D6: the L4 route carries caddy-l4's `tls` handler ahead of `proxy`, and `layer4.handlers.tls` is a required module.
What remains is DTLS, whose datagrams reach the Service still encrypted, and the claim model that lets L4 and HTTPS ingresses share a port by hostname.

## Decisions

- DTLS is terminated in the proxy, by Caddy module code written in this repository on pion/dtls v4 (`github.com/pion/dtls/v4`; v3 is DTLS 1.2 only), built into the seedling-caddy image. caddy-l4 has no DTLS module.
- The module is a Caddy app (`seedling_dtls`) owning its UDP sockets, not a layer4 handler. caddy-l4 splits a UDP listener into one flow per remote address before any handler runs, with a 30 second idle default, so a handler inside it could neither follow a connection-ID session to a new address nor keep the idle period the spec asks for. The module ID is ours, not `layer4.handlers.dtls`, so an upstream module of that name can never collide with it.
- pion/dtls serves DTLS 1.2 and 1.3, connection IDs, and the return routability check.
- The certificate comes from the proxy's TLS app, looked up by the ingress's hostname rather than the client's SNI, so runtime-managed certificates (through `get_certificate`) and proxy-held ones both apply with no new certificate path. Approach: build a default `caddytls.ConnectionPolicy`, take its `tls.Config`, and call its `GetCertificate` with the ingress hostname as the server name, adapted to pion's hello type.
- A claim is `(port, transport, Option<hostname>)`. Claims of one kind (TLS on TCP, HTTP/3 on UDP, plaintext HTTP on TCP) for different hostnames share a port; anything else conflicts. Pass-through and DTLS claim the whole port. DTLS SNI sharing can be added if a use case appears.
- A TLS handshake with no SNI, or naming no ingress on the port, is refused.
- A conflicting app ingress is left out whole, not per transport. Two conflicting ingresses in one app fail evaluation.
- One upstream UDP flow per session. Sessions are capped by the proxy's source ports toward one Service; that cap is shared with every L4 ingress already and sits far above one site's needs, so the proxy stays single-address. A wider port range in the proxy's namespace, or more proxy source addresses, are the levers if a host ever needs more.
- The idle period is fixed (five minutes), not declared in the BSL; a per-ingress setting can be added later.
- Sessions survive a config apply that leaves their ingress as it was; an upgrade may end them.
- A refused session is logged and holds a `dtls_session_refused` condition fault. The runtime learns of refusals from an admin API route the module registers, polled each iteration like the health query.
- Whether the Service sees the client's address is L6 (outbound), alongside A2 (inbound).
- The DTLS session rules live in PRX ("DTLS-terminating ingresses"); ING keeps the termination, version and certificate rules.

## Rejected

- Terminating DTLS in the daemon: proxy-held certificates would have to be read out of the proxy's storage, and the Rust DTLS libraries are thinner.
- Removing `Terminate.Dtls` from the language until a terminator exists.

## Cross-repo assumption

The ad-hoc hosts' Caddy, built by beyondessential/third-party-builds, is checked against ours for version only. It will not carry `seedling_dtls`, which matters only if those hosts are ever given a Seedling-emitted config.

## Build

### 1. Claim model (ING#identity-within-an-app, PRX#shared-ports)

- [x] `crates/core/src/defs/ingress.rs`: a `Claim { port, transport, scope }` type with `scope` either the whole port or a hostname of a kind (TLS, HTTP/3, plaintext HTTP), a `claims(&IngressDef)` returning one claim per transport the ingress serves (its redirect's plaintext claim included), and `conflicts(&Claim, &Claim)`
- [x] Fail evaluation when two of an app's ingresses make conflicting claims, alongside the existing redirect-termination check, with an error naming both ingresses
- [x] Map site ingress attachments onto the same `Claim`: `tcp` and `udp` take the whole port, `http`/`http2` take TLS and HTTP/3 claims, or a plaintext claim when the mode is `none`
- [x] Tests in `crates/core/src/tests/ingress.rs`: TLS and HTTPS on one port with different hostnames evaluate; pass-through beside anything on its TCP port, DTLS beside HTTPS on 443, and two DTLS ingresses on one port each fail naming both

### 2. Conflict resolution (ING#claims-across-apps, SITE#ingress-conflicts, FLT#ingress-and-service-faults)

- [x] `reconcile/proxy.rs::collect` hands L4 app ingresses back as claimants carrying their app, not as finished `L4Route`s, so they take part in conflict resolution; today they bypass it entirely
- [x] `site_proxy.rs`: rekey `resolve_app_claims`, `resolve_site_claims` and `detect_conflicts` from `(hostname, port)` to pairwise claim conflicts, keeping install order across apps, creation order across site ingresses, and both-out for app against site; fold the redirect-versus-plaintext special case into the general rule
- [x] Rename the `drop_conflicting_*` helpers to work from the losing claimants, dropping an app ingress whole and an app ingress's redirect alone when only the redirect's claim lost, as today
- [x] `ConflictParties` carries port, transport and each side's hostname where it has one; `reconcile_ingress_conflicts` writes them into the fault description
- [x] `reconcile/rules.rs`: ingress rules are derived from the final proxy configuration, so only served claims are forwarded (INFRA#ingress-redirection updated)
- [x] Tests in `site_proxy.rs`: rewrite the `(hostname, port)` cases against claims; add TLS-beside-HTTPS sharing, pass-through-beside-HTTPS conflict, DTLS-beside-HTTP/3 conflict, app L4 ingress against a site `tcp` attachment, plaintext-beside-TLS on one TCP port

### 3. Proxy config types and persisted-config compatibility (PRX#proxy-upgrades)

- [x] `system/types.rs`: give `L4Route` a `hostname: Option<String>` for SNI-routed TLS, and add `ProxyConfig::dtls_routes: Vec<DtlsRoute>` (`port`, `hostname`, `upstream`, `ingress` id), `#[serde(default)]` on both
- [x] Move DTLS ingresses out of `l4_routes` into `dtls_routes` in `reconcile/proxy.rs`
- [x] `startup.rs`: no backfill, since a cached UDP `L4Route` cannot be told from a pass-through one and replaying it as it was is what was running; a cached TLS route without a hostname renders as it did, and a cached-config test covers the old shape
- [x] Replace `conflicting_listener_ports` with a backstop over every server the rendered config binds (HTTP, HTTPS, QUIC, layer4, `seedling_dtls`), so a claim-model bug fails as `proxy_apply_failed` rather than as a whole-config rejection from Caddy

### 4. TLS on TCP shared by SNI (PRX#shared-ports)

- [x] `caddy/config.rs`: key layer4 servers by port and transport; on a port with SNI-routed TLS claims, emit one route per hostname matched by `tls` `sni`, each `tls` then `proxy`
- [x] Where an HTTPS server shares the port, emit no layer4 server; put the `caddy.listeners.layer4` wrapper on the HTTPS server with the `Terminate.Tls` routes, falling through to HTTP
- [x] Refuse a TLS handshake with no SNI or an unknown one: no catch-all route on a layer4 TLS port; confirm the HTTPS server does the same for a name it has no vhost for, or add a closing route
- [x] Pin the TLS versions on the L4 `tls` handler and the HTTPS server to 1.2 and 1.3 in emitted config, rather than relying on the proxy's default
- [x] Add `caddy.listeners.layer4` and `layer4.matchers.tls` to `docker/caddy/required-modules.txt`
- [x] Tests in `caddy/tests.rs`: two TLS ingresses on one port, TLS beside HTTPS on 443, a lone TLS ingress, version pins present

### 5. The `seedling_dtls` Caddy app (PRX#dtls-terminating-ingresses, ING#termination)

- [x] `docker/caddy/dtls/`: a Go module (`go.mod`, `go.sum`) registering app `seedling_dtls`, its config one server per port: listen port, ingress hostname, upstream, ingress id; pin pion/dtls with `go get` to an exact version
- [x] Listen through Caddy's shared packet listener so a reload reuses the socket; keep sessions in a `caddy.UsagePool` keyed by port, so a reprovisioned app adopts the sessions of a server whose hostname and upstream are unchanged and closes the rest
- [x] Handshake with cookie exchange required, DTLS 1.2 and 1.3, connection IDs on, return routability check on address change
- [x] Certificate by the ingress hostname from the TLS app, whatever the client's SNI
- [x] On handshake completion, dial a connected UDP socket to the upstream per session; on dial failure refuse the session, log at warn naming the ingress, and record the refusal time against the ingress id
- [x] Copy datagrams both ways; drop non-DTLS datagrams without reply; end a session on close_notify or after five idle minutes, closing its upstream socket
- [x] Admin API route (`/seedling/dtls/refusals`) returning, per ingress id, the time of the last refusal
- [x] Go tests with a pion client: 1.2 and 1.3 handshakes; certificate served for a client sending no SNI and for one sending another name; a spoofed-source handshake gets no flow; two sessions get distinct upstream source ports and replies route back; idle expiry and close_notify close the upstream socket; a CID session survives an address change after the return routability check; a reload with the server unchanged keeps the session, with the upstream changed ends it; refusal recorded when the dial fails

### 6. Emitting the app (PRX#proxy-image)

- [x] `caddy/config.rs`: render `dtls_routes` into `apps.seedling_dtls`
- [x] `caddy/image.rs`: `referenced_modules` counts `seedling_dtls` as an app id, as for `layer4`; `everything_on()` gains a DTLS route and an SNI-shared TLS port
- [x] `docker/caddy/Containerfile`: copy the module into the builder and `--with github.com/beyondessential/seedling/docker/caddy/dtls=./dtls`; add `seedling_dtls` to `required-modules.txt`; bump the serial in `.github/workflows/caddy-image.yml` and `CADDY_IMAGE` in `startup.rs`
- [x] `.github/workflows/caddy-image.yml`: widen the tag-moved check from the Containerfile to `docker/caddy/` minus `required-modules.txt`, so a module change also moves the tag; add a `go test` and `go vet` job for the module

### 7. Refusal fault (FLT#ingress-and-service-faults, PRX#dtls-terminating-ingresses)

- [x] `faults` registry: `dtls_session_refused`, condition, keyed `(app, kind, ingress)`
- [x] `caddy/proxy.rs`: read the refusals route each iteration; an unreadable route files nothing and clears nothing
- [x] `reconcile/faults.rs`: file while a refusal is within the last five minutes, clear after five quiet minutes or when the ingress leaves the definition, through `sync_faults`, and log at warn when filing
- [x] Tests: filed on a recent refusal, cleared after the quiet period, untouched when the route cannot be read, cleared when the ingress is removed

### 8. Docs and comments

- [x] `docs/networking.md`: DTLS termination, port sharing by SNI, and the claim conflicts
- [x] `docs/bsl-scripting.md`: a DTLS ingress holds its UDP port alone
- [x] Rewrite the `reconcile/proxy.rs` comment that says DTLS datagrams reach the Service as they arrive
