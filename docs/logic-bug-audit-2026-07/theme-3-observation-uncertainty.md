# Theme 3: Absence of observation conflated with observation of absence

> Companion to the [logic bug audit](../logic-bug-audit-2026-07.md), cross-cutting theme 3.

## The failure pattern

The observation pipeline has two places where "we could not determine the state" silently
degrades into "the thing is gone", and every consumer downstream trusts the degraded answer:

1. **Failed queries default to all-absent.** `observe_one_pod`
   (`crates/core/src/system/reconcile/pods.rs`) handles an `observer.observe()` error by
   returning a fully actuatable `ObservedInstance` with every flag false — `is_running: false`,
   `container_exists: false`, `network_exists: false`. The struct is shaped so that "no
   information" and "confirmed absent" are the same value. `actuate_one_pod` then runs its Job
   terminal-detection predicate `(!obs.container_exists && !obs.is_running && previously_ran)`
   with no check on `result.observe_failure`, stops the Job, and records it in
   `completed_jobs` — from which `job-terminal.defense` guarantees it is killed again if it
   ever reappears. One transient podman/systemd hiccup (any of the three probes in the
   `tokio::try_join!` in `Observer::observe_pod_instance`) permanently destroys an in-flight
   batch workload.

2. **Unrecognised-but-present states map to "removed".** `parse_container_status`
   (`crates/core/src/system/podman.rs:805`) maps any status string outside its recognised list
   to `ContainerStatus::Unknown` — including podman's real transitional states `"stopping"`,
   `"removing"`, and `"initialized"`. `Observer::observe`
   (`crates/core/src/system/observer.rs:201`) then maps `ContainerStatus::Unknown` to
   `ObservationFact::ContainerMissing`, even though the enclosing match arm has just proven the
   container *exists* (`inspect` returned `Some`). `to_obs_kinds`
   (`crates/core/src/system/types.rs:537`) persists `ContainerMissing` as `container_removed`
   in `world_observations`, which `derive_container_lifecycle`
   (`crates/core/src/runtime/barrier/oracle.rs`) treats as the transition to `Unscheduled`, and
   `DbWorldOracle::termination_success` treats as terminal success in its fallback branch. A
   postgres container draining through a long `stop_timeout_secs` is recorded as removed;
   barriers sequenced after the stop are satisfied while the old container still holds its
   volumes and network.

Both are the same epistemic error: the code has one vocabulary for "state of the world" and
no vocabulary for "quality of the evidence", so missing or unclassifiable evidence is coerced
into the most destructive definite answer.

## Affected findings

