# Circuit breaker for runaway evaluations

Bound each evaluation by processor time, and keep a breaker per app definition and per action closure so a runaway with the same definition and inputs is paid for once.
Specs: LIM (`runtime/engine-limits.md`, measuring evaluation time and breakers), FLT (`evaluation_runaway`, `breaker_state_unreadable`, withholding), BSL replicability, wire `evaluation_runaway`, and the refusals in actions, shells, app control, gates, scheduled actions and images.

## Notes

- Every production engine comes from `setup_language` (`crates/core/src/lib.rs`), so the clock hangs off Rhai's progress hook there. The hook fires per operation, which gives "stopped at the next operation" for free; the return check catches a long native call that ends the evaluation.
- One evaluation spans more than one Rhai top-level call: an operation pass re-runs the definition in `capture_closures` (`defs/app.rs`) and then calls the closure, and a probe does the same. The clock is therefore started by the caller per evaluation, not per Rhai call, and the hook only checks it.
- Processor time is the evaluating thread's CPU time. Every evaluation runs synchronously on one blocking thread, so waits (barriers, shell attach, `rt` methods blocking on the host) drop out without bookkeeping. Thread CPU time comes from the `cpu-time` crate (`cargo add cpu-time`, pinned), which keeps core portable.
- Rhai's `no_time` feature stays on; the clock is ours.
- A definition is identified by its bundle hash (`Bundle::hash()`). Breaker digests use `Cipher::key_digest` with its own HKDF context, separate from the gate key digest's.
- A breaker is tripped exactly while its `evaluation_runaway` fault is active: the fault is the tripped state, and the `breakers` row only records which digest tripped it, meaningless once the fault clears. Tripping writes the row and then files the fault; resetting clears the fault and then drops the row. Each change of state is the one statement on the fault, so no transaction is needed and an operator's clear is the reset.
- Breaker subjects name the kind and the name (`action:<name>`, `shell:<name>`, `gate:<name>`, empty for the definition), parsed back into an enum rather than matched by prefix.
- Volume params are left out of a closure's inputs: dispatch replaces them with runtime-named keys, so the operator's value is gone by the time a pass runs. A gate body's `last` input is the definition and stored values of the app `last` reflects, which the pass has to hand, rather than its generation number.
- `Action.invoke` consults breakers through a thread-local the operation and shell loops install for their passes, since the call runs deep inside the engine with no handle on the database.
- Crash-recovery answers (failure-modes §7): only the breaker store writes the breaker table; startup reads it lazily at each tracked evaluation or refusal; a failed read holds `breaker_state_unreadable` and evaluates nothing; a test drops the registry between trip and the next evaluation and checks the breaker still holds.

## Evaluation clock

- [x] Add `max_evaluation_time` (seconds, 0 = none) to `ScriptLimits` with a default of 10, and `--script-max-evaluation-time` to `ScriptLimitArgs` in `crates/daemon/src/main.rs`, with an argument test beside the regex one.
- [x] Add an `EvaluationClock` in core: started per evaluation, reading thread CPU time, and reporting whether the limit is exceeded. Register it in `setup_language` through `engine.on_progress`, terminating with a recognisable token.
- [x] Add a helper that runs one evaluation under the clock (start, run, check on return), and route every evaluation through it: `run_seeded_definition` (`runtime/apps.rs`), each validator call in `run_validators`, each pass in `barrier/replay.rs::run_operation` (covering `capture_closures` and the closure call together), the shell loop in `oi/shells/session.rs`, `probe.rs::probe_app`/`probe_gate_body` handler runs, and templates preview.
- [x] Add a `ScriptError` variant or flag for a runaway, so callers can tell it from an ordinary script error after masking; map the progress-hook termination and the return check to it.
- [x] Tests in `engine_limits_tests`: a tight loop with the operations limit removed stops near the limit; a long native call (a large regex search) fails on return; a removed limit (0) lets it finish; time blocked in a sleeping host call does not count.

## Breaker store

- [x] Migration v85 in `runtime/db.rs`: a `breakers` table keyed by app and thing (kind, name), holding the definition-and-inputs digest, the generation that tripped it, and when.
- [x] `runtime/breakers.rs`: the `BreakerThing` enum (definition, action, shell, gate) with its fault subject encoding and parsing; `BreakerInputs` built from stored params, invocation or session params in canonical form without reserved keys, the gate's last-success generation, host constants, app seed, Seedling version and the limit in force; and the keyed digest over bundle hash plus inputs.
- [x] Store operations: `check` (returning tripped, clear or unreadable, never collapsing unreadable into clear), `trip` (insert or re-record the digest with `ON CONFLICT DO UPDATE`, and file `evaluation_runaway` in the same transaction), `reset` (delete and clear the fault in one transaction), and `discard_app`.
- [x] Hold and clear `breaker_state_unreadable` per thing when `check` fails, following `dynamic_resources_unreadable`'s hold/clear helpers.
- [x] In-memory last-good record per app and thing (per-input digests and the bundle hash) for the likely cause; the fault description names differing inputs by parameter name, host constant name, app seed, Seedling version or limit, never a value.
- [x] Tests: trip then a fresh store over the same database still refuses; re-record on a second runaway with new inputs keeps the fault without clearing it; an unreadable table holds the fault and does not report clear; likely cause for inputs, definition and unknown.

