# Seedling in production

## Problem

Seedling runs BES workloads internally, but it has never taken over a host that was already
serving production traffic. Three capabilities stand between it and that: an ingress proxy
configured richly enough to serve Tamanu the way Tamanu is served today, app definitions that
describe the real fleet rather than exercise the runtime, and a way to bring an app fully up
and prove it before it starts carrying traffic.

The impetus is the Linux fleet. Every Tamanu host is a candidate to move onto Seedling, and
the migration starts as soon as Seedling can take one. Each requirement below traces to
something a production host does today and that Seedling would otherwise stop doing:
compressing and rate-limiting at the edge, serving the API on the patient portal hostname,
obtaining a certificate while another process still holds `:80`.

Requirements are drawn from the ops-side migration plan
(`adhoc-to-seedling-migration.md`, deploy repo) and verified against the code here.

## Scope

Seedling comes out of this project owning **ingress and workloads** on a production host: HTTP
routing, TLS and certificates, and the Tamanu, patient portal and mSupply containers along
with their config, lifecycle and upgrades.

PostgreSQL moves in a later project, and that ordering is a design decision rather than a
deferral. Serving traffic is the recoverable thing to trust Seedling with first: the Seedling
app and the host's existing stack share one database, so a host can move onto Seedling and
back without stranding a write. Taking the database first would put writes into a new cluster
immediately and make the same reversal lossy.

Two items in this project exist to buy that property and retire with it. The `pg-socket` bind
site volume gives containers the host cluster's socket, and `DATABASE_URL` password support
(A2) is needed only because the host cluster authenticates with `scram-sha-256` where
Seedling's own generated `pg_hba.conf` trusts the local socket.

## Constraints that shape the design

**The `:80`/`:443` flip is atomic per host.** Seedling routes ingress by DNAT-ing
locally-destined traffic to its Caddy container, and site-ingress attachments can only target
app services, never a host process. There is no per-vhost phasing. Everything sharing `:443`
on a host moves in one step.

**HTTP-01 is structurally unavailable during a cutover.** Host Caddy owns `:80` right up to
the flip, so a Seedling app that needs a certificate cannot get one by HTTP-01 first. This is
forced ordering, not inconvenience.

**Seedling with no apps registered is inert.** The reconciler idles: no Caddy container, no
nftables rules, no resolver. Installing the package fleet-wide changes nothing that serves
traffic, which is why stage 1 does not wait on any of this work.

## Components

### A. Tamanu app definitions

Everything in `apps/` was written to exercise the runtime, not to serve the fleet. These are
our definitions to own and rewrite, not asks of another team. Some definitions exist only as
untracked dev drafts outside the repo, so start from the draft rather than from scratch.

The bigger change is regime, not delta. Tamanu is dropping json5 config: proxy trust,
localisation, timezone, disk thresholds, status reporting, auth and refresh secrets, sync
credentials, the central canonical URL, and the facility id all move into Tamanu's internal
settings, in the database, where they cross a cutover untouched. The current definitions still
render `/production.json5` from a `config` volume and carry `auth-secret`, `sync-password`,
`facility-id`, `central-url` and `timezone` as params. All of that goes.

Two pieces of per-host state survive, and only two: the per-server crypto key and the database
credentials. Both have to be lifted off the running host during adoption.

| # | Requirement | Blocks |
| --- | --- | --- |
| A1 | Per-server config key as a `secret(true)` param, written into a tmpfs volume by static `Volume.write` so it reapplies on container restart, with the env var pointing at the mount path | All hosts |
| A2 | `DATABASE_URL` carries a password. The host cluster authenticates the `tamanu` role with `scram-sha-256`, so socket-trust assumptions cannot connect. Retired once Postgres moves into Seedling, whose generated `pg_hba.conf` is `local all all trust` | All hosts |
| A3 | Central: bind `/api` and `/v1` on `portal_svc` to the API deployment. `tamanu-central.seed.rhai:229` binds them on `web_svc` only, so a migrated portal serves its frontend and then fails every API call | All central hosts |
| A4 | The facility app must not force an HTTPS ingress. `tamanu-facility.seed.rhai:30` marks `public-hostname` `.required(true)` and declares the ingress from it, while central guards on `is_set()`. Plaintext `.local` hosts need the app to declare no ingress so the site ingress can carry traffic | The `.local` class, 15 hosts |
| A5 | mSupply app promoted into `apps/` from the existing dev draft, carrying the `/etc/msupply/local.yaml` content, the arch-selected image, and the persistent volume | `fsm-prod`, `tokelau-prod` |