| Finding | Section | Severity |
|---|---|---|
| Transient observe failure kills a running Job permanently (H1) | [§12](../logic-bug-audit-2026-07.md#12-system-reconciliation-engine) | high |
| Observer reports a gracefully-stopping container as removed | [§13](../logic-bug-audit-2026-07.md#13-system-actuation-actuator-observer-breadcrumb-journal-stub-types) | medium |
| `parse_container_status` catch-all maps `stopping`/`removing`/`initialized` to `Unknown` (co-located root cause of the above, found independently by the host-integration review) | [§15](../logic-bug-audit-2026-07.md#15-host-integration-podman-systemd-volume_store-confined_write) | medium |

Adjacent but distinct: "Observer can never detect systemd's start-limit-hit"
([§13](../logic-bug-audit-2026-07.md#13-system-actuation-actuator-observer-breadcrumb-journal-stub-types), H4)
is a mis-read of a *successful* query (`SubState` vs `Result`), not an uncertainty coercion —
though its compounding factor (systemd's `CollectMode` garbage-collecting the failed unit so
the observer sees `UnitGone`) is another case where genuine absence and lost evidence
converge on the same fact.

## Would a high-level change help?

**Yes — this is one of the strongest candidates in the audit for a type-driven fix.** Both
instances exist because the types permit the coercion: `ObservedInstance` is a bag of `bool`s
whose zero value means "absent", and `ContainerStatus::Unknown` sits in the same flat enum as
the definite states with no way to force consumers to treat it differently. Fixing only the
two call sites leaves the trap armed for the next probe added to `observe_pod_instance` or the
next status string podman invents.

The honest cost assessment, though, is favourable precisely because the fix does **not** need
to thread a new variant through persistence:

- `world_observations` is an append-only event log of definite transitions; the oracle's
  `derive_lifecycle_state` folds over it. "Unknown this tick" is correctly represented by
  *writing nothing*, which makes the oracle hold the last-known state for free. No new
  `obs_kind` string, no change to the `OBS_KINDS` dedup seed in
  `crates/core/src/system/reconcile.rs`, and no interaction with the frozen-migration rule for
  `crates/core/src/runtime/db.rs`. `observe_one_pod` already returns early on error without
  emitting facts — the persistence layer is already right; only the in-tick actuation view lies.
- The blast radius of the type change is one module boundary: `observe_one_pod` →
  `actuate_one_pod` / `compute_stop_inhibitions`, plus the analogous path in
  `reconcile/volumes.rs` (which has the same `observe_failures` shape).

The one genuinely new mechanism is escalation state (consecutive-unknown counting), which can
live in memory beside the reconciler's other per-tick state (as `PullState` does for images);
losing it on restart is acceptable because a restart re-observes anyway.

## Proposed pattern

**Boundary:** uncertainty is resolved *at* the observer boundary and must not cross it
disguised as fact.

1. **Tri-state per instance.** Replace the `Option<ObservedInstance>` returned by
   `observe_one_pod` with an enum:

   ```rust
   enum PodObservation<'a> {
       /// All probes succeeded; flags are evidence.
       Observed(ObservedInstance<'a>),
       /// At least one probe failed; nothing is known this tick.
       Failed { dr: &'a DesiredResource, error: String },
   }
   ```

   `actuate_one_pod` takes `ObservedInstance` only, so "failure looks like absence" becomes
   unrepresentable — the compiler forces every caller to route `Failed` explicitly. Note
   "Absent" stays inside `Observed`: `ContainerMissing` from a *successful* query is a real
   observation and must keep driving teardown and job-terminal logic. The per-instance
   observation stays atomic (all three probes or nothing): partial results would resurrect the
   bug in a subtler form, e.g. `container_exists: false` because `inspect` failed while the
   network probe succeeded.

2. **Present-but-indeterminate is present.** In `Observer::observe`, the
   `Some(state)` arm has already proven existence, so `ContainerStatus::Unknown` must never
   produce `ContainerMissing`. Map podman's known transitional strings properly in
   `parse_container_status` (`"stopping"`/`"removing"` → a new `ContainerStatus::Stopping`,
   `"initialized"` → `Created`) and map any residual indeterminate-but-present state to a fact
   with an empty `to_obs_kinds` mapping (in-tick only, like `NetworkPresent`) that sets
   `container_exists = true`. Nothing new is persisted; the draining container simply stops
   being recorded as removed.

3. **Per-consumer rules for `Failed`/unknown:**
   - `actuate_one_pod`: never reached — no start, no stop, no `completed_jobs` insertion. The
     spec's `r[autonomous.job-terminal]` phrase "currently observe as gone" becomes literally
     enforced: only an `Observed` value can satisfy it.
   - `compute_stop_inhibitions`: a `Failed` instance is excluded from `current_ready` (it
     cannot vouch for health) and its stale siblings' stops stay inhibited — rolling updates
     pause rather than retire capacity on missing evidence.
   - Lifecycle oracle: unchanged — no facts written means `derive_lifecycle_state` and
     `termination_success` keep the last-known answer.
   - Fault filing: keep the existing `observe_failed` fault path in
     `reconcile/faults.rs`, but gate it on N consecutive failed ticks (say 3) per instance so a
     single blip logs an error without minting an operator-visible fault; clear the counter on
     the first successful observation.

## What it prevents — and what it does not

**Prevents:** the entire class where a query failure or an unmodelled-but-present state
triggers destructive actuation — Job kill-on-blip, premature `container_removed`/`Unscheduled`
lifecycle advancement, barriers releasing resources still held by a draining container, and
any future recurrence when a fourth probe or a new podman state is added.

**Does not prevent:** wrong answers from *successful* queries (H4's `SubState` vs `Result`
mis-read); genuine ambiguity of absence for `--rm` Jobs, where "exited 0 and auto-removed" and
"crashed and auto-removed" both surface as `container_removed` and only the `unit_failed`
secondary signal disambiguates; systemd's `CollectMode=inactive-or-failed` erasing evidence
before the observer runs; or stub-fidelity gaps (§13's stub image-extraction finding). Those
need their own fixes; this pattern just stops uncertainty from masquerading as one of them.

## Migration path

1. Spec first (per repo convention): add an observation-quality requirement under
   `# Observation` in `docs/spec/runtime.md` (see Enforcement) and tighten
   `r[observe.deployment]`'s "missing, created, running, or exited" list to name the
   indeterminate-present case.
2. `parse_container_status`: model podman's full documented state set; delete the `_ =>`
   catch-all (see Enforcement). Unit-testable immediately.
3. `observer.rs`: remove the `Unknown → ContainerMissing` arm; emit present-but-indeterminate.
4. `reconcile/pods.rs`: introduce `PodObservation`, split `Failed` handling out of
   `actuate_one_pod`, adjust `compute_stop_inhibitions`. Mirror in `reconcile/volumes.rs`.
5. Add the consecutive-failure escalation counter and gate the `observe_failed` fault on it.
6. Each step is independently committable and behaviour-improving; no data migration at any step.

## Enforcement

- **Tracey spec requirement.** Add to `docs/spec/runtime.md`, e.g.
  `r[observe.failure-not-absence]`: "A failed observation attempt yields no facts. The runtime
  must not treat a failed observation as evidence of absence: no destructive actuation
  (stop, job-terminal detection, teardown) may be based on an instance whose current-tick
  observation failed, and lifecycle derivation must retain the last successfully observed
  state." Annotate the `PodObservation::Failed` handling and the observer arms with
  `r[impl ...]`, and cover it with `r[verify ...]` tests so `tracey query uncovered` keeps it
  honest.
- **Stub-System fault injection.** The stub backend (`crates/core/src/system/stub.rs`)
  currently cannot fail a probe — which is exactly why the existing tests pass while
  production dies. Add fail-once/fail-N hooks to the stub `ContainerRuntime`/`ProcessManager`
  (`inspect`, `network_exists`, `unit_state`) and add reconcile-loop tests: (a) a Job with
  `container_running` persisted whose observation errors for one tick — assert no stop is
  issued and the Job is not in `completed_job_instances`; (b) a stub container reporting a
  `"stopping"`-derived state — assert no `container_removed` observation is produced and the
  oracle still reports the pre-stop state; (c) N consecutive failures — assert the
  `observe_failed` fault appears only after the threshold and clears on recovery.
- **Exhaustive-match discipline.** Parse podman status strings into an enum listing every
  state podman documents (`created`, `configured`, `initialized`, `running`, `paused`,
  `exited`, `stopped`, `dead`, `stopping`, `removing`, `unknown`) with no silent catch-all:
  the residual arm should log an error naming the unrecognised string and, per the repo's
  log-error-implies-fault rule, file a fault — so the next new podman state fails loudly in
  one place instead of quietly becoming "removed" three layers away. Inside Rust, matches on
  `ContainerStatus` should avoid `_` arms so adding a variant breaks compilation at every
  consumer that must decide what it means.
