# Healthcheck-driven replace and health-aware routing

## Context

Healthchecks landed earlier in this branch as observation + fault filing only. Two gaps remain:

1. **Routing.** Service backend pools include any running pod, regardless of health. A pod that started 100ms ago but isn't yet serving requests, or a pod that's gone unhealthy mid-flight, both still receive traffic. This contradicts the spec at `r[lifecycle.service]` ("at least one backend is healthy and traffic can be routed").
2. **Failure response.** `on_failure: kill | restart | stop` delegates the disruptive action to podman. For a single-server platform with no substrate failover, killing a backend without a ready replacement is the wrong move — it just removes capacity. Better: spawn a replacement first, swap traffic when it's healthy, retire the old. If the replacement also fails, give up gracefully and keep serving from the (degraded) original.

This plan reframes `on_failure` as a seedling-managed swap-then-retire flow and updates routing to prefer healthy backends, with a controlled fallback to "anything running" when nothing is healthy.

## Behaviour model

A healthcheck declared on a deployment means: "the platform should know when this is ready and act on its health." Two policies:

- **`replace` (default, implicit when `.healthcheck()` is declared)**: when an instance is observed unhealthy, the reconciler spawns a replacement alongside it. The original keeps serving (degraded, possibly partial) until the replacement is healthy. When healthy, traffic shifts and the original is retired. If the replacement *also* fails healthcheck without ever passing, the reconciler stops the cycle, keeps the original serving, and files a hard `health_check_replace_failed` fault for operator attention.
- **`monitor` (opt-out)**: observe and gate routing on health, but no automatic replacement. Operator-driven recovery.

The replace-loop reset triggers:

- **AppDef generation change** (operator pushes new code → fresh slate). Implemented in this round.
- **Operator-cleared fault** — design hook only; no UI/CLI surface yet to invoke it. The clearing path will reset the counter automatically once a clear endpoint exists.

Routing rule: **prefer healthy, fall back to anything running**.

- A pod enters the routing pool the first time it observes healthy.
- A pod leaves the pool when it goes unhealthy *if* a healthy sibling exists.
- If no healthy sibling exists, unhealthy pods stay in the pool (some traffic may still succeed) and a degraded fault is filed.
- Pods in start-period (running but never observed healthy yet) are excluded from the pool — readiness gating.

## Non-goals

- **Healthchecks on Jobs.** Disallowed at BSL time. Add support later if there's demand; the semantics are murky for one-shot workloads.
- **Operator-clearable replace-loop reset via UI/CLI.** Hook in the code, but no surface to invoke it yet. Comes when the broader fault-clear UI lands.
- **Connection draining tuning.** Caddy handles its own; out of scope.
- **Distinct readiness vs liveness probes.** Single-probe model with `replace` policy covers the seedling use case; the reserved `kind:` field leaves room for `kind: "http"`/`"tcp"` later.

## BSL surface

```rhai
app.deployment("web")
    .image("...")
    .healthcheck(#{
        kind: "command",
        cmd: ["curl", "-fsS", "http://localhost/health"],
        interval: 5,
        timeout: 2,
        retries: 3,
        start_period: 10,
        on_failure: "replace",   // default; "monitor" is the opt-out
    });
```

`on_failure` accepts only `"replace"` or `"monitor"`. The previous `"kill" | "restart" | "stop"` values are removed (those healthchecks shipped earlier today; the example app is the only consumer).

`.healthcheck(...)` on a `Job` throws at BSL evaluation.

## Spec changes

`docs/spec/language.md`:
- Update `l[container.healthcheck.on-failure]` to enumerate the two values and document the swap-then-retire / monitor-only semantics.
- New rule `l[container.healthcheck.deployment-only]`: declaring a healthcheck on a Job is an error.

`docs/spec/runtime.md`:
- New `r[autonomous.healthcheck-replace]`: when an instance with `on_failure: replace` is observed unhealthy, the reconciler must spawn a replacement before retiring the unhealthy one.
- New `r[autonomous.healthcheck-replace.guard]`: if a replacement instance itself fails to become healthy before reaching its own `start_period + retries × interval`, the reconciler must stop attempting further replacements for that deployment until the AppDef generation changes.
- New `r[fault.healthcheck-replace-failed]`: hard fault filed when the replace-loop guard trips.
- Update `r[lifecycle.service]` to spell out the prefer-healthy-fall-back-to-running pool semantics.
- New `r[fault.service-degraded]`: fault filed for a service whose pool contains only unhealthy backends.

## Implementation layering

### 1. Defs / BSL
- `crates/core/src/defs/container.rs`: shrink `HealthcheckOnFailure` to `{Monitor, Replace}`, change default to `Replace`, update the parser to accept only the two strings.
- `crates/core/src/defs/app/job.rs`: detect `.healthcheck()` invocation in the Job context (the `ContainerDef` mixin sees this). Easiest: don't include the healthcheck builder in the Job mixin path. Job's `CustomType::build` calls `PodDef::mixin` which calls `ContainerDef::mixin`. Solution: add a parameter to `ContainerDef::mixin` that controls whether `.healthcheck()` is exposed; pass `false` from Job, `true` from Deployment.

### 2. System / podman
- `crates/core/src/system/types.rs`: shrink `HealthCheckOnFailure` enum to `{None, Replace}` ... actually drop `on_failure` from `HealthCheckSpec` entirely if podman is always told `none`. Simpler: rename to a single `monitor: bool` flag if anything; the Replace flow is the runtime's job, not podman's.
  - Decision: drop `HealthCheckOnFailure` from `HealthCheckSpec`. Always pass `--health-on-failure=none`.
