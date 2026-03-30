# Barrier Engine Implementation Plan (Item 7)

This plan covers the barrier/suspension engine: the mechanism by which `rt.start()`,
`rt.stop()`, `rt.reconcile()`, and the `.scheduled()/.running()/.ready()/.terminated()`
barrier methods on `Started` become real, blocking-from-the-script's-perspective
operations, implemented via replay rather than VM suspension.

This is the core of what makes Beset a runtime rather than a script validator.

---

## The Problem

Rhai has no native coroutine or continuation support. You cannot suspend a
running Rhai closure and resume it from the host side. So when the spec says
"`.ready()` blocks until the resource is Ready", we need a different mechanism.

## The Solution: Replay With a BarrierHit Exception

The approach:

1. Run the action closure from the top.
2. Each `rt.*` call checks a **replay log** (the action execution log from
   item 3). If this call has already been recorded (we are replaying), the
   call is idempotent: it updates the desired state but does not re-issue
   real-world operations.
3. When a barrier method (`.scheduled()`, `.running()`, `.ready()`,
   `.terminated()`) is reached:
   - Check whether the barrier condition is already satisfied (by querying the
     world observation state, or the in-memory equivalent used in tests).
   - If **satisfied**: return immediately. The closure continues.
   - If **not satisfied**: throw a special `BarrierHit` error. This unwinds the
     Rhai VM back to the host. The host records the pending barrier condition
     and waits for the reconciliation loop to satisfy it.
4. When the reconciliation loop determines the barrier is satisfied, it
   re-invokes the closure. The closure replays from the top (steps 1–3 again),
   fast-forwarding through already-satisfied barriers, until it reaches the
   previously-pending barrier, which now passes (or a later one which doesn't).
5. Repeat until the closure returns normally (no more `BarrierHit`).

From the script's point of view, `.ready()` blocked. From the host's point of
view, the Rhai VM ran to completion multiple times, each time getting a little
further.

### Why This Works

BSL closures have no side effects beyond `rt.*` calls. Given the same AppDef
and the same parameters, re-execution produces the same sequence of `rt.*`
calls up to any barrier point. `rt.*` calls are idempotent: calling
`rt.start(frontend)` twice does not start two frontends. So replaying is safe.

---

## Components

### A. `ReplayContext` — the shared state carried by `RuntimeInstance` and `Started`

```rust
pub struct ReplayContext {
    // Identity of the current lifecycle operation.
    pub operation_id: OperationId,

    // Sequential index of the next rt.* call in this execution pass.
    // Incremented on every rt.start(), rt.stop(), rt.reconcile() call.
    // Reset to 0 at the start of each replay pass.
    pub call_index: usize,

    // The log entries already committed for this operation (loaded at replay start).
    // Indexed by call_index. If call_index < committed.len(), this call is a replay.
    pub committed: Vec<ActionLogEntry>,

    // New entries written in this pass (not yet committed to the DB).
    // At the end of a successful pass (or barrier hit), flushed to the DB.
    pub pending: Vec<ActionLogEntry>,

    // The barrier condition that was pending when we last hit BarrierHit, if any.
    // Used to check whether we should fast-forward or re-throw on this pass.
    pub pending_barrier: Option<BarrierCondition>,

    // The world state oracle: given a ResourceInstance and a LifecycleState,
    // returns whether the resource has reached (or passed) that state.
    // In production this queries the world observation history.
    // In tests this is an injected in-memory map.
    pub world: Arc<dyn WorldStateOracle>,
}
```

`RuntimeInstance` and `Started` both hold `Arc<Mutex<ReplayContext>>`, so they
share the same context within one closure execution.

### B. `WorldStateOracle` — injectable world state for testing

```rust
pub trait WorldStateOracle: Send + Sync {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> LifecycleState;
}
```

In production: queries the world observation history DB.
In tests: a simple `HashMap<ResourceInstance, LifecycleState>` wrapped in a
struct that implements this trait.

This is the key seam that makes the barrier engine fully testable without any
system calls.

### C. `BarrierHit` — the internal control-flow exception

```rust
pub struct BarrierHit {
    pub condition: BarrierCondition,
}

pub struct BarrierCondition {
    pub resources: Vec<ResourceInstance>,
    pub required_state: LifecycleState,
    pub deadline_secs: u64,
}
```

`BarrierHit` is returned from barrier methods as `Err(Box<EvalAltResult>)` using
Rhai's `EvalAltResult::ErrorRuntime`. The host catches it specifically, by
inspecting the error's dynamic value. BSL scripts cannot catch it (it is not a
BSL-level error; any `try/catch` that catches it at script level must re-throw
it, which we enforce by checking in the catch handler — see section on
`try/catch` interaction below).

### D. `ActionLogEntry` — what gets recorded per `rt.*` call

```rust
pub struct ActionLogEntry {
    pub call_index: usize,
    pub call_kind: CallKind,           // Start | Stop | Reconcile | Query
    pub resources: Vec<ResourceInstance>,
    pub barrier: Option<BarrierRecord>,
}

pub struct BarrierRecord {
    pub required_state: LifecycleState,
    pub deadline_secs: u64,
    pub satisfied_at: Option<SystemTime>,
}
```