`tamanu_extra_certs` (host CA mount and `NODE_EXTRA_CA_CERTS`) is set by no inventory host.
Recorded, not built.

### B. Proxy feature parity

Verified against `crates/core/src/system/caddy/config.rs`. `build_caddy_config` emits routing
and TLS automation only. `proxy_routes_for_vhost` emits a bare `reverse_proxy` with a list of
upstreams. There is no compression, no load-balancing or retry configuration, no rate
limiting, no header manipulation, no cache-control, and no error handling anywhere in it.

Tier 1 blocks every production cutover. Tier 2 blocks non-AWS hosts only: AWS deployments sit
behind a stack-wide security group, AWS Backup snapshots, and recovery paths that do not need
the host healthy, so a tier 2 gap is survivable there. On-prem has none of that.

| # | Requirement | Tier |
| --- | --- | --- |
| B1 | Response compression (`encode zstd gzip`). Losing asset compression hurts most where connectivity is worst | 1 |
| B2 | Upstream retries and load-balancing policy (`lb_retries 2`, `lb_try_duration 5s`, `lb_policy least_conn`). Without retries, requests hitting a draining container 502 on every rolling restart | 1 |
| B3 | Per-route rate limiting (1000/s per IP on `/api/*`, 10/s on `/api/login`). Dropping this removes brute-force protection on login | 1 |
| B7 | `Cache-Control` rules: `no-cache` on `/` and `/manifest.json`, `no-store` on API responses, immutable on `/env.js` | 1 |
| B8 | WAF hook equivalent to `import waf*`. `caddy_waf_enforce` is false fleet-wide, so the hook is the requirement, not enforcement | 1 |
| B4 | Upstream `Host` header override. The frontend upstream currently receives `Host: localhost` | 2 |
| B5 | Systemd slice and `OOMScoreAdjust` control from BSL (`critical.slice` at `-500` for API, `elevated.slice` for sync and fhir workers) | 2 |
| B6 | Custom error pages served from the container's `/resources/errors/` | 2 |
| B9 | Path-level redirect within an ingress, for `redir /v1/login /api/login 308`. Redirects today exist at vhost level and as site-ingress `attach-redirect`, not per path | 2 |
| B10 | HTTP/1.1 keep-alive response headers | 2 |

### C. Certificates without `:80`

A cutover host with a public hostname needs a certificate in Seedling's store before the flip.
Three candidate paths, any one of which satisfies the requirement:

- Canopy off-site issuance
- Route53 DNS-01. The runtime drives DNS-01 itself and needs no listener, so this works while
  host Caddy still owns `:80`. `acme-dns.ts` currently mints users only for `external` servers
- Importing the live leaf and key out of Caddy's storage with `tls certs upload-manual`, which
  already exists across ctl, the OI, and the web interface

**All three are blocked by the same defect.** `cert_valid` observations are emitted only when
a cert file is found in Caddy's on-disk cache
(`crates/core/src/system/caddy/cert_observation.rs`), and `rt.warm_certs(...).ready()`
resolves against exactly those observations. Certificates the runtime provisioned by DNS-01,
or that an operator uploaded, are served to Caddy through the `get_certificate` HTTP endpoint
and never land in that cache. So pre-provisioning a certificate makes install *worse* rather
than better: the barrier stalls and then faults with `cert_acquisition_failed`.

Warm-cert observation has to be satisfiable by runtime-managed and imported certificates, not
only by ones Caddy fetched itself. This is the single highest-leverage fix in the project: it
gates C entirely and is a hard dependency of D.

