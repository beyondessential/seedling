# F6: start the resources a shell closure creates

A shell closure's `rt.start` places the session's own dynamic resources in the app's desired state, beside the operation's slot, and the session tears them down after it ends, recorded so a crash leaves nothing behind. Specs: RCN#desired-state, RT#starting-resources, RT#stopping-resources, OPS#shell-sessions, APP#dynamic-resource-lifetime, FLT#app-faults, APPS#dynamic-resources.

## Technical notes

- The reconciler reads one progress per app, `AppEntry::active_progress`, which only works because one operation runs per host. Shell sessions need a progress each, kept in a per-app map keyed by session beside that slot. Every open session's dynamic resources are merged into the desired state, whatever the app's phase or `start_pending` holds (RCN#desired-state). A session only touches dynamic instances, so merging cannot clash with the operation's entries.
- The shell's `run_operation` call gets a database and the session's progress slot. `rt.start` then records each dynamic resource before placing it, as it already does for operations (the `insert_dynamic_resource` path in `runtime/barrier/runtime.rs`). In a shell, the static resources of a selection are left out of the progress (RT#starting-resources), and `rt.stop` throws if its selection holds one (RT#stopping-resources).
- Record key: the shell's internal `OperationId` becomes the session's `session_id` (today `run_app_shell` mints a fresh one), so the record's `operation_id`, the action log and the `operation_cleanup_stalled` subject all carry the id an operator sees. A new `shell_session` flag column says which kind created the record; `/apps` reads it to report `session_id` with `operation_id` null (APPS#dynamic-resources). Never edit an existing migration.
- When a session ends, its slot is handed to the same retrying teardown ended operations use, driven through that slot rather than the app's operation slot, with `operation_cleanup_stalled` keyed by the session id. The exit frame does not wait for that teardown (OPS#shell-sessions).
- The startup teardown already removes every record not spared for a replay. Sessions are never replayed, so their records are always torn down. The background re-read in `keep_tearing_down` and `clear_finished_operations` must count open sessions, and sessions still tearing down, as live, or it will tear down a live session's resources.
- GC's `known_desired_state` treats an app as steady when its operation slot is empty. A session that starts something after the snapshot would leave its instance outside the snapshot's desired set, so an app with any session slot is not steady there either.

## Records

- [ ] Add `crates/core/src/runtime/db/migrations/v81.sql`: `ALTER TABLE dynamic_resources ADD COLUMN shell_session INTEGER NOT NULL DEFAULT 0`, and register version 81 in `MIGRATIONS` (`runtime/db.rs`).
- [ ] `runtime/desired.rs`: `DynamicResourceRecord` gains the creator kind (an enum, not a bare bool); `insert_dynamic_resource` takes it and names the column in its `ON CONFLICT ... DO UPDATE`; `parse_dynamic_row` and the three `list_dynamic_resources*` queries select it.
- [ ] Update the raw `INSERT INTO dynamic_resources` test helper in `crates/daemon/src/main.rs` and any other test inserts for the new column.

## Shell closure wiring

- [ ] `oi/shells/session.rs`: pass the session's `SessionId` from `open_shell_session` into `run_app_shell`, and use it as the closure's `OperationId`.
- [ ] `run_app_shell`: pass `db: Some(...)` and the session's progress slot as `active_progress`; drop the comment claiming shell resources vanish with the replay context.
- [ ] `runtime/barrier/runtime.rs`, `do_start_or_query`: record a shell's dynamic resources with the shell creator kind (the `ReplayContext` already knows `is_shell`).
- [ ] `runtime/barrier/replay.rs` (the progress update after the pass, around `OperationProgress::from_log`): for a shell, keep only the instances that have a dynamic definition, so a static resource the shell started never enters the session's progress. Cite RT#starting-resources.
- [ ] `rt.stop` in `runtime.rs`: in a shell, throw before claiming a log position or touching the progress when any resource of the selection is static. Cite RT#stopping-resources.

## Reconciler

- [ ] `runtime/apps.rs`: `AppEntry` gains a per-session progress map (`Arc<RwLock<BTreeMap<SessionId, OperationProgress>>>` or similar), initialised in both constructors.
- [ ] `runtime/desired.rs`: a function that merges session progresses into a computed `DesiredState`, carrying each instance's state and dynamic definition.
- [ ] `system/reconcile.rs` (~967): apply the merge in every branch for `Installing`, `Installed` (including `start_pending` and the empty-progress install) and `Uninstalling`. Cite RCN#desired-state.
- [ ] `system/reconcile/state.rs` `known_desired_state`: an app with any session slot is not still steady.
- [ ] Check `system/translate/proxy.rs` and anything else reading the operation slot for routes or warm certificates, and fold session progress in where the operation's is read, so a shell's anonymous service or ingress contributes to absolute state as RCN#applying-absolute-state has it.

## Session end and teardown

- [ ] Move `cleanup_dynamic_resources` and `CleanupPace` out of `oi/handler/actions/lifecycle.rs` into a shared module (next to `runtime/dynamic_teardown.rs`), parameterised by the progress slot it drives and the owner id it reads records and keys the fault by. Update the operation call site.
- [ ] `oi/shells/session.rs`: once the session's exit frame is written and it is removed from `ShellRegistry`, spawn that cleanup against the session's slot; the slot entry is removed from the map once the cleanup finishes. Cite OPS#shell-sessions.
- [ ] `runtime/dynamic_teardown.rs`: `live_operations` also counts open sessions and sessions whose cleanup is running; thread a way to read them (the shell registry and the per-app session maps) through `keep_tearing_down` and `clear_finished_operations`, and the daemon's call site.

## Interface

- [ ] `oi/handler/apps.rs` (~844): report `session_id` and a null `operation_id` for a shell's record, and `session_id: null` otherwise. Cite APPS#dynamic-resources.
- [ ] `crates/web/frontend/src/lib/types.ts`: `operation_id` and `session_id` as `string | null` on dynamic entries; check `AppDetail.tsx` and the resources section for any reader of `operation_id`.

## Docs

- [ ] `docs/runtime-overview.md` (Shells) and `docs/bsl-scripting.md` (dynamic context, shell examples): a shell's `rt.start` starts its own dynamic resources, which are torn down after the session; `rt.stop` on a declared resource throws in a shell.

## Tests

- [ ] `oi/shells/tests.rs`: a shell that attaches and then runs `rt.start(app.job()...).terminated()` ends with the job's outcome rather than hanging until stopped.
- [ ] Same file: `rt.start` of a static resource in a shell leaves the app's desired state unchanged and returns a `Started` whose barrier waits on it; `rt.stop` of a static resource throws and changes nothing.
- [ ] Two concurrent sessions, and a session beside an operation: each session's resources stay in the desired state independently, and ending one session tears down only its own.
- [ ] A session's dynamic resource is recorded with the shell kind and its session id before it is started; a record that cannot be written fails the call without starting it.
- [ ] Session end: the exit frame is written before teardown finishes; teardown that stalls past a minute holds `operation_cleanup_stalled` with the session id as subject and clears it when done.
- [ ] Daemon startup (`crates/daemon/src/main.rs` tests): shell records left by a previous run are torn down.
- [ ] `dynamic_teardown` tests: a background re-read does not tear down an open session's records.
- [ ] `/apps` test: a shell's dynamic resource reports `session_id` set and `operation_id` null; an operation's the reverse.
- [ ] GC: an instance a session started after the steady snapshot is not collected.
