# Site services: IPv4 and DNS endpoints

## Context

Today, every `site_service_endpoint` row's `remote_host` must be an IPv6 literal.
Three layers enforce this:

- `crates/core/src/oi/handler/services.rs:99` — `require_ipv6_remote_host` rejects
  anything else with `RequirementsInvalid` and a "tracked as a follow-up" message.
- `crates/ctl/src/services.rs:316` — `parse_ipv6_remote` rejects v4 / DNS at the CLI
  layer.
- `crates/core/src/system/reconcile/{rules,routes}.rs` — the route and DNAT builders
  call `e.remote_host.parse::<Ipv6Addr>().ok()` and silently drop anything that fails
  to parse, falling back to a blackhole (defence in depth — see the comment at
  `rules.rs:504`).

The DB column itself is plain `TEXT` (`v40.sql`), so the constraint is policy, not
schema.

This blocks two common operator shapes:

1. **IPv4-only legacy backends** — a database, monitoring sink, or message broker that
   has no IPv6 address. Today the operator either has to give it one or stand up a
   v6-fronting proxy themselves.
2. **DNS-named backends** — cloud-provider endpoints (RDS, MSK, internal load
   balancers) where the address can change but the hostname is stable. Today the
   operator has to resolve manually and re-register on every DNS change.

The pod data plane is IPv6-only by design (`docs/networking.md`); to extend site
services to v4/DNS we have to bridge those into the IPv6 backend list the existing
DNAT and route code consumes, not introduce a new v4 path through pods.

## Decisions

- **No schema change.** `remote_host` stays `TEXT`. Validation moves into the OI/CLI
  layer; the reconciler treats unresolvable / unroutable rows as the equivalent of
  zero backends (blackhole + fault).
- **IPv4 literal → NAT64-wrapped IPv6.** `192.0.2.10` resolves to `64:ff9b::192.0.2.10`
  for the data plane. NAT64 must be active on this host (either auto-detected
  external or daemon-managed); if not, the endpoint is faulted as unroutable.
- **DNS name → background resolver.** A new `SiteServiceResolver` task maintains a
  cache `{ host → Vec<IpAddr> }` keyed by literal hostname, refreshing on TTL.
  Reconciler reads from the cache; cache changes notify the reconciler.
- **AAAA preferred when host has IPv6 egress.** Dual-stack lookups: if the host has
  IPv6 egress (existing `force_dns64_translation` probe in
  `crates/core/src/system/netinfo.rs:66`), use AAAA literally and ignore A; otherwise
  use the A records via NAT64 and ignore AAAA. This mirrors how pod DNS is already
  steered for IPv4-only hosts.
- **DNS resolver: `hickory-resolver` 0.25.x.** Reasons: TTL exposure, separate A vs
  AAAA queries, async, well-maintained, drop-in `system_conf::read_system_conf` so
  the daemon picks up `/etc/resolv.conf` changes on the host (split-DNS, MagicDNS).
  The dependency-free fallback is `tokio::net::lookup_host` on a fixed cadence — it
  works, but loses TTL fidelity and can't separate AAAA from A. **Confirm with user
  before adding.**
- **No write-time NAT64 check.** An operator may register a v4 endpoint on a host
  that doesn't yet have NAT64 — the host could gain it later. We surface
  unroutability as a fault, not a write-time error. Same for DNS names that don't
  resolve at all.
- **DNS resolver runs on the host, not via CoreDNS.** Site service backends are
  resolved using the host's `/etc/resolv.conf` so that operator-controlled
  resolvers (corporate DNS, Tailscale MagicDNS) work as expected. CoreDNS stays
  pod-only.

## Address resolution pipeline

A new module `crates/core/src/runtime/site_services/resolve.rs` centralises the
"endpoint string → list of `(Ipv6Addr, port)` for the data plane" function:

```rust
pub struct ResolveCtx<'a> {
    pub nat64_active: bool,
    pub has_ipv6_egress: bool,
    pub resolver_cache: &'a SiteResolverCache,
}

pub enum ResolveOutcome {
    /// One or more IPv6-routable addresses; data plane consumes these.
    Routable(Vec<Ipv6Addr>),
    /// The host parses but routing is impossible right now (e.g. v4 literal
    /// + NAT64 disabled, or DNS name with only A records and no NAT64).
    Unroutable { reason: UnroutableReason },
    /// Hostname has not yet been resolved or repeatedly failed.
    Unresolved { reason: UnresolvedReason },
}

pub fn resolve_endpoint(host: &str, ctx: &ResolveCtx<'_>) -> ResolveOutcome;
```

Cases:

| `remote_host` shape | Outcome                                                               |
|---------------------|-----------------------------------------------------------------------|
| IPv6 literal        | `Routable([ip])`                                                      |
| IPv4 literal        | NAT64 active → `Routable([nat64_synth(ip)])`; otherwise `Unroutable`. |
| DNS name, AAAA      | Host has v6 egress → `Routable(all AAAA)`; otherwise route via A.     |
| DNS name, A only    | NAT64 active → `Routable([nat64_synth(a)])`; otherwise `Unroutable`.  |
| DNS name, no record | `Unresolved`                                                          |

`nat64_synth(v4)` is RFC 6052 form: `64:ff9b::a.b.c.d`, computed inline (no
runtime allocation; it's an `Ipv6Addr` constant from segments).

`rules.rs::resolve_external_backends` and `routes.rs::build` both stop calling
`parse::<Ipv6Addr>()` directly and call `resolve_endpoint`. Each tick they thread
through a `ResolveCtx` populated from the reconciler's existing `nat64_active`,
`force_dns64_translation` (negated → `has_ipv6_egress`), and the new resolver
handle. Per-protocol filtering stays in `protocols_match` as today.

## SiteServiceResolver

New file `crates/core/src/runtime/site_services/resolver.rs`.

```rust
pub struct SiteServiceResolver {
    inner: Arc<ResolverInner>,
}

struct ResolverInner {
    cache: RwLock<HashMap<HostKey, CachedEntry>>,
    kick: mpsc::Sender<()>,
    notify_tick: Arc<Notify>,
    failure_streak: Mutex<HashMap<HostKey, u32>>,
}

struct CachedEntry {
    aaaa: Vec<Ipv6Addr>,
    a: Vec<Ipv4Addr>,
    fetched_at: Instant,
    expires_at: Instant,            // earliest TTL across the result records
    last_error: Option<DnsErrKind>,
}

impl SiteServiceResolver {
    pub fn spawn(db: DbHandle, tick_notify: Arc<Notify>) -> Arc<Self>;
    pub fn lookup(&self, host: &str) -> Option<CachedEntry>; // None == not yet resolved
    pub fn kick(&self);                                      // OI handler entry
}
```

Behaviour:

- **Bootstrap**: on startup, read every distinct `remote_host` from
  `site_service_endpoints` whose value isn't an IP literal, kick the resolver to
  populate.
- **Tick**: every 5 s, refresh any entry whose `expires_at` is in the past or whose
  `last_error` is set and at least 5 s old (negative caching is short).
- **Kick channel**: OI add/remove endpoint handlers send on `kick` so the resolver
  picks up the new name immediately. Kick is also fired at startup.
- **Resolver**: `hickory_resolver::AsyncResolver::tokio_from_system_conf()` reads
  `/etc/resolv.conf`. Two queries per host: `lookup_ip` separating AAAA and A.
- **Failure**: count consecutive failures per host. After N=5, file
  `site_service_endpoint_unresolvable` once; clear when a successful lookup
  returns. Mirrors the `warm_cert_first_seen` pattern in
  `crates/core/src/runtime/tls/issuance.rs:402`.
- **Cache change → tick**: when a host's resolved IP set changes (set inequality,
  not just refresh), call `tick_notify.notify_one()` so the reconciler rebuilds
  routes/nft for the affected services.
- **GC**: every refresh tick, drop cache entries that no longer correspond to any
  row in `site_service_endpoints` (e.g. operator removed the endpoint).
- **No retries inside one tick**: a single failed lookup doesn't retry; the next
  refresh tick will.

