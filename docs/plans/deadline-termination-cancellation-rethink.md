# Deadline-less barrier blocking for long-running actions

## Context

Some actions legitimately run for hours with unpredictable duration — backups, bulk restores, maintenance jobs. Today every barrier call (`.terminated()`, `.ready()`, etc.) requires a finite deadline. Users work around this by guessing a large number (`apps/kopia-s3.seed.rhai` passes `AN_HOUR`), which is a lose-lose: guess too low and the action spuriously fails when a backup genuinely takes 90 minutes; guess too high and real hangs take ages to surface.

We want a deliberate, named escape hatch for the genuinely-unbounded case — scoped narrowly to where it makes sense (`.terminated()` and `.ready()`), with supporting infrastructure so that an unbounded wait is observable, cancellable, and cheap to sustain.

## Current state

- `Started::check_barrier(required, deadline_secs: u64)` at `crates/core/src/runtime/barrier/runtime.rs:754–882` enforces deadlines via `now.saturating_sub(started_at) >= deadline_secs`.
- `BarrierCondition`/`BarrierRecord` at `crates/core/src/runtime/barrier.rs:28–63` store `deadline_secs: u64` (required).
- The four state methods are wired on `Started` in `runtime.rs:908–968`; each has a no-arg overload (default 30s) and an `(i64)` overload that clamps negatives via `d.max(0) as u64`.
- The action runner in `crates/core/src/oi/handler/actions/lifecycle.rs:110–168` runs the closure on `spawn_blocking`, and on `OperationResult::Suspended` it `thread::sleep(2s)` then re-runs from scratch. No cancellation path exists.
- Barrier state persists across restarts: `BarrierRecord.started_at_secs` is serialised into the action log and `check_barrier` correctly uses the original-wait timestamp on replay.
- Per-app single-active-operation is enforced at `crates/core/src/runtime/scheduler.rs:94+`; long waits block the owning app's queue but not the rest of the system.
- Spec: `r[barrier.deadline]` at `docs/spec/runtime.md:552–554` asserts every barrier has a deadline; `l[rt.started.state-methods]` at `docs/spec/language.md:817+` asserts deadline must be a positive integer.
- Backup example: `apps/kopia-s3.seed.rhai` uses `AN_HOUR` for `.terminated()` on kopia snapshot runs.

## Design

### API

Add two new Rhai methods on `Started`:

- `.terminated_eventually()` → `Termination` — waits indefinitely for all resources to reach `Terminated`. Mirrors `.terminated()` but with no deadline arm. Pairs with `.ensure_success()` as usual.
- `.ready_eventually()` → `Started` — waits indefinitely for all resources to reach `Ready`. Useful for cert provisioning that can take minutes under Let's Encrypt rate-limiting.

Not added: `.scheduled_eventually()`, `.running_eventually()`. Those barriers protect k8s-scheduling correctness signals; indefinite waits there would hide real bugs. Explicit non-goal.

### Default deadline bump for `.terminated()`

Change the no-arg `.terminated()` overload's default from 30s to 6 hours (21600s). Rationale: `.terminated()` is routinely used on Jobs that can run long, and 30s almost never matches real-world usage. A 6-hour default is a "reasonable upper bound for almost anything that still should deadline" — users who truly need unbounded semantics opt in to `.terminated_eventually()`. Leave `.scheduled()`/`.running()`/`.ready()` defaults at 30s.

### Dynamic poll backoff

Replace the fixed `thread::sleep(Duration::from_secs(2))` at `lifecycle.rs:165` with a dynamic interval driven by how long the current operation has been suspended on *any* barrier. Applies uniformly to bounded and unbounded waits.

Shape (tunable, approximately):

```
waited < 2 min   → 2s
2 min … 1 hour   → linear ramp 2s → 30s
1 hour … 6 hours → linear ramp 30s → 300s
> 6 hours        → 300s
```

Implementation: compute elapsed from the earliest unsatisfied `BarrierRecord.started_at_secs` for this operation at each suspension tick, and feed it through a piecewise function.