### E. Updated `RuntimeInstance` and `Started`

`RuntimeInstance` becomes:

```rust
pub struct RuntimeInstance {
    ctx: Arc<Mutex<ReplayContext>>,
}
```

The `rt.start(resources)` implementation:
1. Acquires the context.
2. Reads `call_index` and checks if `call_index < committed.len()`.
   - If **replaying**: assert the log entry matches (same resources, same kind).
     Add resources to the desired state set. Increment `call_index`. Return a
     `Started` carrying the same context and recording which resources this
     call covered.
   - If **live**: write a new `ActionLogEntry` to `pending`. Add resources to
     the desired state set. Increment `call_index`. Return `Started`.
3. `rt.stop()` follows the same pattern but also acts as a barrier
   (blocks until Terminated) per `r[barrier.replay.rt-stop]`.

`Started` becomes:

```rust
pub struct Started {
    ctx: Arc<Mutex<ReplayContext>>,
    resources: Vec<ResourceInstance>,
}
```

The `.ready()` implementation (and equivalently `.scheduled()`, `.running()`,
`.terminated()`):
1. Check the context's `pending_barrier`. If this barrier has already been
   recorded as satisfied in `committed`, return immediately (fast-forward).
2. Query `world.lifecycle_state(resource)` for each resource in `self.resources`.
3. If all resources have reached the required state: record barrier as satisfied
   in the log, return self (or `Termination` for `.terminated()`).
4. If not all satisfied: record the `BarrierCondition` as `pending_barrier`,
   flush `pending` entries to the DB, throw `BarrierHit`.

---

## The `try/catch` Interaction

BSL scripts can use `try/catch`. This is important for patterns like:

```rhai
try {
    migration.terminated().ensure_success();
} catch (_err) {
    rt.start(migrate.call()).terminated().ensure_success();
}
```

A `BarrierHit` must not be swallowed by a BSL `try/catch`. Rhai's `catch`
block receives the error as a dynamic value. We handle this by making
`BarrierHit` distinguishable and checking for it:

- Option A: use a distinct Rhai error type that bypasses `try/catch`. Rhai
  does not natively support this.
- Option B: in the Rhai engine setup, register a custom error handler that
  re-throws `BarrierHit` errors even inside catch blocks.
- Option C: use a thread-local flag. When `BarrierHit` is thrown, set a
  thread-local. When Rhai's catch block executes, the first thing the
  `RuntimeInstance` does on any method call is check this flag and re-throw.

Option C is the most practical given Rhai's architecture. The thread-local is
cleared at the start of each closure execution pass. If a `BarrierHit` is
thrown inside a `try` block and the catch runs, the next `rt.*` call in the
catch block will immediately re-throw it, and the `BarrierHit` propagates up.

A cleaner alternative: since `BarrierHit` carries the barrier condition, the
host can inspect the error after the Rhai evaluation returns, regardless of
whether it propagated through catch blocks or not. If the error kind is
`EvalAltResult::ErrorRuntime` and the inner value is a `BarrierHit`, treat it
as a barrier. The key insight is that `BarrierHit` should propagate naturally
in the common case (not inside try/catch), and only needs special handling when
BSL code tries to catch it. Given the thread-local approach, any subsequent
`rt.*` call re-throws it.

---

## The Replay Loop (Host Side)

The host (the reconciliation loop, or the operation executor) drives the
closure execution:

```rust
pub fn run_operation(
    engine: &Engine,
    scope: &mut Scope,
    script_ast: &AST,
    operation: &LifecycleOperation,
    log: &ActionLog,         // persistent log, items 1-6
    world: Arc<dyn WorldStateOracle>,
) -> OperationResult {
    loop {
        // Load committed entries for this operation from the log.
        let committed = log.load_entries(operation.id);

        // Build a fresh ReplayContext for this pass.
        let ctx = Arc::new(Mutex::new(ReplayContext::new(
            operation.id,
            committed,
            Arc::clone(&world),
        )));

        // Inject a RuntimeInstance carrying the context into the Rhai scope.
        let rt = RuntimeInstance { ctx: Arc::clone(&ctx) };
        scope.push("__bsl_rt", rt);

        // Run the closure.
        let result = eval_action_closure(engine, scope, script_ast, operation);

        match result {
            Ok(_) => {
                // Closure completed normally. Flush any remaining pending entries.
                let ctx = ctx.lock();
                log.commit(&ctx.pending);
                return OperationResult::Completed;
            }
            Err(e) if is_barrier_hit(&e) => {
                // Extract the barrier condition and flush pending entries.
                let barrier = extract_barrier(&e);
                let ctx = ctx.lock();
                log.commit(&ctx.pending);

                // Wait for the barrier condition to be satisfied.
                // In the reconciliation loop this means: suspend here,
                // let the loop tick, re-enter when the oracle reports satisfied.
                wait_for_barrier(&barrier, &world);

                // Loop: next iteration will replay with updated committed entries.
                continue;
            }
            Err(e) => {
                // Genuine script error. Record as fault.
                return OperationResult::Failed(e);
            }
        }
    }
}
```