The resolver is owned by the daemon (created in `crates/core/src/system/reconcile.rs`
alongside the existing per-tick context). The reconciler's
`ExternalServiceSnapshot::load` path already runs once per tick — it reads the
endpoints table and the resolver cache, no extra DB hops.

## NAT64 dependency

`nat64_active` is already tracked on `Reconciler` (`reconcile.rs:330`) and threaded
through where it matters. We pass it to `ResolveCtx`.

The synthesised `64:ff9b::a.b.c.d` form depends on Jool's pool6 being the same
well-known prefix. That's a hard-coded constant today (`system/jool.rs:6`). We
expose it as `pub const NAT64_PREFIX: Ipv6Net` from a single module
(`crates/core/src/runtime/nat64_prefix.rs`) so the resolver and the data-plane
synthesis agree.

If `--nat64=disabled` and the host has no external NAT64, IPv4 endpoints can't
work. The resolver still records the result and reports `Unroutable`; the
reconciler files `site_service_endpoint_unroutable` against the affected site
service (faults below). Operators see the misconfiguration immediately.

## Validation rule changes

- **`require_ipv6_remote_host` becomes `validate_remote_host`** (name and signature
  in `oi/handler/services.rs:99`). The new validator accepts:
  - `IpAddr::from_str(host).is_ok()` (v4 or v6 literal); or
  - a syntactically valid DNS name (use the existing `seedling-protocol::names`
    machinery if present, else add a small validator: 1–253 chars, dot-separated
    labels of 1–63 chars matching `[A-Za-z0-9-]+`, each label not starting/ending
    with `-`, total domain not numeric-only). No `localhost`, no
    `[link-local]:port`, no `_underscore-labels`.
- **`parse_ipv6_remote` becomes `parse_remote`** in `crates/ctl/src/services.rs:316`.
  Strategy:
  - If input contains `[…]:port` — IPv6 literal, parse via `SocketAddrV6`.
  - Else split on the **last** `:`. The host part is then either an `Ipv4Addr` or a
    DNS name.
  - Reject empty host, port out of range, `localhost` shorthand.
- **Spec.** `r[service.site]` is amended to specify what `remote_host` may be (IPv6
  literal, IPv4 literal, or DNS name) and that resolution is performed by the
  daemon at runtime. New child item `r[service.site.address]` describes the NAT64
  dependency for IPv4 / A-only DNS records.

## Reconcile integration

Two narrow refactors:

- `crates/core/src/system/reconcile/rules.rs:340` — `resolve_external_backends`
  takes `ctx: &ResolveCtx<'_>` instead of duplicating the `parse::<Ipv6Addr>().ok()`
  filter. Returns the same `Vec<(Ipv6Addr, u16)>` shape.
- `crates/core/src/system/reconcile/routes.rs:127` — same refactor. (Routes don't
  carry ports, so the call collapses to `resolve_endpoint(host, &ctx)` flatmapped
  over endpoints whose `(service_port, protocol)` match the reconciler's per-
  service walk.)

`crates/core/src/system/reconcile.rs` builds the `ResolveCtx` once per tick (it
already has `nat64_active`, `force_dns64_translation`, and now the resolver
handle) and passes it down through `compute_routes` and `compute_nft_rules`
(`phases.rs:79`/`phases.rs:135`).

`ExternalServiceSnapshot` itself doesn't change — it still carries the raw
endpoint rows. Resolution happens at consumption time; that way one snapshot can
be passed to both routes and rules without re-resolving.

Per-tick faults from resolution outcomes are accumulated in a small
`SiteServiceFaultSet` returned by both compute steps and merged into the existing
fault apply loop (similar shape to `degraded_services` in `ServiceDnatBuild`).

## Faults

Add to `crates/core/src/runtime/faults.rs`:

- `site_service_endpoint_unresolvable` — one or more endpoint hostnames have
  failed DNS resolution past the failure-streak threshold. Filed against
  `_system` with `resource_type=site_service`, `resource_name=<site_service>`,
  description names the failing hosts.
- `site_service_endpoint_unroutable` — endpoint resolves to an address that the
  current host cannot route to (v4 literal or A-only DNS without NAT64). Same
  scoping. Description names the host(s) and the missing capability.

