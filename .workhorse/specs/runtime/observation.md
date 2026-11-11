---
id: OBS
---

# Observation

## Observation

<!-- from runtime: Observation -->

> r[observe.facts]
> The runtime must collect timestamped observation facts for each resource instance by inspecting the backing system primitives.

> r[observe.deployment]
> For Deployment and Job resource instances, the runtime must observe pod network presence, container lifecycle state, and systemd unit state.
> The container lifecycle states are: missing, created, running, exited, and present-but-indeterminate — the last covering a container that exists but whose reported state corresponds to no lifecycle transition, such as one still shutting down.

> r[observe.failure-not-absence]
> A failed observation attempt yields no facts.
> The runtime must not treat a failed observation as evidence of absence: no destructive actuation — stopping, terminal-state detection, or teardown — may be based on an instance whose observation failed this iteration, and lifecycle derivation must retain the last successfully observed state.
> An observation attempt for one instance either succeeds in full or fails in full; partial results must not be reported as fact.
> Likewise, a state that is observed but not recognised is evidence that the thing exists, never that it is absent.
> A single failed observation must not by itself raise an operator-visible fault; a persistent one must.

> r[observe.volume]
> For Volume resource instances, the runtime must observe whether the named volume exists.

> r[observe.volume.backend-mismatch]
> When observing a named non-tmpfs volume, if the volume exists but its storage backend does not match the current configuration (e.g. a plain directory exists but BTRFS subvolumes are required, or vice versa), the observer must report this as a backend mismatch rather than as a present volume.

> r[observe.ingress]
> For Ingress resource instances, the runtime must observe whether the proxy is reachable.

> r[observe.ingress.certs]
> For each TLS-terminating ingress, the runtime must observe the certificate status for each declared hostname.
> Statuses are:
>
> - `none`: no certificate has been acquired and no acquisition is in progress.
> - `pending`: certificate acquisition is in progress.
> - `valid`: a non-expired certificate is cached and usable. The expiry timestamp must also be observable.
> - `failed`: certificate acquisition has failed; the most recent error must be observable.
>
> Cert observation drives both the [`Ready` lifecycle state](rule:r/lifecycle.ingress) of an ingress and the satisfaction of [`rt.warm_certs`](rule:l/rt.warm-certs) barriers.

> r[observe.persist]
> After each observation pass, the runtime must persist the resulting facts to the
> `world_observations` table as `obs_kind` string entries so that the barrier oracle
> can derive lifecycle states and satisfy barriers. Each `(instance, obs_kind)` pair
> must be written at most once per runtime session to prevent unbounded table growth.
> Service lifecycle facts (`network_created`, `backend_healthy`) must be emitted by
> the routes phase based on running backends. Ingress lifecycle facts
> (`ingress_configured`, `ingress_ready`) must be emitted by the proxy phase only
> after a successful configuration apply. Stop and cleanup facts (`stop_sent`,
> `network_removed`, `network_cleaned_up`, `ingress_removed`, `ingress_cleaned_up`)
> must be emitted for resources whose desired state is `Unscheduled`.
