# Backup and Scheduling Design

This document describes spec additions and an implementation plan for:
1. Action and shell params
2. Shell control type
3. Scheduled actions
4. Backup strategies and backup apps

## Spec Changes: Language (language.md)

### Action Params

> l[action.params]
> All action closures receive exactly two arguments: the [Runtime Instance](#l--rt.var) (`rt`) and a `Param` object map (`param`).
>
> The `param` is an arbitrary key-value map provided by the invoker. When no params are provided, `param` is an empty map (`#{}`).
>
> Param keys ending in `_volume` are reserved for internal use by the Seedling runtime. The runtime must reject operator-provided params whose keys end in `_volume`.

Update l[action.type]:
> The `fn` closure must take exactly two arguments: the [Runtime Instance](#l--rt.var) (typically named `rt`) and the param map (typically named `param`).

Update l[action.start]:
> The `fn` closure must take exactly two arguments: `rt` and `param`. When fired autonomously (boot, schedule), `param` is an empty map.

Update l[action.install]:
> The `fn` closure must take exactly two arguments: `rt` and `param`. Install requirements values are delivered through `param`. The requirements definition (second argument to `on_install()`) defines the validation schema; it does not change the closure signature.

Note: `on_change` handlers retain the existing `|rt, old|` signature. They are a different closure type with different semantics.

### Shell Control Type

Replace l[action.shell] (the closure form section):
> The `fn` closure must take exactly three arguments: the [Runtime Instance](#l--rt.var) (typically named `rt`), the [Shell Control](#l--action.shell.control) (typically named `shell`), and the param map (typically named `param`).

> l[action.shell.control]
> The Shell Control is the second argument of the Shell Action. It is a custom type with two methods:
>
> - `shell.attach(job: Job)`: bridges the operator's input/output to the Job. Blocks until the operator closes the session or the connection is interrupted. Must be called exactly once per shell invocation. Calling `attach` a second time must throw.
> - `shell.error(msg: string)`: sends an error message to the client and terminates the shell session. This call is terminal: it throws an exception to end the closure.
>
> A shell closure that returns without calling `attach` or `error` is invalid. The runtime must return an error to the client.

Remove the one-argument `|rt|`-returning-Job shortcut form entirely.

### Scheduled Actions

> l[action.schedule]
> An Action returned by `on_action()` may be given one or more cron schedules via the `.on_schedule(expr: string)` builder method.
>
> `on_schedule` returns the Action for chaining. It may be called multiple times to attach multiple schedules to the same action.
>
> The `expr` is a 5-field cron expression (minute, hour, day-of-month, month, day-of-week) with 1-minute minimum resolution. The Jenkins `H` extension is supported: `H` is replaced with a stable hash-derived value within the field's range, computed from `(app_name, action_name)`. For example, `H 2 * * *` fires once daily at a stable minute during the 02:xx hour.
>
> `on_schedule` must not be called on the Start Action (name `"start"`); doing so must throw. `on_schedule` is not available on Shell Actions.
>
> When a scheduled action fires, it is invoked as a normal lifecycle operation with an empty `param` map. Operators may also invoke a scheduled action manually via the action invocation RPC, in which case operator-provided params are passed through.

### Dynamic External Volumes in Action Scope

> l[volume.external.dynamic]
> When `app.external_volume(name)` is called within an action closure, the runtime checks operation-scoped volume bindings first, then falls back to the static external volume mapping table. Operation-scoped bindings are injected by the runtime for specific internal operations (such as backup actions) and are not operator-configurable.
>
> If the name resolves to an operation-scoped binding, the returned ExternalVolume references the bound path for the duration of the operation. The binding is removed when the operation ends.

## Spec Changes: Interface (interface.md)

### Action Invocation Params

Update i[action.invoke]:
> `/apps/action/invoke { app, name, params? }` schedules the named action as a lifecycle operation.
> `params` is an optional JSON object. Keys ending in `_volume` are reserved and must be rejected.
> Returns `{ "schedule": "accepted" }` or `{ "schedule": "queued" }` on success, or an error.

Update i[action.invoke.install]:
> `/apps/install/invoke { app, requirements? }` schedules the install action.
> `requirements` is an optional JSON object of requirement key → string value. The values are delivered to the install closure as `param`.

Update i[shell.open]:
> `/shells/start { app, name, rows, cols, params? }` opens an interactive shell session.
> `params` is an optional JSON object. Keys ending in `_volume` are reserved and must be rejected.

### Event Trigger Provenance

Update i[event.types], add `trigger` field to `OperationStarted`:
> | `OperationStarted` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation`, `trigger` |
>
> The `trigger` field is a string indicating what caused the operation:
> - `"operator"`: manual action or install invocation.
> - `"boot"`: automatic start on runtime startup.
> - `"param_change"`: an `on_change` handler firing.
> - `"schedule"`: a BSL `on_schedule` cron fire.
> - `"backup_schedule"`: a backup strategy scheduled fire.

### Backup App Registration

> i[backup.app.register]
> `/backups/apps/register { name, app }` registers the named app as a backup app under the given backup-app name. The app must declare actions `save-snapshot`, `list-snapshots`, and `restore-snapshot`. If validation fails, the request is rejected.
>
> Returns `{ "registered": true }` on success.

> i[backup.app.deregister]
> `/backups/apps/deregister { name }` removes a backup app registration.
> If any backup strategies reference this backup app, the request is rejected with `backup_app_in_use`.

> i[backup.app.list]
> `/backups/apps/list {}` returns an array of registered backup apps with fields `name` and `app`.

> i[backup.app.validation]
> On `/apps/update` for an app that is a registered backup app, the runtime must evaluate the new script and reject the update if the required actions (`save-snapshot`, `list-snapshots`, `restore-snapshot`) are no longer present.
> The same validation must be performed during `/apps/plan` (dry-run) and reported in the diff.

### Backup Strategies

> i[backup.strategy.create]
> `/backups/strategies/create { name, via, schedule, volumes }` creates a named backup strategy.
>
> - `name`: strategy name (follows standard naming rules).
> - `via`: name of a registered backup app.
> - `schedule`: one of `"every hour"`, `"twice a day"`, `"every day"`.
> - `volumes`: array of source volume identifiers (`"<app>/<volume>"` or `"_site/<volume>"`).
>
> Volume references are not validated at creation time; validation occurs at fire time. The CLI should check volume existence at creation and require `--allow-missing` to proceed if any volume does not resolve.
>
> Returns `{ "created": true }` on success.

> i[backup.strategy.list]
> `/backups/strategies/list {}` returns an array of strategy objects with fields `name`, `via`, `schedule`, and `volumes`.

> i[backup.strategy.show]
> `/backups/strategies/show { name }` returns the strategy object.

> i[backup.strategy.update]
> `/backups/strategies/update { name, via?, schedule?, volumes? }` updates a strategy. All fields except `name` are optional; only provided fields are changed. Changes raise events and are recorded in the audit log.

> i[backup.strategy.delete]
> `/backups/strategies/delete { name }` deletes a strategy. Future fires are cancelled immediately. In-flight operations are not affected.

### Backup Execution

> i[backup.run]
> `/backups/run { strategy, volume? }` triggers an immediate backup for the named strategy. If `volume` is provided, only that volume is backed up; otherwise all volumes in the strategy are backed up.
>
> Returns `{ "schedule": "accepted" }` on success.

### Backup Snapshot Listing

> i[backup.snapshots.list]
> `/backups/snapshots/list { strategy, source? }` invokes the backup app's `list-snapshots` action synchronously and returns the result.
>
> The runtime invokes `list-snapshots` on the backup app with params including `strategy`, `source` (if provided), `output_volume`, and `output_filename`. After the action completes, the runtime reads the output file and returns its contents.
>
> Each entry in the output must contain at least `id` (string) and `timestamp` (RFC 3339). Additional fields are tool-specific and passed through.

### Backup Restore

> i[backup.restore]
> `/backups/restore { strategy, snapshot_id, source }` restores a snapshot into a fresh managed site volume.
>
> The runtime creates a new managed site volume (named `restore-<strategy>-<timestamp>`), invokes the backup app's `restore-snapshot` action with the target volume bound read-write, and returns the site volume name on success.
>
> The operator is responsible for using the restored volume (e.g. remapping, swapping) and deleting it when done.

### CLI

> i[ctl.action.params]
> The CLI accepts action params as positional arguments after the action name: `ctl apps action <app> <name> [key[=value]]...`.
> A bare key (no `=`) maps to `key: true`. A `key=value` pair maps to `key: "value"`.

> i[ctl.shell.params]
> The CLI accepts shell params with the same syntax: `ctl apps shell <app> <name> [key[=value]]...`.

> i[ctl.backup.app.hint]
> When `ctl apps create` evaluates a script that declares actions `save-snapshot`, `list-snapshots`, and `restore-snapshot`, the CLI should print an informational message suggesting backup app registration.

> i[ctl.backup.strategy.allow-missing]
> When creating or updating a backup strategy, the CLI checks volume existence. If any referenced volume does not resolve, the CLI requires `--allow-missing` to proceed.

## Spec Changes: Runtime (runtime.md)

### Action Param Persistence

> r[operation.params]
> When a lifecycle operation is dispatched with params, the params must be persisted alongside the operation record in the action execution log. On replay, the persisted params must be restored and passed to the action closure.
>
> Shell sessions do not persist params; shells are not replayable.

### Scheduled Action Execution

> r[schedule.tick]
> The reconciliation loop must check for due scheduled actions at least once per minute. This check is performed as part of the normal reconciliation tick.

> r[schedule.fire]
> For each `(app, action, cronexpr)` tuple, the runtime computes the next fire time from the stored `last_fired_at` and the cron expression. If the next fire time falls within the last 59 seconds, the action is fired as a lifecycle operation with an empty param map and the `"schedule"` trigger.

> r[schedule.state]
> The runtime stores `(app_name, action_name, cronexpr, last_fired_at)` tuples durably. `last_fired_at` is updated on each successful fire.

> r[schedule.startup-grace]
> On startup, for schedule entries whose cron expression interval is 10 minutes or greater, the fire window is extended from 59 seconds to 5 minutes. This covers short runtime restarts without causing a storm of catch-up fires for frequent schedules.

> r[schedule.prune]
> When a BSL script is evaluated, the runtime must prune schedule state rows that no longer match any `(action, cronexpr)` pair declared in the script.

> r[schedule.audit]
> Scheduled action fires must be recorded in the audit log as lifecycle operations with the `"schedule"` trigger.

> r[schedule.start-reject]
> Calling `on_schedule` on the Start Action (action name `"start"`) must throw at script evaluation time.

### Backup Scheduling

> r[backup.schedule]
> Backup strategies use named schedule buckets: `"every hour"`, `"twice a day"`, `"every day"`. Snapshots are taken on the round boundary:
>
> - `"every hour"`: top of each hour (xx:00 UTC).
> - `"every day"`: midnight UTC (00:00).
> - `"twice a day"`: midnight UTC and noon UTC (00:00, 12:00).

> r[backup.schedule.delay]
> After taking a BTRFS snapshot, the runtime applies a random delay before invoking the backup app's `save-snapshot` action. The delay is uniformly distributed between 0 and 10% of the schedule interval:
>
> - `"every hour"`: 0–6 minutes.
> - `"twice a day"`: 0–72 minutes.
> - `"every day"`: 0–144 minutes.
>
> The delay is freshly randomised on each fire (not pinned per strategy).

### Backup Execution

> r[backup.execution]
> When a backup strategy fires (scheduled or manual), the runtime executes one `save-snapshot` operation per source volume in the strategy, serialised through the backup app's single-active-operation slot.
>
> For each source volume:
>
> 1. Take a BTRFS snapshot of the source volume.
> 2. Apply the random delay (scheduled fires only; manual fires skip the delay).
> 3. Create operation-scoped volume bindings:
>    - A read-only binding for the source snapshot (name chosen internally, e.g. `_backup-src-<run_id>`).
>    - A tmpfs binding for output (name chosen internally, e.g. `_backup-out-<run_id>`).
> 4. Invoke the backup app's `save-snapshot` action with params:
>    - `strategy`: strategy name.
>    - `run_id`: unique UUID for this operation.
>    - `source`: `{ "app": "<app>", "volume": "<volume>" }` or `{ "site": "<volume>" }`.
>    - `snapshot_taken_at`: RFC 3339 timestamp of the snapshot.
>    - `source_volume`: internal name of the source volume binding.
>    - `output_volume`: internal name of the output volume binding.
>    - `output_filename`: `"result.json"`.
> 5. On completion, read the output file if present and under 1 KB; store contents in the audit log entry.
> 6. Clean up: remove operation-scoped bindings, delete the BTRFS snapshot, delete the tmpfs volume.

> r[backup.execution.retry]
> If a `save-snapshot` invocation fails, the runtime retries once with a fresh random delay (using the same snapshot). If the retry also fails, the runtime raises a fault, cleans up the snapshot, and waits for the next scheduled fire. The fault clears when the next fire for this volume succeeds.

> r[backup.execution.per-volume-failure]
> Failure of one volume's backup does not prevent other volumes in the same strategy from being processed. Each volume is independent.

### Backup Fire-Time Validation

> r[backup.validation.fire-time]
> At fire time, the runtime validates each source volume:
>
> - The source volume must exist and be BTRFS-snapshottable.
> - The backup app must be installed and running.
>
> If the backup app is unavailable, a `backup_app_unavailable` fault is raised and the entire strategy fire is skipped. The fault clears when the backup app becomes available.
>
> If a source volume is missing or not snapshottable, a per-volume `backup_source_unavailable` fault is raised, that volume is skipped, and the remaining volumes proceed.

### Backup List and Restore

> r[backup.list]
> The `list-snapshots` action receives params: `strategy`, `source` (optional), `output_volume`, `output_filename`. The backup app writes a JSON Lines file to the output path. Each line must be a JSON object with at least `id` (string) and `timestamp` (RFC 3339). Additional fields are passed through.
>
> The runtime reads the file after action completion and returns the parsed entries to the caller.

> r[backup.restore]
> The `restore-snapshot` action receives params: `strategy`, `snapshot_id`, `source`, `target_volume`, `output_volume`, `output_filename`. The `target_volume` is bound to a fresh managed site volume (read-write). The backup app extracts the identified snapshot into the target.
>
> On success, the site volume persists and its name is returned to the operator.
> On failure, the site volume is cleaned up.

---

## Implementation Plan

### Phase 1: Action and Shell Params

**Goal**: All action closures take `|rt, param|`, shells take `|rt, shell, param|`.

1. **Generalise the requirements/param threading in the scheduler** (src/runtime/scheduler.rs).
   - Change the install-requirements slot to a general `Option<serde_json::Value>` (or `rhai::Map`).
   - Thread through `spawn_accepted_operation` (src/oi/handler/actions/lifecycle.rs).

2. **Persist params in the action execution log** (src/runtime/barrier/replay.rs, src/runtime/db).
   - Add a `params` column to the operation record.
   - On replay, deserialise and inject into the scope.

3. **Unify closure call to always pass `(rt, param)`** (src/runtime/barrier/replay.rs:372-380).
   - Remove the arity-detection try/catch for actions.
   - Install closures: remove the separate `__bsl_reqs` path; fold into `param`.
   - `on_change` closures remain `|rt, old|` (separate code path).

4. **ShellControl type** (src/runtime/barrier/shell.rs).
   - New `ShellControl` CustomType with `attach(job)` and `error(msg)`.
   - `attach`: delegates to existing `__bsl_shell_attach_impl` logic, sets a one-shot flag, throws on second call.
   - `error`: stores message in `ShellAttachCtx`, throws to end closure.
   - Update shell closure call to `closure.call(rt, shell_control, param)`.
   - Remove the try/catch fallback for the one-arg shell form.

5. **Update BSL definitions** (src/defs/app/action.rs, src/defs/app/shell.rs, src/defs/app/install.rs).
   - No structural changes needed (closures are FnPtrs, arity is the caller's concern).

6. **Param key validation** (src/oi/handler/actions.rs, src/oi/shells/).
   - Reject params with keys ending in `_volume` from operator input.

7. **CLI positional params** (src/ctl/apps.rs).
   - Parse `[key[=value]]...` after the action/shell name.
   - Bare key → `true`, `key=value` → string.

8. **Migrate existing scripts** (*.seed.rhai).
   - `|rt|` → `|rt, _param|` (or `|rt, param|`).
   - Shell closures → `|rt, shell, _param|`, replace `attach.call(job)` with `shell.attach(job)`.

9. **Update specs** (language.md, interface.md).

10. **Tests**.

### Phase 2: Scheduled Actions

**Goal**: Actions can have cron schedules; reconciler fires them.

1. **Add `cronexpr` dependency** (Cargo.toml).

2. **`on_schedule` builder method on Action** (src/defs/action.rs, src/defs/app/action.rs).
   - `ActionDef` gains `schedules: Vec<String>` (cron expressions).
   - `on_schedule(expr)` validates the expression and appends.
   - Throw if action name is `"start"`.

3. **Schedule state table** (src/runtime/db).
   - Table: `action_schedules(app TEXT, action TEXT, cronexpr TEXT, last_fired_at TEXT, PRIMARY KEY (app, action, cronexpr))`.

4. **Schedule tick in reconciler** (src/runtime, likely near the barrier-check step).
   - Once per minute (track last-check timestamp; skip if <60s since last).
   - For each row, compute next fire via `cronexpr`. If within last 59s, enqueue.
   - Startup grace: extend window to 5 minutes for intervals >= 10 minutes.

5. **Schedule pruning on script evaluation** (src/runtime/barrier/replay.rs or src/runtime/apps).
   - After evaluating a script, diff `ActionDef.schedules` against stored rows.
   - Delete rows that no longer match.

6. **Trigger field on OperationStarted** (src/oi/handler/actions/lifecycle.rs, src/runtime/audit.rs).
   - Add `trigger` to operation records and event emissions.

7. **Update specs** (language.md, runtime.md, interface.md).

8. **Tests**.

### Phase 3: Dynamic External Volumes in Action Scope

**Goal**: `app.external_volume(name)` in action scope resolves operation-scoped bindings.

1. **Operation-scoped binding store** (src/runtime/barrier/runtime.rs or new module).
   - Thread-local or context-carried map: `name → host_path + read_only`.
   - Populated before an action closure runs, cleared after.

2. **Update ExternalVolume resolution** (src/defs/app/volume.rs, src/system/translate/).
   - When in action scope, check the binding store first.
   - Fall back to the static external_volume_mappings table.

3. **Tests**.

### Phase 4: Backup App Registration

**Goal**: Operators can register apps as backup apps with validation.

1. **Backup app registry table** (src/runtime/db).
   - Table: `backup_apps(name TEXT PRIMARY KEY, app TEXT UNIQUE)`.

2. **Registration RPC** (src/oi/handler, new backups.rs or similar).
   - `/backups/apps/register { name, app }`: validate app has `save-snapshot`, `list-snapshots`, `restore-snapshot` actions.
   - `/backups/apps/deregister { name }`: reject if strategies reference it.
   - `/backups/apps/list {}`.

3. **Continuous validation on script update** (src/oi/handler/apps.rs).
   - On `/apps/update`, if app is a registered backup app, validate new script.
   - Wire into `/apps/plan` (dry-run) response.

4. **CLI hint** (src/ctl/apps.rs).
   - On `apps create`, evaluate script; if backup-shaped, print suggestion.

5. **CLI subcommands** (src/ctl, new backups.rs or similar).
   - `ctl backups apps register|deregister|list`.

6. **Update specs** (interface.md).

7. **Tests**.

### Phase 5: Backup Strategies

**Goal**: Operators can create, manage, and manually trigger backup strategies.

1. **Strategy table** (src/runtime/db).
   - Table: `backup_strategies(name TEXT PRIMARY KEY, via TEXT, schedule TEXT, volumes TEXT)`.
   - `volumes` stored as JSON array.

2. **Strategy CRUD RPCs** (src/oi/handler/backups.rs).
   - `/backups/strategies/create|list|show|update|delete`.
   - Validate `via` references a registered backup app.
   - Validate `schedule` is one of the named buckets.

3. **Manual trigger RPC** (src/oi/handler/backups.rs).
   - `/backups/run { strategy, volume? }`.

4. **CLI subcommands** (src/ctl/backups.rs).
   - `ctl backups strategies create|list|show|update|delete`.
   - `ctl backups run`.
   - `--allow-missing` flag for volume existence check.

5. **Update specs** (interface.md).

6. **Tests**.

### Phase 6: Backup Execution

**Goal**: Scheduled and manual backups execute end-to-end.

1. **Backup scheduler tick** (src/runtime, alongside the schedule tick from Phase 2).
   - Check due strategies against named bucket rules (round boundaries).
   - For each due strategy × volume, enqueue backup operations.

2. **Backup operation orchestration** (new module, src/runtime/backups.rs or similar).
   - BTRFS snapshot of source volume.
   - Random delay (10% of interval, skip for manual).
   - Create operation-scoped bindings (source snapshot read-only, output tmpfs).
   - Invoke `save-snapshot` via the scheduler.
   - On completion: read output file, audit log, cleanup.
   - On failure: retry once with fresh delay, then fault + cleanup.

3. **Fault types** (src/runtime/faults).
   - `backup_app_unavailable`.
   - `backup_source_unavailable`.
   - `backup_failed`.

4. **Audit log entries for backup events** (src/runtime/audit.rs).
   - Include output file contents (if present and under 1 KB).

5. **Tests**.

### Phase 7: Backup List and Restore

**Goal**: Operators can list snapshots and restore from backups.

1. **List-snapshots sync RPC** (src/oi/handler/backups.rs).
   - `/backups/snapshots/list { strategy, source? }`.
   - Invoke `list-snapshots` action, await completion, read output file, return.

2. **Restore RPC** (src/oi/handler/backups.rs).
   - `/backups/restore { strategy, snapshot_id, source }`.
   - Create fresh managed site volume.
   - Bind as `target_volume` (read-write).
   - Invoke `restore-snapshot`, await completion.
   - Return site volume name on success; cleanup on failure.

3. **CLI subcommands** (src/ctl/backups.rs).
   - `ctl backups snapshots list`.
   - `ctl backups restore`.

4. **Tests**.