Both faults auto-clear when the next tick observes the condition resolved
(scratchpad on the reconciler — copy the `prev_states` pattern used for ingress
conflicts in the site-ingresses plan).

`site_service_endpoint_unresolvable` and `site_service_endpoint_unroutable` are
*non-blocking*: the rest of the tick proceeds, just with empty / partial backends
for the affected service. No traffic loss to other site services.

## Web UI

`crates/web/frontend/src/routes/Services.tsx` and the dialog around line 217:

- Replace the current input that expects `[ipv6]:port` with two fields: **host**
  (text) and **port** (number).
- Validation in JS: host must match an `IPv4 | IPv6 | DNS-name` regex. Show the
  same error message as the backend would. The backend remains the source of
  truth.
- Listing display: today renders `[ip]:port` regardless of family. Switch to
  `host:port` when host isn't an IPv6 literal (no brackets). Add a small
  resolver-status indicator per row when the host is a DNS name (resolved /
  resolving / failing). Read from a new `/services/site/resolver-status` OI
  endpoint that returns the resolver cache snapshot.
- `crates/web/frontend/src/lib/types.ts` keeps the existing `SiteServiceEndpoint`
  shape; add an optional `resolved_addresses?: string[]` field for the listing
  view.

## CLI

`crates/ctl/src/services.rs`:

- Replace `parse_ipv6_remote` with `parse_remote` (described above).
- Update help text on `seedling-ctl services site add-port` and `remove-port` to
  list the accepted host shapes.
- Drop `parse_ipv6_remote_rejects_ipv4` / `parse_ipv6_remote_rejects_bare_dns_name`
  tests; replace with positive cases for each shape.
- New subcommand `seedling-ctl services site resolver` (and `--watch`) prints
  the resolver cache for the operator to see what's actually being routed.

## OI handler surface

`crates/core/src/oi/handler/services.rs`:

- `validate_remote_host` replaces `require_ipv6_remote_host`.
- New method `/services/site/resolver-status` returns
  `{ entries: [{ host, addresses: [ip,...], expires_at, last_error }] }`.
- Add/remove endpoint handlers call `resolver.kick()` after DB writes.

## Spec

`docs/spec/runtime.md` around line 953–966:

- Amend `r[service.site]`: `remote_host` may be an IPv6 literal, an IPv4 literal,
  or a DNS name. Resolution occurs at runtime; backend selection follows
  per-`(service_port, protocol)` grouping unchanged.
- New `r[service.site.address]` (sibling of `r[service.site]`):

  > Site service endpoints whose `remote_host` is an IPv4 literal, or a DNS name
  > resolving only to A records, must be routed via NAT64 using the well-known
  > `64:ff9b::/96` prefix. The runtime must report endpoints as unroutable when
  > NAT64 is unavailable on the host (e.g. when the operator has explicitly
  > disabled it and no external NAT64 is present), and as unresolvable when DNS
  > resolution has failed past a small consecutive-failure threshold. These
  > conditions are surfaced as faults on the affected site service and must
  > auto-clear when the underlying condition resolves.

- Annotate the new resolver, the centralised `resolve_endpoint`, and the fault
  paths with `r[impl service.site.address]` (and `r[verify ...]` on tests).
- Update existing `r[impl service.site*]` annotations on
  `oi/handler/services.rs`, `system/reconcile/{rules,routes}.rs` to reflect the
  new flow.

## Critical files to modify

- `crates/core/src/runtime/site_services.rs` (host-shape helpers, no schema)
- `crates/core/src/runtime/site_services/resolve.rs` (new — `ResolveCtx`,
  `resolve_endpoint`)
- `crates/core/src/runtime/site_services/resolver.rs` (new — background task)
- `crates/core/src/runtime/nat64_prefix.rs` (new — shared `NAT64_PREFIX_NET`,
  `synth_v4(v4) -> Ipv6Addr`)
- `crates/core/src/runtime/faults.rs` (two new fault kinds)
- `crates/core/src/runtime.rs` (re-export resolver handle)
- `crates/core/src/system/reconcile.rs` (spawn resolver, build `ResolveCtx`,
  thread to compute steps; merge new faults)