The sleep must be interruptible so that (a) cancellation wakes it immediately and (b) `tick_notify` (already signalled from reconcilers on observation updates) can wake it early when there's likely progress. Use a `tokio::sync::Notify` or a `parking_lot::Condvar` with timeout, driven from the `tick_notify` already threaded into `OperationContext`.

### Cancellation

Add first-class operation cancellation so an unbounded `.terminated_eventually()` always has an escape hatch.

1. **Cancel token.** Add a `CancelToken` (thin wrapper around `Arc<AtomicBool>` + a `Notify`) to the active-operation record held by the scheduler.
2. **Endpoint.** Add a "cancel current operation" handler under `crates/core/src/oi/handler/actions/` (next to `lifecycle.rs`/`install.rs`); wire it through the existing QUIC handler plumbing used by other action endpoints. Flips the token and notifies.
3. **Cooperative check.** At the top of `check_barrier` (`runtime.rs:754`), check the cancel token and return a distinct error (`Err(make_cancel_error())`) that the lifecycle loop translates to a new `OperationResult::Cancelled` variant — parallel to `Failed` but with its own fault kind (`operation_cancelled`) and log phrasing.
4. **Wake the sleep.** The cancel token's `Notify` also breaks the interruptible sleep in the lifecycle loop so cancellation takes effect at most one tick later, not up to 5 minutes later.
5. **Persistence.** A pending cancel request should survive daemon restart — persist a `cancel_requested` flag next to `current_operation` so a cancel issued right before a restart still takes effect on replay.
6. **Cleanup.** Cancellation still goes through the existing `cleanup_dynamic_resources` path (`lifecycle.rs:171+`) so dynamic resources are torn down as for any other terminal outcome.

### Barrier data structure changes

- Change `BarrierCondition.deadline_secs: u64` → `Option<u64>` (None = indefinite).
- Change `BarrierRecord.deadline_secs: u64` → `Option<u64>`. Add `#[serde(default)]` so older persisted rows (which always had a value) still deserialise; only write the new shape going forward. Since these rows only live for the duration of an active operation (cleared on Complete/Fail), migration risk is minimal.
- `check_barrier` signature becomes `check_barrier(required, deadline: Option<u64>)`. The deadline-arm becomes `if let Some(d) = deadline && now.saturating_sub(started_at) >= d { throw }`.
- Existing `(i64)` overloads keep calling `check_barrier(..., Some(d.max(0) as u64))`; new `_eventually` overloads call `check_barrier(..., None)`.

### Observability

Surface long-running waits so operators notice stuck unbounded barriers without waiting for a deadline:

- Emit a periodic `tracing::warn!` (throttled, e.g. every 10 min) when an operation has been suspended on a single unbounded barrier for a long time. Tag with `operation_id`, app, action, resource, elapsed.
- Include `elapsed_in_barrier` and `deadline_secs: Option<u64>` in the operations/in-progress view surfaced to the web UI (wherever `active_progress` is currently rendered).
- Log/fault taxonomy: `operation_cancelled` (new fault kind) distinct from `operation_failed`.

### Spec changes

- **Modify** `r[barrier.deadline]` at `docs/spec/runtime.md:552`: "Each barrier has an optional deadline. When a deadline is set and the condition is not satisfied within it, the barrier must throw. When the deadline is absent, the barrier waits indefinitely."
- **Modify** `l[rt.started.state-methods]` at `docs/spec/language.md:817`: deadline "must be a positive integer number of seconds" remains the rule for the existing methods.
- **New** `l[rt.started.terminated-eventually]`: defines `.terminated_eventually()` — no deadline; resumes only on terminal state or operator cancellation.
- **New** `l[rt.started.ready-eventually]`: defines `.ready_eventually()` similarly for the `Ready` state.
- **New** `r[barrier.suspension.poll-backoff]`: replay cadence is dynamic (short when freshly suspended, long for protracted waits). Not wall-clock prescriptive — states intent.
- **New** `r[operation.cancel]`: operators may cancel an in-progress operation; the runtime must wake any suspended barrier and produce a terminal `Cancelled` outcome with cleanup equivalent to `Failed`.
- **Update** `l[const.default-deadline]` and any spec mention of the 30s default for `.terminated()` to reflect the 6-hour default (or extract the per-method defaults into a small table).
- Bump `apps/kopia-s3.seed.rhai` to use the new methods where appropriate (backup action → `.terminated_eventually()`; maintenance → keep bounded).