`wait_for_barrier` in production blocks the operation executor thread (or
suspends the async task, if we go async later) until the reconciler ticks and
signals the condition. In tests it directly updates the `WorldStateOracle` and
returns immediately.

---

## Test Strategy

The barrier engine must be fully testable without system calls. All tests use
a `TestWorldOracle`:

```rust
pub struct TestWorldOracle {
    states: Mutex<HashMap<ResourceInstance, LifecycleState>>,
}

impl TestWorldOracle {
    pub fn set(&self, resource: ResourceInstance, state: LifecycleState) {
        self.states.lock().insert(resource, state);
    }
}

impl WorldStateOracle for TestWorldOracle {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> LifecycleState {
        self.states.lock()
            .get(resource)
            .copied()
            .unwrap_or(LifecycleState::Pending)
    }
}
```

### Test Cases

**Basic barrier satisfaction**
- Script: `rt.start(dep).ready()`
- Oracle: `dep` starts at Pending
- First pass: BarrierHit thrown
- Set oracle: `dep` → Ready
- Second pass: barrier satisfied, closure completes
- Assert: action log has one Start entry and one satisfied barrier

**Barrier already satisfied on first pass**
- Script: `rt.start(dep).ready()`
- Oracle: `dep` starts at Ready
- First pass: barrier satisfied immediately, no BarrierHit
- Assert: closure completes in one pass

**Deadline expiry**
- Script: `rt.start(dep).ready(10)`
- Oracle: `dep` stays at Pending
- First pass: BarrierHit thrown, deadline recorded as 10s
- Simulate 11s elapsing
- Second pass: deadline check fires before barrier check → throws BSL exception
- Assert: OperationResult::Failed with deadline error

**Multi-resource barrier**
- Script: `rt.start([dep1, dep2]).ready()`
- Oracle: `dep1` → Ready, `dep2` stays Pending
- First pass: BarrierHit (dep2 not ready)
- Set oracle: `dep2` → Ready
- Second pass: both ready, completes
- Assert: two resources in the single Start log entry

**Sequential barriers**
- Script: `rt.start(frontend).scheduled(); rt.start(app).ready()`
- First pass: frontend Pending → BarrierHit at `.scheduled()`
- Set: frontend → Scheduled
- Second pass: first barrier satisfied, hits second barrier (app Pending) → BarrierHit
- Set: all app resources → Ready
- Third pass: both barriers satisfied, completes
- Assert: log has two Start entries, both with satisfied barriers

**Replay idempotency**
- Script: `rt.start(a); rt.start(b).ready()`
- Simulate first pass completing `rt.start(a)` then hitting barrier at `.ready()`
- Log now has one committed Start entry for `a`
- Second pass: `rt.start(a)` replays (idempotent, no new entry), `rt.start(b)` is live, barrier fires
- Set: `b` → Ready
- Third pass: both replayed, barrier satisfied
- Assert: `a` appears exactly once in the log

**try/catch does not swallow BarrierHit**
- Script:
  ```rhai
  try {
      rt.start(job).terminated()
  } catch(e) {
      rt.start(fallback).ready()
  }
  ```
- Oracle: `job` Pending
- First pass: BarrierHit inside try → re-thrown through catch → propagates to host
- Set: `job` → Terminated
- Second pass: barrier satisfied, catch not entered, closure completes

**Action composition and cycle detection**
- Script calls `rt.start(app.select(#{ types: [ResourceType.Action], names: ["start"] }))`
- The `start` action closure is invoked inline
- Barriers in the inner closure participate in the same replay context
- Verify cycle detection: if `start` also calls `start`, the second invocation throws

**rt.stop() as barrier**
- Script: `rt.stop(old)`
- Oracle: resources in `old` start at Running
- First pass: BarrierHit (resources not yet Terminated)
- Set: all → Terminated
- Second pass: stop barrier satisfied, completes

---

## Module Structure

```
src/runtime/
    mod.rs          — re-exports public API
    identity.rs     — ResourceInstance (item 1)
    lifecycle.rs    — LifecycleState + derivation (items 2, 4)
    db.rs           — SQLite schema + migrations (item 3)
    history.rs      — typed DB access (item 3)
    desired.rs      — desired state computation (item 5)
    scheduler.rs    — operation scheduler (item 6)
    barrier/
        mod.rs      — ReplayContext, BarrierHit, WorldStateOracle trait
        runtime.rs  — RuntimeInstance + Started (real implementations)
        replay.rs   — run_operation loop + wait_for_barrier
        oracle.rs   — TestWorldOracle + production DB oracle
```

The `src/defs/runtime.rs` stub remains in place for the language layer tests.
The real `RuntimeInstance` and `Started` live in `src/runtime/barrier/` and
are injected by the operation executor, not by the language layer.
When the full runtime is wired together, `src/defs/runtime.rs` will delegate
to the real implementation.

---

## Dependencies to Add

- `rusqlite` — for the persistent history stores (items 3, 4, 5, 6 also need it)
- `serde` + `serde_json` — for JSON payload columns in the DB
- `uuid` — for operation IDs

All are well-established crates with no async requirements.