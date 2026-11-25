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

## Open

- Whether the demos take over as test fixtures for `crates/core/src/tests/definitions.rs`, or those tests move to inline scripts.
