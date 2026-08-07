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

App definitions are owned by the apps they describe. The Seedling repo keeps the common ones,
and gains the ability to take a definition from the app's own repo at the version that app is
running (E).

Seedling is also operable from Canopy rather than only from the host: reporting health worth
acting on, accepting direction for work it should do, and performing backups Canopy asks for
rather than hosting a backup framework of its own (F, G).

PostgreSQL moves in a later project, and that ordering is a design decision rather than a
deferral. Serving traffic is the recoverable thing to trust Seedling with first: the Seedling
app and the host's existing stack share one database, so a host can move onto Seedling and
back without stranding a write. Taking the database first would put writes into a new cluster
immediately and make the same reversal lossy.

Two items in this project exist to buy that property and retire with it. The `pg-socket` bind
site volume gives containers the host cluster's socket, and `DATABASE_URL` password support
(A2) is needed only because the host cluster authenticates with `scram-sha-256` where
Seedling's own generated `pg_hba.conf` trusts the local socket.

## Components

### A. Tamanu app definitions

Everything in `apps/` was written to exercise the runtime, not to serve the fleet. These are
our definitions to own and rewrite, not asks of another team. Some definitions exist only as
untracked dev drafts outside the repo, so start from the draft rather than from scratch.

These definitions are headed for the repos of the apps they describe (see E). The rewrite
below is the same work either way, so it does not wait on that move.

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

### C. Certificates for a hostname Seedling does not yet serve

Seedling can obtain a certificate for a hostname whose traffic it already receives. It needs
to be able to obtain one for a hostname it does not, because a host being adopted still has
another process on `:80` and will until the moment it hands over. HTTP-01 is unavailable by
construction for as long as that is true, so the certificate has to arrive some other way.

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

A new capability rather than a parity gap, and the most valuable thing to come out of
investigating smoother cutovers.

Ingress today is all or nothing. DNAT rules are emitted for an app's scheduled ingresses, so
an ingress is either unscheduled and untested or scheduled and carrying every request that
arrives on the port. There is no state in between, and no way to exercise the ingress path
(TLS, routing, prefix matching, upstream health) before it is load-bearing. Unscheduling an
ingress gets you an app you can verify everywhere *except* the part being introduced.

**Ask:** a mode where an app is fully installed and running (containers up, ingresses
declared, TLS provisioned, routing live) but Seedling has not installed the host DNAT rules
and so receives no host traffic. Everything is verifiable in that state on an address Seedling
manages. A separate explicit operator action then says "take over": the DNAT rules go in and
traffic moves.

This is worth building beyond adoption. It is the difference between proving an ingress change
and hoping for one, and it applies to any host where routing changes under live traffic.

HTTP-01 is out of scope for this mode by construction: it needs `:80`, and the premise is that
Seedling does not have it yet.

### E. Where definitions live, and how they stay current

An app's definition describes that app, so it belongs with that app. The Tamanu definition
lives in the Tamanu repo, released on Tamanu's cycle, changing in the same commit as the thing
it describes. The Seedling repo keeps definitions for what is genuinely common and owned by no
single app: Postgres, kopia.

Nothing supports that today. `/apps/create` and `/apps/update` take BSL source text and
nothing else, and ctl reads a file and posts its contents. Seedling stores the script durably
but records no provenance, so a registered app cannot say which repo or release its definition
came from, and Seedling has no way to fetch a newer one. The definition and the app it
describes are versioned independently, with nothing holding them in step.

The json5 removal is the live example. A Tamanu release changes what its definition must say,
but the definition sits in a different repo on a different cadence, so setting the `version`
param to a release whose definition has not been updated to match produces a running app
configured for the wrong regime. Section A only fixes the current instance of that; it is the
distribution model that stops it recurring on every release.

What this needs, in outline:

- A definition carries provenance: where it came from, and at which version
- Seedling can fetch a definition from that source, rather than only accepting pushed text
- The definition and the app version move together, so upgrading an app takes the definition
  that release expects

The mechanism is open. Whatever it is, it has to hold the property that makes `/apps/update`
safe today: a definition that fails to evaluate leaves the previous one running and observable
state unchanged.

### F. Canopy: control at a distance, and reporting worth reading

The base integration exists. Seedling has no Canopy identity of its own; a connected client
(in practice bestool) offers to carry its requests under its own identity, and the relay is a
generic outbound HTTP conduit over that connection. On top of it, the runtime reports every
sixty seconds while an offer is live, carrying four fixed checks (`health/apps`,
`health/faults`, `health/proxy`, `health/resolver`) plus version, daemon uptime, app counts by
status, operations in progress, and active fault count. Enable and disable are on ctl and the
web interface.

Two directions of further work.

**Control.** Canopy should be able to act on a host: set a param value for an app it can see,
and thereby drive real work. Bumping Tamanu's `version` is the motivating case, because
`version.on_change` already runs the upgrade closure, so one param set is a whole upgrade.