Related discrepancy in the app definitions: `tamanu-facility.seed.rhai:392` calls
`rt.warm_certs(app).ready()` and blocks, while `tamanu-central.seed.rhai:362` calls
`rt.warm_certs(app)` without `.ready()` and does not. The two apps behave differently at
install and should be made deliberate either way.

### D. Staged ingress takeover

A capability request rather than a parity gap, and the most valuable thing to come out of
investigating smoother cutovers.

**Ask:** a mode where an app is fully installed and running (containers up, ingresses
declared, TLS provisioned, routing live) but Seedling has not installed the host DNAT rules
and so receives no host traffic. Everything is verifiable in that state on an address Seedling
manages. A separate explicit operator action then says "take over": the DNAT rules go in and
traffic moves.

**Why it is worth building.** It splits one irreversible-feeling step into a long verifiable
phase and a short mechanical one. Today the ops plan approximates this by leaving the app's
ingress unscheduled through stage 2, which works, but means the ingress path itself (TLS,
routing, prefix matching, upstream health) is first exercised at the moment it starts carrying
production traffic. Under a staged takeover the whole pathway is proven first and the flip
verifies nothing new.

HTTP-01 is permanently out of scope for this mode. It cannot work while another process owns
`:80`, and the point of the mode is that Seedling does not own `:80` yet.

### Existing behaviour the migration depends on

Verified, no work required, but load-bearing enough that changing it would break the plan:

- **The flip drains rather than cutting over.** The DNAT chains are NAT-type, which netfilter
  traverses only for `NEW`-state packets; the apply is a single atomic batch that rebuilds the
  table with no conntrack flush; every chain is `policy: Accept` with an established-accept
  rule first. Established connections finish against host Caddy while new ones go to Seedling,
  and no packets are dropped in between.
- **A site ingress with no attachments emits no DNAT rules**, so it can be created in advance
  during adoption without moving any traffic.
- **`apps stop-resource <app> ingress <name>` unschedules an app ingress**, which is what lets
  registration, params, image pulls and `apps plan` all happen while `:443` still belongs to
  host Caddy.

Because the flip drains, downtime is `apps install` (warm certs, then `rt.start(app).ready(300)`,
plus the DB provision and migrate jobs), not the flip itself. Pre-pulling images removes one
term; a staged takeover removes most of the rest.

## Success criteria

- A pilot AWS environment, built through the existing ad-hoc path and then migrated, serves
  production-shaped traffic from Seedling with tier 1 parity and no regressions.
- A certificate provisioned by DNS-01 or uploaded manually satisfies `rt.warm_certs().ready()`.
- An app can be installed, started and verified on a host where Seedling does not yet own
  `:80`/`:443`, and taken over by one explicit operator action.
- Rollback stays one step at any point before decommission: unschedule the app ingress, detach
  the site ingresses, stop the app, start the old units.

## Open questions

- **Does the logic-bug audit remediation gate this project?** The July 2026 audit found 138
  defects and was carried out "ahead of larger-scale deployment", which is this. Themes 1, 2
  and 4 plus the critical finding have landed; themes 3, 5, 6, 7 and 8 sit on unmerged
  branches, as does `docs/failure-modes.md` and its CI checks. Several outstanding findings
  bear directly on cutover risk: image-pull retry exhaustion, Tailscale issuance bypassing the
  TLS state machine, and the exact-hostname cert fast path serving expired certs with no
  `not_after` check anywhere on the serve path. Needs a decision on whether these are in scope
  here or tracked separately.
- **Which of the three certificate paths do we build for?** They are not equivalent in effort
  or in what they leave behind. The warm-cert fix is common to all three, but committing to
  one changes what stage 3 looks like.
- **What shape does the staged takeover take?** A per-app flag, a site-level mode, or an
  explicit `ingresses takeover` operation. This changes the ops stages 2 and 3 enough that
  their step lists get rewritten against what ships rather than adapted to it.
- **How does `bestool-alertd` health-check a Seedling-run Postgres?** Not needed for this
  project, but one of the options is "move the checks inside Seedling and have it report
  them", which overlaps Canopy reporting. Worth knowing before Canopy work is scheduled.