## Critical files

- `crates/core/src/runtime/barrier.rs` — `BarrierCondition`, `BarrierRecord`, serde defaults.
- `crates/core/src/runtime/barrier/runtime.rs` — `check_barrier` signature, new Rhai fn registrations, default deadline constants, cancel-token check.
- `crates/core/src/runtime/barrier/replay.rs` — deserialising optional deadlines.
- `crates/core/src/oi/handler/actions/lifecycle.rs` — dynamic poll backoff, interruptible sleep, `OperationResult::Cancelled`, cancel-token plumbing.
- `crates/core/src/runtime/scheduler.rs` — hold the cancel token on the active operation, expose cancel API.
- `crates/core/src/oi/handler/actions/` — new handler file for the cancel endpoint (mirroring `install.rs`/`lifecycle.rs` structure).
- `crates/core/src/runtime/faults.rs` (or wherever fault kinds live) — register `operation_cancelled`.
- `docs/spec/runtime.md`, `docs/spec/language.md` — spec updates above.
- `apps/kopia-s3.seed.rhai` — call-site update.

## Tradeoffs recorded (so future-me remembers)

- **Scope restricted to `.terminated()` and `.ready()`** — chosen over "all four methods" to preserve `.scheduled()`/`.running()` as correctness signals. Reversible if real demand appears.
- **Separate method name over sentinel** — `.terminated_eventually()` is slightly more verbose than `.terminated(0)` or `.terminated(-1)` but self-documents. Rejected `0` (already means "fail immediately") and `-1` (magic, requires removing the existing `d.max(0)` clamp, no inherent meaning).
- **`Option<u64>` in data structures vs. `u64::MAX` sentinel** — `Option` is clearer at the rust level and forces call sites to handle the None arm explicitly. The serde `#[serde(default)]` story is clean because barrier records don't outlive the operation they belong to.
- **Dynamic backoff applied to bounded waits too** — simpler than two policies. A 5.5-hour bounded wait pays the same poll-cost amortisation as an unbounded one. No downside because the reconciler's `tick_notify` still pre-empts when observations change.
- **Cancellation bundled in, not follow-up** — without it, an unbounded `.terminated_eventually()` has no operator-initiated exit. That would be a bad default state to ship even briefly.

## Verification

- `cargo test -p core` — existing barrier tests still pass; new tests below.
- `cargo clippy` and `cargo fmt` clean.
- `tracey query status` — no regressions; new spec items covered.
- New unit tests in `crates/core/src/tests/barrier.rs`:
  1. `.terminated_eventually()` on a resource that never terminates: `check_barrier` returns `BarrierHit` with `deadline_secs: None` in the recorded condition; after simulated-clock advance of 100 years, still `BarrierHit`, never a timeout error.
  2. Replay: persist an `action_log` entry with `deadline_secs: None`, restart-replay, confirm it resumes cleanly once the oracle reports `Terminated` and that `ensure_success` threads through.
  3. Cancellation from suspended state: start a fake long-running action, flip the cancel token, assert the operation exits within one tick with `OperationResult::Cancelled` and that dynamic resources are cleaned up.
  4. Dynamic poll backoff: drive the lifecycle loop with a mock clock, confirm sleep intervals transition through the expected bands (2s / 30s / 300s).
- Manual smoke: run `apps/kopia-s3.seed.rhai` (or a minimal synthetic "sleep-forever" job), kick off the action, issue a cancel via the new endpoint, confirm the operation terminates promptly and fault is filed as `operation_cancelled`.
