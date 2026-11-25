# Q6: Replace sample apps with demo applications

## Goal

Replace `apps/postgres.seed.rhai`, `apps/tamanu-central.seed.rhai` and `apps/tamanu-facility.seed.rhai` with definitions that read plainly as demos of Seedling, not as production deployments.
Each demo exists to show and exercise a slice of BSL and the runtime.

## Demo lineup

- **hello-world**: one deployment, one HTTP service, one ingress on a hostname param. The smallest working app.
- **http-tour**: one HTTPS ingress whose HTTP service shows each HTTP feature on its own route: route redirects, header operations, compression, rate limiting, balancing across replicas, redirect to HTTPS. A request-echo image makes each route's effect visible.
- **protocol-tour**: one ingress per termination pairing: plain TCP passthrough, TLS to TCP, DTLS to UDP, HTTPS to HTTP/1.1, HTTPS to cleartext HTTP/2. Each backed by a small echo server.
- **actions-and-gates**: gates with keys and start ordering, a gate re-running when its key moves, actions with a param schema, a shell, a scheduled action, a job.
- **paired apps** (provider and consumer): an exported service and an exported volume consumed by a second app.
- **bundle-tour**: every bundle feature: several script files in a `script` order, sidecar files read with `app.file` and `app.dir` and written into a volume, the metadata table read with `app.metadata()`, a `seedling` version requirement, and files pulled in from `demos/shared/` through `include`.
- **bundle-share**: a small second demo including the same files from `demos/shared/` (a helper script and a static page), so `include` is shown actually shared between bundles.

- **params-tour**: every parameter kind, secret params, required params and defaults, validators.
- **workloads-tour**: deployment scale, update and termination strategies, health checks (HTTP and command) with timing and failure response, priority, container options (command and args, env, resource limits, privileges, working directory, stop signal and timeout), sizing from host constants, a job with instances and a deadline, every volume kind (storage options, declared file writes, an external volume).
- **language-tour**: the Seedling stdlib helpers no other demo already uses naturally: encoding, structured formats, regex, URL, IP, purl, version parsers, durations, quoting, random, collections.

Placed into the demos above as well:

- **hello-world** also sets `canopy_type` and `app.description`, nothing more.
- **protocol-tour** also: an ingress for a site ingress to attach to. Installing it twice under different app names with the same hostname shows `ingress_conflict`.
- **paired apps** also: an external service, and a service mount across apps.
- **actions-and-gates** also covers the whole runtime handle: starting, stopping and querying resources, deployment restart, certificate and image warming, signals, command execution, volume writes, and waiting on started, barriers, termination and executed.

Each demo covers every feature of its area, not a sample of it.
The stdlib is covered once across the set: a helper a tour uses naturally counts, and language-tour picks up the rest.

## Layout

- `apps/` goes; demos live under `demos/<name>/`, each a definition bundle: `seedling.toml` plus `app.seed.rhai`, and sidecar files where the demo uses them.
- Each demo's header comment and `app.description` say it is a demo and what it shows.
- The paired apps are two bundles, `demos/paired-provider/` and `demos/paired-consumer/`.

## Params and hostnames

Every demo installs with no params set: each param has a default, except the required param params-tour exists to show.
Hostnames default to `<demo>.demo.internal`: `.internal` is reserved for private use and never resolves publicly, and an internal name takes its certificate from the proxy's internal CA with no TLS policy to set.

## Images

Demos use well-known public images pinned by tag (e.g. `traefik/whoami` for HTTP echo, a small base image running `socat` for TCP/UDP echo).
Check each supports what its route needs, h2c in particular, before relying on it.
No demo image is built or published from this repo.

## Demos as test fixtures

The demos replace the current definitions as the fixtures in `crates/core/src/tests/demos.rs`, so they keep evaluating as the language changes.
Every demo gets at least an evaluation test.
Fixtures load as bundles rather than `include_str!` of one script, so a demo's sidecar files and metadata are part of what is tested.
Demos using `include` are assembled the way the CLI assembles them; the assembly moves from `crates/ctl/src/definition.rs` into `seedling-bundle`, behind its `source` feature, where both ctl and the core tests call it, rather than being reimplemented for the tests.

The gate tests carry over to actions-and-gates, which must exercise what they assert today:

- a gate exclusive with a deployment (stopped for the run)
- a gate with a check job and an interval
- `after` ordering between gates, and deployments ordered after gates
- gate keys that move with some params and not others
- a gate body that branches on first run vs later runs, observed under a probe
- a gate body that fails on a refused transition (today: a postgres downgrade)
- actions whose bodies run under a probe

The Tamanu name-quoting and missing-hostname tests test those definitions alone, and go with them.

## Other changes

- [x] Delete the stale copy of the "Seedling in production" PRD from this branch; the live one is on its project branch.
- [x] README's "Apps" section points at the demos.
- [x] `docs/dev-vm.md`: the "Bringing up the postgres app" walkthrough becomes a walkthrough of actions-and-gates (external volume attach, params, install, gate runs).
- [x] `docs/deploying.md` and `docs/bsl-scripting.md`: example app names become demo names; postgres snippets stay where postgres is the reason for the advice (e.g. SIGINT as fast shutdown).

No spec changes: the demos are repo content, not product behaviour.

## Explicitly rejected

- Loading the demos as host templates, or any tooling that ships them to a host. It would present them as ready-to-deploy apps, the confusion this card removes.