## Tracked evaluations

- [x] Definition: in `AppRegistry` (`reevaluate`, `load_from_db`, registration, param change, seed rotation, registry allowlist re-evaluation), check the definition breaker before evaluating; trip on a runaway, reset on an in-time evaluation with a different digest. Only these trip or reset; `evaluate_validated` for proposals, plans and the validator fallback do not.
- [x] `sync_script_error_fault` (`runtime/apps/params.rs`): on a runaway, neither file nor clear `script_error`; `AppEntry.script_error` keeps the runaway so gates go undetermined and refusals still fire.
- [x] Operation passes (`operations.rs::run_operation_loop`): treat a pass as tracked only when the operation's generation is the app's current one; check the action's breaker before the first pass; trip the operation's own action on a runaway, including one inside `invoke`; reset on an in-time pass with a different digest.
- [x] Gate body runs (`gates/run.rs`): tracked as for operations, with the gate's last-success generation as an input.
- [x] Shell sessions: tracked against the current app; check before the closure runs, trip on a runaway, reset on an in-time run with a different digest.
- [ ] Tests beside `runtime/operations/tests.rs`, `gates/run/tests.rs` and `oi/shells/tests.rs`: a runaway trips the right breaker; a pass at an older generation stops but leaves no breaker; a later in-time run with different params resets it. Action passes and the definition are covered; shell runs and gate body runs are not.

## Refusals and skips

- [x] Protocol: add `ErrorCode::EvaluationRunaway` (`crates/protocol/src/error.rs`) and extend the serialisation test.
- [x] `/apps/action/invoke` (`oi/handler/actions.rs`): refuse with `evaluation_runaway` while the definition's fault is active, and when the action's breaker is tripped for the request's definition and inputs.
- [x] Install (`oi/handler/apps.rs`) and gate run (`oi/handler/gates.rs`): refuse on the definition's fault; the gate-run request also on the body's breaker.
- [x] Shell open (`oi/shells/session.rs`): refuse with `script_error` and with `evaluation_runaway` on the definition's faults, and on the shell's breaker.
- [x] `Action.invoke` (`runtime/barrier/action_call.rs`): throw when the invoked action's breaker is tripped for its definition and the invoke params, except for a call already recorded in the action log; skip the check on replay as validation is skipped.
- [x] Scheduled fires (`runtime/schedules.rs::check_due_schedules`): when the action's breaker is tripped for the run's definition and inputs, stamp the fire and skip it without a further fault.
- [x] Gate preparation (`gates/run.rs::prepare`): fail with nothing stopped when the body's breaker is tripped or unreadable, and when the preparation probe is stopped at the limit.
- [x] Probes (`runtime/probe.rs`): report a tripped breaker, and a stop at the limit, as the handler's error; never trip or reset.
- [ ] Tests for each refusal beside the existing `script_error` refusal tests (`oi/handler/apps/tests.rs`, `oi/handler/gates/tests.rs`, actions, shells, schedules, probe). Actions, gate-run requests, schedules, probes and `invoke` are covered; shell opening, gate preparation and a replayed `invoke` are not.

## Fault lifecycle

- [x] Withholding: replace the kind-only `WITHHOLDING_KINDS` check in `runtime/faults.rs` with one that counts `evaluation_runaway` and `breaker_state_unreadable` only for the definition or a gate subject; extend `only_a_withholding_fault_faults_the_app`.
- [x] `/faults/clear` (`oi/handler/faults.rs`): reset the breaker of each cleared `evaluation_runaway` in the same transaction, then re-evaluate the definition when its breaker was among them.
- [x] After each in-time definition evaluation, reset the breakers (and clear the faults) of closures the definition no longer declares; discard every breaker on deregistration.
- [ ] Tests: clearing an unrelated fault re-arms every breaker; clearing the definition's fault re-evaluates; renaming an action clears its fault. Clearing and an undeclared closure are covered at the store; the re-evaluation on clear, and `/apps/update` or a validator running away, need a `TestOi` built with short limits.

## Interface reporting

- [x] `/apps/update` (`oi/handler/apps.rs::update_app`): a runaway in the proposed definition or a validator refuses with `evaluation_runaway` in place of `script_error` or `validation_failed`, filing nothing.
- [x] `run_validators` (`runtime/apps.rs`): a validator stopped at the limit is no `Rejection`; carry it separately so `/apps/params/set` refuses with `evaluation_runaway` and `/apps/plan` lists it in `rejections` with a reason naming the limit.
- [x] Fault records name an action's, shell's or gate's breaker faults by `resource_type` (`action`, `shell`, `gate`) and `resource_name`, leaving the definition's empty, so a client can tell them apart.
- [x] Web UI (`crates/web/frontend/src/routes/AppDetail.tsx`): treat the definition's `evaluation_runaway` and `breaker_state_unreadable` as `script_error` is treated for disabling Install, Gates and Actions. An action's or gate's breaker fault leaves its button live, since other params still evaluate and the server refuses the tripped run.

## Docs

- [x] `docs/bsl-scripting.md`: the evaluation time limit beside the other limits, and what a breaker does and how clearing the fault retries.
- [x] `docs/deploying.md`: `--script-max-evaluation-time` beside the other daemon options.
- [x] Run `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh` and `etc/ci/check-spec-citations.sh`.
