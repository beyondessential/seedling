# Theme 8: Restart/replay correctness around persisted state

> Companion to the [logic bug audit](../logic-bug-audit-2026-07.md), cross-cutting theme 8.

## The failure pattern

Seedling persists state so that a daemon crash can be recovered from: the `current_operation` row, the `action_log`, `dynamic_resources`, and `schedules.last_fired_at`. In every affected finding, the write path and the restart read path were built and tested separately, and the bug lives exactly at their seam. Four distinct sub-patterns:

1. **Lossy re-persist.** `save_current_operation` (`crates/core/src/runtime/history.rs`) uses `INSERT OR REPLACE INTO current_operation` with an explicit column list that predates the `cancel_requested` column (added by migration `db/migrations/v34.sql`). REPLACE deletes the old row, so the column silently reverts to its default `0` — destroying a flag owned by a *different* writer, `set_cancel_requested`. The replay path re-saves the row (`oi/handler/actions/lifecycle.rs:346`) right after the daemon read the cancel flag, wiping it while the cancelled operation is still in flight.
2. **Effect stamped before it exists.** `check_due_schedules` (`crates/core/src/runtime/schedules.rs:107-121`) calls `db::upsert_schedule_fired` for both `ScheduleResult::Accepted` and `ScheduleResult::Queued`. A queued fire exists only in `Scheduler.queue` — an in-memory `VecDeque<QueuedOperation>` — so a restart loses the operation while the DB says it fired.
3. **State preserved for recovery, with no owner on the failure branch.** Startup orphan cleanup (`crates/daemon/src/main.rs:458-468`) deliberately skips `dynamic_resources` rows whose `operation_id` matches the persisted operation, expecting `replay_interrupted_operation` to adopt them. But every abort branch in that function — unregistered app, missing/undecryptable params (`revert_install_and_fault`), phase mismatch, scheduler refusal — just calls `clear_current_operation` and returns. Nobody stops the preserved units/containers or deletes their rows; the reconciler ignores them by design.
4. **Replay identity matched by value, not position.** The action log is positional: `ReplayContext.call_index` walks `committed`, and `do_exec` correctly recovers its result via `committed_entry()` at the current index (`barrier/runtime.rs:1172-1231`). `do_signal` instead scans the whole committed log for any entry with the same `(resources, signal)` (`runtime.rs:1069-1079`), so a second identical call is swallowed and a changed instance set re-delivers on replay. `check_barrier`'s `already_satisfied` fast path (`runtime.rs:1960-1968`) has the same shape and can match an earlier `Stop` entry's `Terminated/satisfied=true` record.

The common root: state written for crash recovery was only ever exercised on the path that wrote it. None of the four had a test that severs in-memory state between the write and the read.

## Affected findings

