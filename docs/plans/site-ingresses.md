# Site ingresses (incl. Tailscale)

## Context

Today, ingresses are declared inside apps in BSL: `service.ingress(hostname, port).tls(...)`.
The hostname, port, TLS termination, and any HTTP→HTTPS redirect all live in the app
script. There is no operator-side mechanism to:

- attach an additional entry point to an existing app service (e.g. expose an internal
  app on the host's Tailscale hostname without touching the app script);
- migrate an app from one URL to another (the old hostname must redirect to the new
  one, but apps shouldn't have to know about the old name);
- consume an externally-supplied ingress — most importantly, the host's Tailscale
  FQDN, which we can discover via tailscaled.

Today's workaround would be to bake every operator preference into BSL (cf. the
`TAILSCALE_HOSTNAME` BSL constant suggested in `TODO.txt`), which pushes operator
concerns into apps and forces every app to know about every entry point.

This change introduces **site ingresses**: operator-managed, named entry points that
live outside any app. They mirror the **site service** / **site volume** model already
in the codebase: defined by the operator (or discovered by the daemon), persisted in
the daemon DB, exposed through CLI / OI / web UI / spec, and bound to apps via a
separate attachment table. A site ingress can:

1. **Forward** `(port, protocol)` traffic to an app service — operator picks which app
   gets the hostname.
2. **Redirect** `(port, protocol)` traffic to a fixed URL (transitioning
   `old.example.com` → `https://new.example.com`).
3. **Be discovered** — the Tailscale provider auto-creates a site ingress for the
   host's `<host>.<tailnet>.ts.net` hostname, with TLS supplied by tailscaled.

### Decisions

- **Tailscale**: auto-create discovered site ingress entries (operator can attach apps
  to them but cannot delete them while Tailscale is configured).
- **Targets**: separate `site_ingress_attachments` table — the site ingress carries
  hostname/TLS only, attachments carry `(port, protocol, target)`.
- **No front pre-enumeration**: a site ingress does **not** declare which ports it
  exposes. For a `*.ts.net` host that's "any port"; concrete fronts come into existence
  when an attachment is created.
- **Conflicts on `(hostname, port)`**: if an app ingress and a site-ingress attachment
  both claim the same `(hostname, port)`, both are dropped from proxy config and both
  sides receive a fault.
- **Multi-front per site ingress**: yes, expressed as multiple attachments on the same
  `site_ingress`, each with its own `(port, protocol)`.

## Data model

### Tables (new migration `v49.sql`)

```sql
-- r[ingress.site] Operator-managed named entry points, independent of any app.
CREATE TABLE site_ingresses (
    name             TEXT PRIMARY KEY,
    hostname         TEXT NOT NULL,
    description      TEXT,
    source           TEXT NOT NULL,             -- 'manual' | 'discovered'
    discovered_key   TEXT,                      -- non-NULL iff source='discovered'
                                                --   (stable provider key, e.g. Tailscale node id)
    discovered_provider TEXT,                   -- 'tailscale' | ... ; matches source='discovered'
    tls_provider     TEXT NOT NULL,             -- 'acme' | 'tailscale' | 'internal' | 'none'
    stale            INTEGER NOT NULL DEFAULT 0,-- discovery temporarily lost source (e.g. tailscaled down)
    created_at       TEXT NOT NULL,
    UNIQUE (discovered_provider, discovered_key)
);

-- r[ingress.site.attachment]
CREATE TABLE site_ingress_attachments (
    site_ingress     TEXT    NOT NULL REFERENCES site_ingresses(name) ON DELETE CASCADE,
    port             INTEGER NOT NULL,
    protocol         TEXT    NOT NULL,          -- 'tcp' | 'udp' | 'http' | 'https'
    target_kind      TEXT    NOT NULL,          -- 'forward' | 'redirect'
    target_app       TEXT,                      -- forward: app name
    target_service   TEXT,                      -- forward: app service name
    redirect_url     TEXT,                      -- redirect: target URL (scheme://host[:port])
    redirect_code    INTEGER,                   -- redirect: 301 | 302 | 307 | 308
    redirect_preserve_path INTEGER,             -- redirect: 0 | 1
    created_at       TEXT    NOT NULL,
    PRIMARY KEY (site_ingress, port, protocol)
);
```

Schema notes:

- `tls_provider='tailscale'` is only legal when the row's `source='discovered'` and
  `discovered_provider='tailscale'`.
- A discovered ingress always has the `name` chosen by the provider (e.g. `tailscale`)
  and is rejected if an operator tries to delete it. The daemon owns the row's
  lifecycle via `discovered_key`.
- Attachments specify `(port, protocol)` only; the hostname is inherited from the
  parent site ingress.

### Rust types

New files:

- `crates/core/src/runtime/site_ingresses.rs` — `SiteIngressDef`, `SiteIngressSource`,
  `TlsProvider`, CRUD functions (`create`, `list`, `get`, `update_hostname`,
  `mark_stale`, `delete_manual`, `delete_discovered_by_key`).
- `crates/core/src/runtime/site_ingress_attachments.rs` —
  `SiteIngressAttachment { site_ingress, port, protocol, target: AttachmentTarget }`
  with `AttachmentTarget::Forward { app, service }` and
  `AttachmentTarget::Redirect { url, code, preserve_path }`. CRUD `attach`, `detach`,
  `list_for_ingress`, `list_all`, `update_target`.
- `crates/protocol/src/names.rs` — add `SiteIngressName` newtype (parallel to
  `SiteServiceName`).

## Tailscale provider

New module: `crates/core/src/runtime/tailscale.rs`.

```rust
pub struct TailscaleProvider { db: DbHandle, kick: mpsc::Sender<()> }
struct DiscoveredIdentity { hostname: String, tailnet: String, node_id: String }

impl TailscaleProvider {
    pub fn spawn(db: DbHandle) -> Arc<Self>;       // tokio task, 60s tick + kick channel
    pub fn refresh_now(&self);                     // OI handler entry point
    async fn poll_once(&self) -> Result<Option<DiscoveredIdentity>, TailscaleError>;
    fn reconcile_db(&self, identity: Option<DiscoveredIdentity>);
}
```

Transport: `hyper` + `hyperlocal` to `/var/run/tailscale/tailscaled.sock`. Use
`/localapi/v0/status` for identity and `/localapi/v0/cert/<host>?type=pair` for certs
(see TLS section). Same client serves both.

Identity keying: stable Tailscale node id (`Self.ID` from `/status`), stored as
`discovered_key`. Hostname rename = `UPDATE site_ingresses SET hostname=?` keyed by
`discovered_key` — attachments stay bound. Node-id change = old row deleted (cascading
attachments) and new row inserted.

Failure modes:

- Socket missing / `ECONNREFUSED`: log at debug, skip cycle. Tailscale not installed
  is a configuration choice, not a fault.
- `BackendState != "Running"` (logged out): file `tailscale_not_logged_in` against
  `_system`, mark existing discovered row `stale=1`. Don't delete — its attachments
  are still meaningful when the operator logs back in.
- Transient API error: log at warn; file `tailscale_unreachable` only after N=5
  consecutive failures (mirror the `warm_cert_first_seen` pattern).
- Multiple identities: not exposed by tailscaled in v1; if observed, file
  `tailscale_multiple_identities` and pick `Self`.

OI delete handler rejects rows with `source='discovered'`; the only path that removes
them is `TailscaleProvider::reconcile_db`.

## TLS for site ingresses

Reuse the existing `Coordinator` (`crates/core/src/runtime/tls/issuance.rs`) by adding
a new origin/policy variant rather than a parallel subsystem. Two systems doing
on-demand cert resolution would invite drift; the existing `serve.rs:lookup` already
keys by hostname against the `tls_certificates` table.

Changes in `crates/core/src/runtime/tls.rs` and `tls/state.rs`:

- Add `TlsCertOrigin::Tailscale` to the existing `TlsCertOrigin` enum.
- Add `TlsPolicy::Tailscale` variant (no DNS provider; presence of the variant tells
  the coordinator to dispatch to the Tailscale issuer).

New file `crates/core/src/runtime/tls/tailscale_issuer.rs`:

```rust
pub async fn issue(db: &DbHandle, hostname: &Hostname) -> Result<Issued, IssueError>;
```

Calls `/localapi/v0/cert/<hostname>?type=pair`, parses the concatenated PEM (cert
chain + private key), persists via `store::insert_certificate` with `origin=Tailscale`.
The serve path is unchanged — Caddy gets the cert from the same `get_certificate`
endpoint.

In `Coordinator::run`, after `compute_state` resolves the policy, branch:

- `TlsPolicy::AcmeDns` → existing `acme::issue`
- `TlsPolicy::Tailscale` → `tailscale_issuer::issue`

Renewal cadence (`Decision::Scheduled`'s threshold) is unchanged. Tailscale issues
~90-day certs; the existing renewal fraction works as-is.

Caddy automation policy: site-ingress hostnames flow through the **same**
`get_certificate` chain in `system/caddy/config.rs:129`. No new policy bucket. The
daemon answers from `tls_certificates` regardless of origin.

Failure mode for Tailscale certs: keep last-good cert in DB; file `cert_issue_failed`
with `origin=tailscale`. **Do not** fall back to internal CA — that would silently
break clients that pin the public-PKI chain via Tailscale's MagicDNS expectations. If
the expired-cert sweep retires the row before tailscaled comes back, the handshake
fails visibly, which is the correct behaviour.

## Reconcile integration

### Collection

New file `crates/core/src/system/reconcile/site_proxy.rs`:

```rust
pub struct SiteProxyEntry { pub def: IngressDef, pub upstream: ProxyUpstream }
pub fn collect(state: &ReconcileState) -> Vec<SiteProxyEntry>;
```

For each `site_ingress` row:

- For each attachment:
  - Synthesise an `IngressDef` from `(hostname, port, protocol, tls_provider, redirect?)`.
  - Resolve forward target → `ProxyUpstream`. Redirect target → upstream is "redirect".
  - For unresolved forward targets (the named app/service doesn't exist), file
    `site_ingress_target_missing` against the site ingress and skip the entry.

### Conflict detection

Modify `crates/core/src/system/reconcile/phases.rs`'s `compute_proxy_config` (around
line 248) to call a new pure helper:

```rust
fn detect_conflicts(
    app_pairs: &[(AppName, IngressDef, ProxyUpstream)],
    site_entries: &[SiteProxyEntry],
) -> ConflictReport;
```

- Granularity: per `(hostname, port)`. `app=host:443` + `site=host:8080` are
  independent.
- Tie-breaking: when both claim `(hostname, port)`, **drop both** from the proxy
  config.
- Faults:
  - App side: `kind=ingress_conflict` against
    `(app, resource_type=ingress, resource_name=<ingress>)`.
  - Site side: against `_system` with `resource_type=site_ingress, resource_name=<name>`.
  - Description on each fault names the other party and the `(host, port)` tuple.
- Auto-clear: store the prior tick's conflict set on the reconciler scratch (next to
  `prev_states`); diff against the current tick's set; for any `(host, port)` no
  longer in conflict, clear faults on both sides (mirror `clear_cert_fault` in
  `issuance.rs:402`).

Don't abort the tick on conflict — that would punish unrelated apps for an operator
misconfiguration. Just drop the conflicting entries.

### TLS issuance loop

The reconcile TLS-hostname pass already calls `Coordinator::ensure(hostname)` for
every proxy hostname. It will pick up site-ingress hostnames automatically once
they're folded into the proxy config — no extra wiring beyond ensuring the hostname's
`TlsPolicy` is resolved correctly by `state::compute_policy`. Add a branch there:
hostnames whose matching site ingress has `tls_provider='tailscale'` resolve to
`TlsPolicy::Tailscale`.

## OI handler surface

New file `crates/core/src/oi/handler/ingresses.rs`. Methods registered in
`oi/handler.rs`:

- `/ingresses/site/list` → `list_site_ingresses` (returns ingresses with attachments
  inlined for UI convenience).
- `/ingresses/site/get` → `get_site_ingress`.
- `/ingresses/site/create` → operator creates a manual ingress; rejects `source`
  other than implicit-manual.
- `/ingresses/site/delete` → rejects discovered.
- `/ingresses/site/update` → description, tls_provider (manual only).
- `/ingresses/site/attachment/add`, `/remove`, `/update` — attach forward/redirect.
- `/ingresses/site/discovery/refresh` → kick the Tailscale provider's poll channel.
- `/ingresses/site/discovery/status` → return
  `(provider, last_poll_at, healthy, identity)`.

Each mutating handler emits an event (mirror `service.site.lifecycle.events`) and an
audit-log entry.

## CLI surface

New file `crates/ctl/src/ingresses.rs`. Subcommand under top-level
`Command::Ingresses`:

```
seedling-ctl ingresses site list
seedling-ctl ingresses site show <name>
seedling-ctl ingresses site create <name> --hostname <h> [--description ...] [--tls acme|internal|none]
seedling-ctl ingresses site delete <name>
seedling-ctl ingresses site attach <name> --port <n> --protocol <tcp|udp|http|https> --to <app>/<service>
seedling-ctl ingresses site attach-redirect <name> --port <n> --protocol <https|http> --to <url> [--code 307] [--preserve-path]
seedling-ctl ingresses site detach <name> --port <n> --protocol <p>
seedling-ctl ingresses site discovery refresh
seedling-ctl ingresses site discovery status
```

Pattern is identical to `crates/ctl/src/services.rs`; the dispatcher just shells JSON
RPC requests to the OI methods listed above.

## Web UI surface

New routes:

- `crates/web/frontend/src/routes/Ingresses.tsx` — list view, mirroring the layout of
  `Services.tsx`. Sections: Manual ingresses, Discovered ingresses (with provider
  chip and "stale" badge when applicable). Click-through to a detail dialog for
  attachments.
- Dialogs: `CreateSiteIngressDialog`, `ConfirmDeleteSiteIngressDialog`,
  `AttachForwardDialog`, `AttachRedirectDialog`, `DiscoveryStatusBanner` (shows on
  Ingresses page when Tailscale is detected logged-out).

Navigation: the Ingresses entry must sit **immediately after Services** in the
sidebar (i.e. before Volumes), so the operator-managed entry-point family reads
`Services → Ingresses → Volumes`. Wire this in the same place that defines the
existing nav order (currently `crates/web/frontend/src/App.tsx` and the sidebar
component).

Type additions in `crates/web/frontend/src/lib/types.ts`:
`SiteIngress`, `SiteIngressSource`, `SiteIngressAttachment`, `AttachmentTarget`,
`TlsProvider`.

API hooks: `useOiQuery("/ingresses/site/list")`, `useOiAction("/ingresses/site/...")`.

## Spec

Add to `docs/spec/runtime.md`, alongside `r[service.site]` (line 910):

- `r[ingress.site]` — definition, source kinds, TLS providers.
- `r[ingress.site.lifecycle]` — manual creation/deletion via operator commands;
  discovered ingresses managed by the daemon's discovery loop and not deletable while
  the source is active.
- `r[ingress.site.lifecycle.events]` — event/audit-log entries.
- `r[ingress.site.attachment]` — attachment shape (port, protocol, forward/redirect),
  multiple per ingress, cascade on parent deletion.
- `r[ingress.site.tailscale]` — Tailscale provider behaviour: hostname is the host's
  `Self.DNSName`; node-id is the stable identity key; renames update the row in
  place; TLS is provisioned via tailscaled.
- `r[ingress.site.conflict]` — when an app ingress and a site-ingress attachment
  claim the same `(hostname, port)`, both are dropped and both sides are faulted;
  faults auto-clear when the conflict is resolved.

Annotate implementations with `r[impl ingress.site*]` markers per AGENTS.md
conventions: type definitions, lifecycle handlers, the conflict detector, and the
Tailscale issuer.

## Faults

Add fault kinds (`crates/core/src/runtime/faults.rs`):

- `ingress_conflict` — app-side and site-side conflict on `(hostname, port)`.
- `site_ingress_target_missing` — attachment forwards to an app/service that doesn't
  exist.
- `tailscale_not_logged_in`, `tailscale_unreachable`, `tailscale_multiple_identities`,
  `tailscale_cert_issue_failed` — provider/issuer health.

## Critical files to modify

- `crates/core/src/runtime/db/migrations/v49.sql` (new)
- `crates/core/src/runtime/db.rs` (register `SQL_V49`)
- `crates/core/src/runtime/site_ingresses.rs` (new)
- `crates/core/src/runtime/site_ingress_attachments.rs` (new)
- `crates/core/src/runtime/tailscale.rs` (new)
- `crates/core/src/runtime/tls.rs` and `crates/core/src/runtime/tls/state.rs`
  (`TlsCertOrigin::Tailscale`, `TlsPolicy::Tailscale`)
- `crates/core/src/runtime/tls/tailscale_issuer.rs` (new)
- `crates/core/src/runtime/tls/issuance.rs` (dispatch in `Coordinator::run`)
- `crates/core/src/runtime/faults.rs` (new fault kinds)
- `crates/core/src/system/reconcile/site_proxy.rs` (new)
- `crates/core/src/system/reconcile/phases.rs` (call `site_proxy::collect`,
  `detect_conflicts`)
- `crates/core/src/system/reconcile.rs` (Tailscale provider startup/shutdown)
- `crates/core/src/system/translate/proxy.rs` (handle redirect-only entries from
  site ingresses; existing redirect machinery already supports the shape)
- `crates/core/src/oi/handler.rs` (route registration)
- `crates/core/src/oi/handler/ingresses.rs` (new)
- `crates/protocol/src/names.rs` (`SiteIngressName`)
- `crates/protocol/src/events.rs` (lifecycle events)
- `crates/ctl/src/main.rs` (subcommand registration)
- `crates/ctl/src/ingresses.rs` (new)
- `crates/web/frontend/src/lib/types.ts` (new types)
- `crates/web/frontend/src/routes/Ingresses.tsx` (new)
- `crates/web/frontend/src/components/site_ingress/*` (new dialogs)
- `crates/web/frontend/src/App.tsx` (route registration; nav order
  `Services → Ingresses → Volumes`)
- `docs/spec/runtime.md` (new `r[ingress.site*]` requirements)

## Verification

End-to-end checks, organised by layer:

1. **Migration**: `cargo test -p seedling-core --lib runtime::site_ingresses` and
   `runtime::site_ingress_attachments` covering CRUD, FK cascade, `(hostname, port)`
   uniqueness, source-locked delete.
2. **Tailscale provider** (unit, with a fake socket): identity discovery, rename in
   place, node-id change replaces row, logged-out marks stale, repeated failure files
   fault after threshold.
3. **TLS issuance**
   (`cargo test -p seedling-core --lib runtime::tls::tailscale_issuer`):
   parse PEM pair, persist with `origin=Tailscale`, last-good cert kept on issuer
   error.
4. **Reconcile**:
   - `cargo test -p seedling-core --lib system::reconcile::site_proxy` for collection.
   - Conflict detector unit tests covering: no conflict, one-side-only, full overlap,
     partial port overlap, fault auto-clear after resolution.
5. **Tracey**: `tracey query uncovered --spec-impl runtime/main` should report the
   new `r[ingress.site*]` items as covered after annotation pass.
6. **OI/CLI smoke**:
   - Create a manual site ingress, attach it forward to an app service, hit the
     hostname through Caddy from a peer node, expect 200.
   - Add an HTTP→HTTPS redirect attachment, expect 307 to the configured URL.
   - With Tailscale logged in, run `seedling-ctl ingresses site list` and confirm a
     discovered entry appears; run `discovery status` and confirm `healthy=true`.
   - Deliberately overlap an app ingress with a site attachment, expect both to fault
     and Caddy to drop both routes; remove the site attachment, expect both faults
     to clear automatically on the next tick.
7. **Web UI manual check**: load the Ingresses page in dev (`cd crates/web/frontend &&
   npm run dev`), verify list, create, attach (forward & redirect), and delete
   flows; confirm the discovered entry's delete button is disabled with a tooltip,
   and confirm the sidebar order is `Services → Ingresses → Volumes`.
8. **Lints / fmt / tracey status**: `cargo clippy`, `cargo fmt`, `tracey query
   status` per AGENTS.md before finishing.