- `crates/core/src/system/reconcile/phases.rs` (signature plumbing)
- `crates/core/src/system/reconcile/rules.rs` (use `resolve_endpoint`; drop the
  inline `parse::<Ipv6Addr>` defence and the silent-skip path)
- `crates/core/src/system/reconcile/routes.rs` (same)
- `crates/core/src/system/jool.rs` (use shared `NAT64_PREFIX`)
- `crates/core/src/system/resolver/config.rs` (use shared `NAT64_PREFIX`)
- `crates/core/src/oi/handler/services.rs` (`validate_remote_host`,
  `resolver-status` route, kick on writes)
- `crates/core/src/oi/handler.rs` (route registration)
- `crates/ctl/src/services.rs` (`parse_remote`, `services site resolver`
  subcommand, test rewrite)
- `crates/web/frontend/src/routes/Services.tsx` (host/port split, resolver
  badge)
- `crates/web/frontend/src/lib/types.ts` (`resolved_addresses`)
- `Cargo.toml` workspace: `hickory-resolver = "0.25"` (TOCONFIRM)
- `docs/spec/runtime.md` (amend `r[service.site]`, new `r[service.site.address]`)
- `docs/networking.md` (a paragraph under "DNS resolver" or new top-level "Site
  service backend resolution" section explaining the host-side resolver and
  NAT64 synthesis path)

## Verification

1. **Validation unit tests** (`oi/handler/services.rs`,
   `crates/ctl/src/services.rs`): each accepted shape parses; `localhost`,
   underscore labels, empty hosts, ports out of range all rejected.
2. **NAT64 prefix synthesis**: `synth_v4(192.0.2.10) ==
   "64:ff9b::c000:20a".parse()`; round-trip across the IANA prefix.
3. **`resolve_endpoint` matrix**: every row of the table above. Use a fake
   `SiteResolverCache` populated in-test; no real DNS.
4. **Resolver task**: stand up a test resolver against an in-process fake
   `hickory` upstream (or mock the `lookup_ip` method behind a trait). Cover:
   first lookup, TTL expiry refresh, v4-only result, v6-only result,
   dual-stack, NXDOMAIN, repeated failure files fault, kick channel triggers
   refresh, GC drops stale entries.
5. **Reconciler integration** (`system/reconcile/{rules,routes}.rs` tests):
   feed a `ResolveCtx` with each combination of `nat64_active` ×
   `has_ipv6_egress`; assert the right backends and faults emerge for each
   endpoint shape.
6. **End-to-end smoke**:
   - Register a site service whose endpoint is `192.0.2.10:5432` on a NAT64-
     active host. Confirm with `seedling-ctl services site resolver` that the
     synthesised `64:ff9b::c000:20a` shows up. Connect from a pod, confirm
     traffic egresses through Jool.
   - Same with `db.example.com:5432` (use a name with both A and AAAA on a
     dual-stack host); confirm AAAA path is used and no NAT64 traffic.
   - Same with `db.example.com` on a NAT64-only host; confirm A path via
     synthesised v6 is used.
   - `--nat64=disabled` + v4 endpoint → fault appears; remove endpoint → fault
     clears next tick.
   - DNS name that doesn't resolve → fault appears after threshold; fix DNS →
     fault clears.
7. **Web UI manual check**: load Services dialog in dev (`cd
   crates/web/frontend && npm run dev`); confirm host/port split, validation
   messaging, resolver badge.
8. **Tracey**: `tracey query uncovered --spec-impl runtime/main` should report
   the new `r[service.site.address]` items as covered after annotation pass.
9. **Lints / fmt / tracey status**: `cargo clippy`, `cargo fmt`, `tracey query
   status` per AGENTS.md before finishing.

## Open question for the user

- **DNS resolver dependency**: `hickory-resolver` (recommended) vs sticking with
  `tokio::net::lookup_host` on a fixed cadence. The recommendation reflects the
  TTL/A-vs-AAAA needs above; the alternative trades fidelity for zero new deps.
  Confirm before I add the crate.