The seam for this already exists and is unused. `r[canopy.report.backup-prompt]` specifies
that a report's response carries instructions for the reporting source, and that Seedling
receives an empty list and does not act on it. That response is the natural inbound channel:
it is poll-driven, bounded to the report cadence, needs no new listener, and requires no
inbound authority through the relay. Worth preserving in whatever shape this takes, because
the relay is deliberately outbound-only and the OI deliberately refuses to relay arbitrary
requests: an inbound path would hand the carrying client's Canopy authority in the other
direction.

**Reporting.** The check catalogue is fixed on purpose, so that what Canopy has to maintain
does not grow with the set of apps an operator installs. That constraint bounds what richer
reporting can mean: more useful checks, not per-app checks. Checks already name the apps and
faults responsible for their result, so an operator can act without a second lookup, which is
the pattern to extend.

Both directions are polish on something that works rather than new subsystems.

### G. Backups through Canopy rather than through apps

Seedling currently ships a backup framework of its own. An app can be registered as a backup
provider if it declares `save-snapshot`, `list-snapshots` and `restore-snapshot`; named
strategies bind a provider to a schedule and a list of volumes; the runtime schedules them
with a random delay, executes with retries, files `backup_failed`, and injects
operation-scoped volume bindings through reserved `_volume` and `_filename` params.
`apps/kopia-s3.seed.rhai` is the reference provider.

That is roughly 2,800 lines across the OI handlers, the runtime, ctl and the web interface,
plus its spec sections, the `backup-snap-` reserved volume namespace, and the operation-scoped
binding machinery entangled with action invocation.

It goes. Backups become something Canopy drives and Seedling performs, rather than a framework
Seedling hosts. The shape is already written down: a report's response carries a list of
backups to run immediately, addressed to whichever source owns backups on that host. Seedling
is never that source today and so never receives a non-empty list. Becoming one is the
integration, and it lands on the same channel as F's control work.

One principle from the fleet's backup arrangement constrains this and should survive it:
Seedling's own state is backed up by something that is not Seedling, so that recoverability
never depends on Seedling being healthy. Whatever Seedling comes to back up, `/var/lib/seedling`
is not it.

### Invariants not to regress

Three behaviours hold today, are relied on by everything above, and are not obviously
load-bearing from the code. Each deserves a test that fails loudly if it changes.

- **Applying a data plane drains rather than cuts.** The DNAT chains are NAT-type, which
  netfilter traverses only for `NEW`-state packets; the apply is a single atomic batch that
  rebuilds the table with no conntrack flush; every chain is `policy: Accept` with an
  established-accept rule first. Connections in flight when routing changes finish against
  wherever they started, and no packets are dropped in between. Adding a conntrack flush, or
  splitting the apply into more than one batch, silently turns every routing change into a
  reset for anyone mid-request.
- **A site ingress with no attachments emits no DNAT rules.** This is what makes an ingress
  declarable ahead of the traffic it will carry.
- **An unscheduled app ingress emits no DNAT rules**, so registration, params, image pulls and
  `apps plan` all work on an app that is not yet receiving anything.

The second and third are the primitives D generalises. Whatever shape the staged takeover
takes, it should not be a fourth mechanism sitting beside them.

Because applying drains, the disruption in a takeover is `apps install` (warm certs, then
`rt.start(app).ready(300)`, plus any provision and migrate jobs), not the routing change.
Warming images ahead of time removes one term, and D removes most of the rest.

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
- **How does a definition reach Seedling from the app's repo, and does it gate the
  migration?** Candidates differ a lot in cost and in what they assume about host
  connectivity: pulling from a release artefact, carrying the definition in the app's own
  container image, or relaying through an existing connection. A migrating host is often on a
  poor link, so anything requiring the host to reach a new external service needs care. The
  migration can ship with definitions still in `apps/` and pick this up after, so the question
  is whether it is a blocker or a follow-on.
- **What shape does the staged takeover take?** A per-app flag, a site-level mode, or an
  explicit `ingresses takeover` operation. This changes the ops stages 2 and 3 enough that
  their step lists get rewritten against what ships rather than adapted to it.
- **What is Canopy allowed to change, and what stops it?** A param set is not a small write:
  `on_change` runs arbitrary script, so "set a param" and "run an upgrade" are the same
  operation. Needs a decision on which params are remotely settable, whether the host can
  refuse, and how this interacts with `operation_in_progress` when Canopy asks for something
  during a lifecycle operation. An operator watching the host should be able to see what
  Canopy asked for and what it caused.
- **Does volume snapshotting survive the backup framework?** Backups rest on point-in-time
  volume snapshots. If the framework goes but Canopy-driven backups still need a consistent
  source, snapshotting is a primitive to keep rather than part of the removal. Settle before
  the removal starts, not during.
- **Is anything depending on backup apps today, and does the removal need a migration?** The
  fleet's app-data backups are not Seedling's yet, which suggests the answer is no and the
  removal can be clean. Worth confirming rather than assuming.
- **How does `bestool-alertd` health-check a Seedling-run Postgres?** Not needed for this
  project, but one of the options is "move the checks inside Seedling and have it report
  them", which overlaps Canopy reporting. Worth knowing before Canopy work is scheduled.