- `crates/core/src/system/translate/container.rs`: emit `--health-on-failure none` unconditionally when a healthcheck is declared.

### 3. Reconciler state
- `crates/core/src/system/reconcile.rs`: add fields:
  - `unhealthy_deployments: HashSet<(AppName, String)>` — per tick, indicates a deployment has at least one unhealthy instance with `on_failure: replace` and a replacement is wanted.
  - `replacement_failures: HashMap<(AppName, String), ReplacementGuard>` — persistent across ticks. Each entry records the most recent replacement attempt's instance ID and a flag for whether it failed.
- `compute_effective_scales`: when a deployment is in `unhealthy_deployments` and *not* in the failed-replacement set, bump effective by 1 (mirroring the existing rolling logic).

### 4. Health-aware keep selection
- `crates/core/src/runtime/registry.rs`: extend `ensure_scaled_group` (or its caller `compute_steady`) to receive per-instance health status and prefer healthy instances when assigning to `keep`. Sort by `(is_currently_healthy desc, created_at asc)`.
- This is the first step where the desired-state computation needs observation data. Plumbing: snapshot health per instance in `snapshot_all_apps` (DB query for last health observation per instance) and pass to `compute_steady`.

### 5. Stop inhibition
- `crates/core/src/system/reconcile/pods.rs::compute_stop_inhibitions`: extend to recognise unhealthy-being-replaced instances. Rule: an unhealthy instance can be stopped *only if* a healthy sibling exists in the same deployment group. Otherwise inhibit (keep serving in degraded mode).

### 6. Replacement-failure detection + hard fault
- New module or section in `crates/core/src/system/reconcile/`: track per-deployment replacement attempts.
  - When a new instance is created (during a replacement bump), record its ID and start time.
  - Each tick, check: has this instance ever been observed healthy? If not, and we're past `start_period + retries × interval` (read from the `HealthcheckDef`), declare the replacement failed.
  - On failed replacement: file `health_check_replace_failed` fault, set the deployment's `replacement_failures` entry to `Failed`. Subsequent ticks: don't bump effective scale.
  - When the AppDef generation changes for the app, clear the deployment's entry. Hook: snapshot_all_apps already loads `current_generation`; compare against last-seen generation.

### 7. Routing — prefer healthy with fallback
- `crates/core/src/system/reconcile/rules.rs::collect_service_backends` (and any siblings building backend lists for proxy/services): for each backend group, partition by health. If any healthy, use only those. If none healthy, use all running and emit a `service_degraded` fault for the service.
- `RunningPod` itself can stay shape-compatible — add a `is_healthy: bool` field populated from the same observed_healthy flag we already plumb. The filtering happens at backend-collection time.

### 8. Service Ready
- `crates/core/src/runtime/barrier/oracle.rs::derive_service_lifecycle`: today maps `backend_healthy` to Ready and stays there. Consider: emit `backend_unhealthy` (or `service_degraded`) observations from the routing layer when the pool degrades; oracle uses them to demote Ready → Scheduled. Punt this for the first round if it adds too much surface — the fault carries the operator-visible signal regardless.
  - Decision: punt for round one. The fault is the primary signal; lifecycle Ready stays sticky. Re-evaluate after.

## Files to modify

Spec:
- `docs/spec/language.md`
- `docs/spec/runtime.md`

Defs / BSL:
- `crates/core/src/defs/container.rs`
- `crates/core/src/defs/container/tests.rs`
- `crates/core/src/defs/app/job.rs` (no-healthcheck path)

System:
- `crates/core/src/system/types.rs`
- `crates/core/src/system/translate/container.rs`
- `crates/core/src/system/translate/container/tests.rs`

Reconciler:
- `crates/core/src/system/reconcile.rs` (state, generation reset, replacement tracking)
- `crates/core/src/system/reconcile/pods.rs` (stop inhibition, replacement detection)
- `crates/core/src/system/reconcile/rules.rs` (routing health gate)
- `crates/core/src/system/reconcile/state.rs` (health snapshot for desired state, possibly)
- `crates/core/src/system/reconcile/faults.rs` (health_check_replace_failed, service_degraded)

Registry:
- `crates/core/src/runtime/registry.rs` (health-prefer keep selection)
- `crates/core/src/runtime/desired.rs` (plumb health into ensure_scaled_group)

Example:
- `nodejs.seed.rhai` — already uses `on_failure: "restart"`, change to `"replace"` (or omit, since it'll be the default).

## Verification

Static:
- `cargo fmt`, `cargo clippy --all-targets`, `cargo test --workspace`.
- `tracey query status` clean across all four specs.

Live:
1. Existing daemon picks up the new binary via watchexec.
2. The current `node` app already has a healthcheck declared. With the BSL update, `on_failure: "restart"` will need to be migrated to `"replace"` (or just dropped — replace is the default).
3. Watch one healthcheck-fail cycle: confirm a replacement instance is spawned, healthy replacement comes up, old is retired, no traffic interruption.
4. Force a permanently-failing healthcheck (cmd: ["false"]) and confirm:
   - Replacement is spawned once.
   - Replacement also fails, hard fault filed.
   - No further replacements; degraded mode (traffic still flowing to original).
   - Push a script change → fault remains, but next replacement attempt fires (counter reset on generation bump).
5. Confirm routing: with two instances at scale=2 where one is unhealthy, traffic only goes to the healthy one. With both unhealthy, traffic still flows but `service_degraded` fault appears.

## Follow-ups (not in this round)

- Operator-clearable replace-loop reset (waits on fault-clear UI/CLI).
- Distinct readiness vs liveness via `kind: "http"` etc.
- Healthchecks on Jobs.
- Service Ready state demotion when pool fully degrades.