| Finding | Section | Severity |
|---|---|---|
| `save_current_operation` silently resets a persisted cancel request | [§7](../logic-bug-audit-2026-07.md#7-runtime-persistence-db-generations-history-audit-faults-gc) | medium |
| Queued schedule fire is stamped as fired but lost on daemon restart | [§6](../logic-bug-audit-2026-07.md#6-backups-volumes-scheduling) | low |
| Dynamic resources preserved for a replay are never torn down when the replay is abandoned | [§16](../logic-bug-audit-2026-07.md#16-daemon-and-ctl-crates) | medium |
| `rt.signal` replay dedup is value-based, not positional (H7) | [§8](../logic-bug-audit-2026-07.md#8-runtime-barrierorchestration-barrier-replay-oracle-probe-scaling) | high |

Adjacent findings with the same root (positional/replay discipline), fixed by the same rules:

| Finding | Section | Severity |
|---|---|---|
| `check_barrier`'s `already_satisfied` can match a committed `Stop` entry for a later `.terminated()` barrier | [§8](../logic-bug-audit-2026-07.md#8-runtime-barrierorchestration-barrier-replay-oracle-probe-scaling) | medium |
| Sub-action replay re-runs param validation instead of recovering the recorded params | [§8](../logic-bug-audit-2026-07.md#8-runtime-barrierorchestration-barrier-replay-oracle-probe-scaling) | medium |
| Barrier `satisfied=true` is never persisted for `check_barrier` barriers | [§8](../logic-bug-audit-2026-07.md#8-runtime-barrierorchestration-barrier-replay-oracle-probe-scaling) | low |

## Would a high-level change help?

**Partially — two mechanical rules plus one ordering rule, held together by a test discipline.** This theme, unlike (say) the fault-lifecycle theme, does not reduce to one missing abstraction: the four bugs live in three crates and the local fix for each is small. But they are not independent either:

- The positional-replay rule and the `INSERT OR REPLACE` rule are genuinely mechanical — each eliminates a *class*, is enforceable by construction (a private field, a CI grep), and would have prevented four of the seven findings above outright.
- The schedule bug is an ordering rule ("stamp the effect when it happens, not when it is intended"), not an abstraction; a helper would be ceremony around a one-line move.
- The abandoned-`dynamic_resources` bug is structural: cleanup responsibility is split across two files (`daemon/src/main.rs` startup vs replay) with a handshake ("the replay will adopt them") that no type or test enforces. Only a restart-shaped test catches this shape of bug, because the invariant ("no unmanaged workload survives an abandoned replay") spans a crash boundary.

So the honest verdict is: adopt the two mechanical rules as code changes, fix the ordering, and make restart-shaped tests the required pattern for all four — the test discipline is the only element that generalises to the *next* persisted-state feature.

## Proposed pattern

### 1. Positional replay accessor — the only way to ask "already done?"

`ReplayContext` already has the right primitive: `committed_entry()` returns `committed.get(call_index)`, and `is_replaying()` is `call_index < committed.len()` (`crates/core/src/runtime/barrier.rs:287-293`). Make that the *only* replay check:

- Add `ReplayContext::replay_step(&mut self, expect: CallKind) -> Result<Option<ActionLogEntry>, ReplayMismatch>`: if not replaying, return `Ok(None)`; otherwise clone `committed_entry()`, verify `call_kind == expect`, advance `call_index`, and return the entry. `do_exec` already implements this shape inline, including the diagnostic dump on mismatch — hoist it.
- Port `do_signal`, `do_write`, `do_start`/`do_stop`/`do_query`, and `record_subaction_entry` to `replay_step`. For `do_signal` that means: on replay, skip delivery only when the entry *at this index* is a `Signal` (optionally verifying `extra`); when not replaying, always deliver and log — fixing both halves of H7. For `SubAction`, recover the recorded params from the returned entry's `extra` instead of re-validating.
- Make `committed` private (accessor for the operation loop's read-only uses) so a value-based scan cannot be reintroduced. `check_barrier`'s `already_satisfied` and `started_at` lookups then have to anchor on the committed entry that *owns* the barrier (the tracked `Start`/`Stop` entry for these resources at or before `call_index`), not on any matching record anywhere in the log — which also removes the Stop/`.terminated()` false positive.

### 2. No `INSERT OR REPLACE` on rows with independently-updated columns

`INSERT OR REPLACE` is delete-then-insert: every column not listed reverts to its default. That is safe only when one writer owns the whole row. Current inventory (`rg "INSERT OR REPLACE" crates`):

- `history.rs:507` (`current_operation`) — **the bug**. Replace with an upsert that only touches the columns this writer owns: `INSERT INTO current_operation (...) VALUES (...) ON CONFLICT(singleton) DO UPDATE SET operation_id=excluded.operation_id, ... ` — leaving `cancel_requested` alone. Companion fix: `set_cancel_requested` returns `Ok(false)` when no row matched and `cancel_action` (`oi/handler/actions.rs:55`) ignores it; either persist the row before the operation becomes cancellable or surface the `false`.
- `history.rs:371` (`action_log`) — legitimate: positional overwrite keyed on `(operation_id, call_index)` is exactly the replay contract. Annotate it as the allowlisted exception.
- `apps.rs:279` / `oi/handler/apps.rs:127` (`registered_apps`), `scaling.rs:37`, `apps/params.rs:36`, `secret_params.rs:50`, `desired.rs:312` — currently list every column, so no live bug; but the `cancel_requested` failure was *created* by migration v34 adding a column to a table an older REPLACE already wrote. Converting these to `ON CONFLICT ... DO UPDATE` makes future `ALTER TABLE ADD COLUMN` migrations safe by default.

### 3. Stamp schedule fires when dispatched, not when queued

In `check_due_schedules`, call `upsert_schedule_fired` only for `ScheduleResult::Accepted`. For `Queued`, stamp at promotion — the site that consumes `Scheduler::complete_current()`'s popped `QueuedOperation` and spawns it. While queued, the next tick's re-fire attempt hits `Rejected(SameAppAlreadyQueued)` and is already silently dropped, so there is no double-fire; after a restart the unstamped schedule fires again, restoring the `r[schedule.catch-up]` guarantee. (Persisting the queue itself would also work but is strictly heavier for the same observable behaviour.)

### 4. A restart-shaped test harness

The existing pieces are close: `run_operation` + `InMemoryActionLog` (`crates/core/src/tests/barrier.rs`) test suspension, and `db_action_log_barrier_suspends_then_resumes` (`crates/core/src/runtime/barrier/replay/tests.rs`) tests the DB-backed log — but both reuse the same engine, scope, `App`, and registry across passes. They exercise *suspension*, not *restart*: no in-memory state is ever dropped. The missing pattern:

```rust
/// Everything that survives a daemon crash: the DB and the outside world.
struct RestartWorld {
    db: DbHandle,                  // opened once; represents disk
    oracle: Arc<TestWorldOracle>,  // containers keep running through a crash
}

impl RestartWorld {
    /// One daemon lifetime: build ALL in-memory state from scratch, run one
    /// pass, drop it. Scope, engine, App, registry, scheduler — nothing is
    /// carried over except what `self` holds.
    fn pass(&self, script: &str, op: &OperationId, action: &str) -> OperationResult {
        let (engine, mut scope, app) = crate::setup_language(&ScriptLimits::default());
        let ast = crate::tests::run_script(&engine, &mut scope, script).unwrap();
        let log = DbActionLog::new(self.db.clone(), op.clone(), app_name(), action_name(action));
        let registry: Arc<dyn InstanceRegistry> = Arc::new(DbInstanceRegistry::new(self.db.clone()));
        run_operation(OperationContext { engine: &engine, script_ast: &ast, log: &log,
            world: Arc::clone(&self.oracle), registry, /* ...defaults... */ }, &mut scope)
    }
}
```

Worked example (fails today — H7):

```rust
#[test]
fn second_identical_signal_survives_restart() {
    let world = RestartWorld::new();
    let signals = Arc::new(RecordingSignaler::default()); // stub ContainerSignaler
    let script = r#"
        let db = app.deployment("db").image("docker.io/library/postgres:16");
        app.on_start(|rt, _p| {
            rt.signal(app.deployment("db"), "SIGHUP");
            rt.start(app.job(job_def)).terminated().ensure_success();
            rt.signal(app.deployment("db"), "SIGHUP");
        });
    "#;
    let op = OperationId::new();
    // Pass 1: job not terminated -> first signal delivered, suspend. Crash.
    assert!(matches!(world.pass_with_signaler(script, &op, "start", &signals),
        OperationResult::Suspended(_)));
    world.oracle.set(job("j"), LifecycleState::Terminated);
    // Pass 2 = fresh daemon: replay must skip signal #1 and deliver signal #2.
    assert!(matches!(world.pass_with_signaler(script, &op, "start", &signals),
        OperationResult::Completed));
    assert_eq!(signals.delivered(), 2);
}
```

The same shape covers the other findings without new machinery: the cancel-flag test is pure `Db` (save op, `set_cancel_requested`, save again, assert `load_cancel_requested` — extend `history/tests.rs::cancel_requested_round_trips_through_db`); the schedule test is `check_due_schedules` into a busy `Scheduler`, then a *fresh* `Scheduler::new()` and a second tick asserting the fire re-occurs.

## What it prevents — and what it does not

**Prevents:** the whole value-vs-position dedup class (H7 and the three adjacent §8 findings) by construction once `committed` is private; silent column loss on every future `ALTER TABLE ADD COLUMN` against an upserted table; lost queued fires; and — via the harness — regressions in any operation-level restart behaviour, including the double-crash cancel scenario (the harness exposes the `DbHandle`, so a test can assert `load_cancel_requested` between passes).

**Does not prevent:** the abandoned-`dynamic_resources` bug as such. `replay_interrupted_operation` and the orphan sweep are free functions in `crates/daemon/src/main.rs` over `OiState` and a live `driver`; the harness cannot reach them until the abort branches are extracted into `crates/core` behind a cleanup trait (the stub `System` backend then makes the §16 test moderate, as the audit notes). Nor does it catch bugs where the *intent* is wrong — stamping on `Queued` was a deliberate anti-double-fire choice; a restart test reveals the consequence, but a human still has to decide the ordering rule. Daemon-level cleanup ordering (the §16 pod-network leak) needs the stub-driver startup test, which is a sibling discipline rather than this harness.

## Migration path

1. Fix `do_signal` positionally and add the restart-shaped test above (H7 is the high-severity item; spec first: tighten `l[rt.signal]`/`r[rt.signal]` to state at-most-once *per call site*).
2. Add `ReplayContext::replay_step`, port `do_write`/`do_start`/`do_stop`/`do_query`/`record_subaction_entry`, re-anchor `check_barrier`'s committed-log lookups, then make `committed` private.
3. Convert `save_current_operation` to `ON CONFLICT DO UPDATE`; extend `history/tests.rs` with the save-cancel-save round trip; handle `set_cancel_requested == false` in `cancel_action`.
4. Move the `Queued` stamp to promotion; add the fresh-`Scheduler` restart test in `schedules.rs::tests`.
5. Extract the replay abort branches from `daemon/src/main.rs` into core with an injected teardown; every branch that gives up on the operation must also tear down (or hand to the orphan sweep) the `dynamic_resources` it asked startup to preserve.
6. Convert the remaining `INSERT OR REPLACE` sites opportunistically (no behaviour change today, so these can ride along with the next touch of each file).

Each step is an independent commit; per repo convention, spec updates in `docs/spec` land before the implementation and tests carry `r[verify ...]` annotations.

## Enforcement

- **Restart-shaped tests as a required pattern:** any change that adds a table/column read during daemon startup, or a new `rt.*` call kind, must ship a test that runs to a suspension point, drops all in-memory state, rebuilds from the `Db`, continues, and asserts the invariant. The `RestartWorld` harness makes this cheap enough to demand.
- **Tracey:** add spec items for the two mechanical rules (e.g. `r[barrier.replay.positional]`: replayed calls are matched to the committed entry at their call position; `r[history.persist.partial-update]`: writers of shared rows must not reset columns they do not own) so `tracey query uncovered --spec-impl runtime/main` flags new `do_*` calls or persistence writers with no verify coverage.
- **CI grep:** `rg -n "INSERT OR REPLACE" crates --glob '!crates/core/src/runtime/db/migrations/*'` with an explicit allowlist containing only `history.rs` `action_log` (positional replace is its contract). Migrations are exempt; everything else fails the build with a pointer to the upsert helper.
- **Review checklist:** for any persisted-for-recovery state — *who else writes this row? which restart path reads it? what happens on every abort branch of that path? which test severs memory between the write and the read?* If the last question has no answer, the change is not done.
