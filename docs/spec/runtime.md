The Seedling Runtime is the component responsible for making the real world match what a BSL script declares.

It evaluates BSL scripts, observes the state of the system, and continuously reconciles the two.
It also executes action closures defined in BSL, which direct the runtime through lifecycle transitions such as starting, upgrading, and installing an application.

Absent specification bugs, anything that is not defined here is either defined in another spec document (the language spec, the operator interface spec), or is implicitly not allowed.

# Reconciliation

> r[reconciliation.loop]
> The runtime must run a continuous reconciliation loop.
> Each iteration of the loop must:
>
> 1. Observe the current state of the world.
> 2. Record observations to the [world observation history](#r--history.world).
> 3. Derive the [lifecycle state](#r--lifecycle.states) of each resource from its observation history.
> 4. If a [lifecycle operation](#r--operation.lifecycle) is in progress and suspended on a [barrier](#r--barrier.suspension), check whether the barrier condition is satisfied. If so, resume the action closure.
> 5. Compute the difference between the [desired state](#r--desired-state.definition) and the derived state.
> 6. For each intended operation, consult the [autonomous operations log](#r--history.operations) to decide whether to proceed, back off, or file a [fault](#r--fault.definition).
> 7. Execute the operations that pass evaluation.
> 8. Record each executed operation (with [provenance](#r--history.operations.provenance)) to the autonomous operations log.

> r[reconciliation.convergence]
> The reconciler must be convergent: given a stable desired state and a functioning system, repeated iterations must bring the world closer to the desired state, and eventually reach it.

> r[reconciliation.idempotency]
> All operations performed by the reconciler must be idempotent.
> Performing the same operation twice must not cause errors or duplicate side effects.

> r[reconciliation.liveness]
> Individual reconciliation operations must not block the loop for an unbounded or long duration.
> When an operation requires waiting for an external condition (e.g. a process to terminate), the reconciler must release control and re-evaluate the condition on a subsequent iteration rather than polling inline.

# Script Engine Limits

> r[engine.limits]
> The runtime must constrain the BSL script engine to prevent unbounded resource consumption.

> r[engine.limits.operations]
> The engine must enforce a maximum number of operations per script evaluation.
> When the limit is reached, evaluation must fail with an error.
> The default limit is 100 000 operations.
> This limit may be overridden at startup.

> r[engine.limits.call-depth]
> The engine must enforce a maximum function call nesting depth.
> The default limit is 64.
> This limit may be overridden at startup.

> r[engine.limits.expr-depth]
> The engine must enforce a maximum expression nesting depth.
> The default limit is 64.
> This limit may be overridden at startup.

> r[engine.limits.string-size]
> The engine must enforce a maximum string length in bytes.
> The default limit is 1 048 576 (1 MiB).
> This limit may be overridden at startup.

> r[engine.limits.array-size]
> The engine must enforce a maximum array size in elements.
> The default limit is 10 000.
> This limit may be overridden at startup.

> r[engine.limits.map-size]
> The engine must enforce a maximum object map size in entries.
> The default limit is 10 000.
> This limit may be overridden at startup.

# Desired State

> r[desired-state.definition]
> The desired state is the set of resources that should exist and their intended [lifecycle states](#r--lifecycle.states).
>
> The desired state is derived from two inputs:
>
> 1. The AppDef: the resource graph produced by evaluating the BSL script.
> 2. The progress of the current [lifecycle operation](#r--operation.lifecycle), if any: which resources have been started or stopped by the action closure so far.

> r[desired-state.steady]
> When no lifecycle operation is active, the desired state is the full AppDef.
> The reconciler maintains it autonomously: restarting terminated containers according to their restart policy, maintaining scale, and keeping networking and ingress configuration consistent.

> r[desired-state.during-operation]
> When a lifecycle operation is in progress, the desired state is built incrementally by the action closure's `rt.start()` and `rt.stop()` calls.
> The reconciler must only act on resources that the operation has placed into the desired state so far.
> Resources that are not in the desired state during an operation are left untouched: any running instances continue to run with whatever spec they were last started with, and no rotation is performed against them until the operation either schedules them explicitly or completes (returning the app to steady state).

> r[desired-state.during-install]
> An install operation obeys the same rules as any other lifecycle operation under [desired-state.during-operation](#r--desired-state.during-operation).
> While an app is in the `Installing` status, the reconciler must actuate resources the install closure has placed into the desired state, exactly as it would for any other in-progress operation.
> The fact that the app has never previously been installed does not exempt the reconciler from actuating these resources.

# Generation

> r[generation.definition]
> A *generation* is a monotonically increasing per-app counter that uniquely identifies the application's defined state at a point in time.
> Each generation corresponds to a specific `(script, parameter values)` pair.
> The current generation of an app determines the AppDef that the reconciler maintains in steady state.

> r[generation.bumps]
> The generation of an app must be incremented (by exactly one) on each of:
>
> - The initial registration of the app.
> - A successful script update.
> - A successful parameter set or unset.
>
> The bump must be committed to durable storage atomically with the change that caused it.
> If the change triggers a lifecycle operation (such as an `on_change` handler), the operation is dispatched after the generation has been committed.

> r[generation.monotonic]
> Generations must be strictly monotonic for the lifetime of an app.
> A failed lifecycle operation (for example, a failing `on_change` handler) must not roll back the generation: the change to the defined state has already been committed, and the failure is recorded as the operation's [outcome](#r--generation.history).
> An operator may file a corrective change as a new generation; the failed generation remains visible in history.

> r[generation.history]
> The runtime must maintain a durable per-app history of generations.
> Each generation history entry must contain:
>
> - The generation number.
> - A timestamp of when the generation was created.
> - The kind of change: `Register`, `ScriptUpdate`, `ParamSet`, or `ParamUnset`.
> - For `ParamSet` and `ParamUnset`: the parameter name, the previous value (`Option<Value>`), and the new value (`Option<Value>`). A `ParamSet` from an unset state has a previous value of `None`; a `ParamUnset` has a new value of `None`.
> - For `Register` and `ScriptUpdate`: the content hash of the script registered at this generation.
> - The identity of the lifecycle operation triggered by this change, if any.
> - The outcome of that operation: `Pending` (still running or queued), `Succeeded`, or `Failed` (with details).

> r[generation.script-storage]
> Script bodies must be stored content-addressed by hash.
> Multiple generations whose script content is identical share a single stored script body.
> A generation history entry references its script by hash; the script body is retrieved by looking up the hash.

> r[generation.reconstruction]
> Given a generation number N for an app, the runtime must be able to reconstruct the AppDef as it was at generation N by:
>
> 1. Looking up generation N's script hash and loading the corresponding script body.
> 2. Building the parameter map at generation N: for each parameter that has any history entry at or before N, the value is taken from the most recent `ParamSet` or `ParamUnset` entry at or before N. A `ParamUnset` (or absence of any entry) yields `None`.
> 3. Evaluating the script with that parameter map.

> r[generation.previous]
> The *previous generation* of generation N is generation N − 1.
> Reconstruction of the previous generation is required to materialise the `old` argument of [`on_change`](#l--param.on-change.old) handlers.

> r[generation.deregister]
> When an app is deregistered, its generation history and all stored generation data (parameter values, script bodies referenced only by this app) must be deleted as part of teardown.
> A subsequent registration of an app with the same name begins a new generation lineage from generation 1; the two registrations are independent for all runtime purposes.
>
> Forensic reconstruction of a prior app's history (across a deregister/re-register cycle) is the responsibility of the [audit log](#r--audit.log), not the per-app generation history.

# Lifecycle

## States

> r[lifecycle.states]
> Every resource instance tracked by the runtime is in exactly one lifecycle state at any time:
>
> - **Pending**: the resource is in the desired state but no action has been taken to realise it.
> - **Scheduled**: the resource's underlying primitives have been created but are not yet operational.
> - **Running**: the resource is operational but has not yet been confirmed ready.
> - **Ready**: the resource is operational and has passed any applicable readiness criteria.
> - **Terminating**: the runtime has initiated termination but the resource has not yet stopped.
> - **Terminated**: the resource has stopped.
> - **Unscheduled**: the resource's underlying primitives have been cleaned up.

> r[lifecycle.transitions]
> The normal transition order is Pending → Scheduled → Running → Ready → Terminating → Terminated → Unscheduled.
>
> A resource may skip states: for example, a container that exits on its own transitions directly from Running (or Ready) to Terminated, without passing through Terminating.

> r[lifecycle.derivation]
> Lifecycle states must be derived from the [world observation history](#r--history.world), not maintained as an independent state machine.
> The runtime must be able to re-derive the current lifecycle state of any resource from its observation history at any time.

## Resource Lifecycle Semantics

> r[lifecycle.container]
> For container resources (Deployments, Jobs):
>
> - **Scheduled**: the container image pull has been initiated and/or the container has been created.
> - **Running**: the container process is executing.
> - **Ready**: the container is running and any configured health checks are passing.
> - **Terminated**: the container process has exited (with an exit code).

> r[lifecycle.service]
> For Service resources:
>
> - **Scheduled**: the internal network plumbing exists.
> - **Ready**: at least one backend is healthy and traffic can be routed.
> - **Terminated**: the network plumbing has been torn down.

> r[lifecycle.ingress]
> For Ingress resources:
>
> - **Scheduled**: the ingress configuration has been submitted.
> - **Ready**: the ingress is accepting external traffic and certificates are valid.
> - **Terminated**: the ingress configuration has been removed.

> r[lifecycle.volume]
> For Volume resources:
>
> - **Scheduled**: the volume directory exists and any declared writes have been applied.
> - **Ready**: the volume is available for mounting.
> - **Terminated**: the volume has been removed.

> r[lifecycle.external-volume]
> For External Volume resources, the lifecycle is managed entirely by the runtime:
>
> - **Ready**: immediately upon reconciliation, since the resource is a declaration only. The presence or absence of a mapping is tracked via faults, not lifecycle state.
> - **Unscheduled**: immediately upon uninstall.

# Persistent History

> r[history.persistence]
> All history records must be stored durably and must survive runtime restarts, including unexpected termination and node power loss.

> r[history.storage]
> The storage mechanism must support transactional writes and efficient queries by resource identity and time range.

> r[template.persist]
> Stored templates (see [template.definition](interface.md#i--template.definition)) must be persisted durably and must survive runtime restarts.
> Template removal must be durable: a removed template must not reappear after restart.
> Templates are independent of app persistence: removing a template has no effect on apps previously instantiated from it.

## World Observation History

> r[history.world]
> The world observation history is a timeline of observations per resource instance.
> Each entry is a timestamped, structured record of something the runtime observed about the real world.

> r[history.world.entries]
> Each observation entry must contain:
>
> - A timestamp.
> - The resource identity (type, name, and instance identifier if scaled).
> - The observation kind (e.g. container status, exit event, health check result, network reachability).
> - The observed value or payload.

> r[history.world.source]
> Observations come from the runtime's interaction with the underlying system.
> The runtime must not fabricate observations; every entry must correspond to a real check or event.

> r[history.world.state-derivation]
> The runtime must be able to derive a resource's current [lifecycle state](#r--lifecycle.states) and the time of its last state transition from the world observation history alone.

## Autonomous Operations Log

> r[history.operations]
> The autonomous operations log records operations the reconciler performed without direction from an action closure.

> r[history.operations.entries]
> Each autonomous operation entry must contain:
>
> - A timestamp.
> - The resource identity affected.
> - The operation performed (e.g. start container, rebuild ingress config).
> - The provenance.
> - The outcome (success or failure, and details if failure).

> r[history.operations.provenance]
> Every autonomous operation must record its provenance: the specific observation(s) that triggered it and the rule that applied.
>
> Examples of provenance:
> - "Container exited (observation at T), OnTerminate policy is Recreate."
> - "Observed 1 running instance, scale requires 2."
> - "Ingress controller unreachable (observation at T), rebuilding configuration."

> r[history.operations.rate-limiting]
> The runtime must use the autonomous operations log to detect repeated operations on the same resource and apply backoff.
> The backoff strategy must prevent tight loops (e.g. a container that exits immediately after starting must not be restarted indefinitely at full speed).

## Action Execution Log

> r[history.action-log]
> The action execution log records the progress of a [lifecycle operation](#r--operation.lifecycle) through its action closure.

> r[history.action-log.entries]
> Each action execution log entry must contain:
>
> - A timestamp.
> - The lifecycle operation identity.
> - The `rt.*` call that was made (e.g. start, stop, warm_certs), and on which resources.
> - The barrier condition, if a barrier was reached (which resources, which state, what deadline).
> - Whether the barrier has been satisfied.

> r[history.action-log.replay]
> The action execution log must contain enough information to replay an interrupted lifecycle operation from the beginning and fast-forward to the interruption point.

# Audit Log

> r[audit.log]
> The runtime must maintain a durable, append-only audit log capturing significant system events. The audit log is stored separately from operational data so that operational tables can be garbage-collected without losing the historical record.

> r[audit.log.path]
> The audit log must be written to a configurable file path, defaulting to `/var/log/seedling/audit.log`. The runtime must create the parent directory if it does not exist.

> r[audit.log.format]
> Each audit log entry must be a single line of JSON (JSON Lines format). Each entry must include a timestamp and an event type.

> r[audit.log.events]
> The audit log must record at minimum:
>
> - App registration and deregistration.
> - Lifecycle operation start, completion, and failure.
> - Faults filed and cleared.
> - Resource state transitions.
> - Site volume creation, deletion, snapshotting, and promotion.
> - External volume mapping creation, deletion, and retargeting.
> - Template creation, removal, and instantiation.

> r[audit.log.generations]
> Audit entries that record changes to an app's defined state — including registration, script update, parameter set, and parameter unset — must include the [generation](#r--generation.definition) of the change.
> For changes that supersede a prior generation (any change other than initial registration), the entry must also include the previous generation.
> Lifecycle operation audit entries triggered by a generation change must include the source and target generations from the operation record.

> r[audit.log.rotation]
> The audit log file must be compatible with external log rotation tools. The runtime must detect when the log file has been rotated (renamed or removed) and reopen the configured path.

> r[audit.log.resilience]
> Failure to write to the audit log must not block or crash the runtime. Audit write failures must be reported via tracing.

# Garbage Collection

> r[gc.background]
> The runtime must periodically remove stale rows from operational database tables to bound storage growth. Garbage collection must run as a background task on a configurable interval, defaulting to one hour.

> r[gc.action-log]
> Completed lifecycle operation entries in the action log must be deleted after a configurable retention period (default: 24 hours). Entries belonging to the current in-flight operation must never be deleted.

> r[gc.faults]
> Cleared fault records must be deleted after a configurable retention period (default: 7 days).

> r[gc.observations]
> World observation rows whose instance identity no longer appears in the resource instance registry must be deleted.

> r[gc.autonomous-operations]
> Completed autonomous operation records must be deleted after a configurable retention period (default: 7 days).

> r[gc.instances]
> Resource instance records that have remained in the Unscheduled lifecycle state for longer than a configurable retention period (default: 10 minutes) must be deleted, along with their associated world observation rows.
> Instances that are part of the active desired state (i.e. in the `keep` set of a scaled group or a singleton) must never be deleted regardless of their lifecycle state.

# Lifecycle Operations

> r[operation.lifecycle]
> A lifecycle operation is the top-level unit of scripted orchestration.
> It is a single execution (possibly interrupted and replayed) of an action closure.

> r[operation.lifecycle.single]
> At most one lifecycle operation may be in progress across all applications on a node at any time.

> r[operation.lifecycle.single.intra-app]
> If a lifecycle operation is requested for an application that already has one in progress, the request must be rejected immediately.

> r[operation.lifecycle.single.inter-app]
> If a lifecycle operation is requested for an application while a different application's operation is in progress, the request must be queued.
> There may be at most one queued operation per application.
> If an operation is already queued for that application, the new request must be rejected.
> Queued operations are started in request order once the current operation completes.

> r[operation.lifecycle.events]
> Lifecycle operations are initiated by these events:
>
> - **Normal boot** (prior state exists, no interrupted operation): the `start` action.
> - **Boot, interrupted operation exists**: replay of the interrupted operation. This includes installs that were in progress when the runtime stopped; an app persisted in the `Installing` status must have its install replayed from the persisted params and action log.
> - **Param change**: the `on_change` handler registered on the parameter that changed, when the change matches one of the [transitions](#l--param.on-change.transitions) in the language spec, the app is installed, and a handler is registered.
> - **Operator request**: a named action, including `install`.
> - **Schedule**: a BSL `on_schedule` cron expression fires.

> r[operation.lifecycle.param-change]
> A param change is a lifecycle operation.
> It is subject to the same [concurrency restrictions](#r--operation.lifecycle.single) as all other lifecycle operations.
> Only one parameter may be changed at a time.

> r[operation.lifecycle.generations]
> Every lifecycle operation record must carry a *source generation* and a *target generation*.
>
> - For an operation triggered by a [generation bump](#r--generation.bumps) (param change, script update), the source generation is the generation that was current immediately before the bump, and the target generation is the new generation produced by the bump.
> - For an operation not triggered by a generation bump (operator-invoked actions, `start`, replay), the source and target generations are equal — the current generation at the time of dispatch.
>
> Replay of an interrupted operation must reconstruct the AppDef using the operation's stored target generation, and `old` (when applicable) using the source generation.
> This must hold even if further generations have been committed in the meantime; the operation always sees the world it was scheduled for.

> r[operation.lifecycle.completion]
> When a lifecycle operation completes (the action closure returns), the full [desired state](#r--desired-state.steady) — derived from the AppDef at the app's current generation — takes effect and the reconciler maintains it autonomously.
> The operation's outcome is recorded in the [generation history](#r--generation.history) entry that triggered it (when applicable).

> r[operation.params]
> When a lifecycle operation is dispatched with params, the params must be persisted alongside the operation record. On replay, the persisted params must be restored and passed to the action closure.
> Params may contain secret values, so the persisted form must be encrypted with the same cipher used for stored secret params.
>
> Shell sessions do not persist params; shells are not replayable.
> Backup operations do not persist params; they bind per-process snapshot paths that cannot survive a runtime restart, so interrupted backups are dropped and the next scheduled fire takes over.

## Operation Volume Params

Some internal operations (for example [backup.list](#r--backup.list), [backup.restore](#r--backup.restore), [backup.execution](#r--backup.execution)) must hand a writable or read-only path to an action closure without giving that path a stable, app-visible name. The runtime uses reserved param keys and operation-scoped volume bindings to do this in a collision-free way.

> r[operation.volume-param]
> When an internal operation hands a volume to an action closure, it must not use a fixed name hard-coded in the BSL script. Instead, for each logical binding the operation wants to provide, the runtime must:
>
> 1. Generate a fresh name that is unique for the operation — typically `seedling-op-<operation-id>-<logical-key>`. The name must not appear anywhere in the operator-provided param map.
> 2. Register that name in the operation-scoped volume binding table (see [volume.external.dynamic](language.md#l--volume.external.dynamic)), mapping it to the actual filesystem path and the intended `read_only` flag. The binding lives for the duration of the operation and is removed when the operation ends.
> 3. Insert a param named `<logical-key>_volume` into the `param` map delivered to the action closure, whose value is the generated name.
>
> The action closure resolves the volume by reading the param, then calling `app.external_volume(param["<logical-key>_volume"])`.
>
> Because the generated name is produced by the runtime, not chosen by the script, different operations cannot collide with each other and cannot collide with operator-configured static external volume mappings.

> r[operation.volume-param.filename]
> An operation may additionally instruct an action closure to read or write a specific filename under an injected volume. It must not rely on a fixed filename known only to the runtime: instead, for each `<logical-key>_volume` param it adds, the runtime may add a companion param named `<logical-key>_filename` whose value is the filename the action closure must use.
>
> This keeps the filename an implementation detail of the runtime — the runtime may rename it or change its structure without requiring every backup app's BSL script to be rewritten.

> r[operation.volume-param.reserved]
> Param keys ending in `_volume` or `_filename` are reserved. The runtime must reject operator-provided params whose keys end in either suffix (see [action.invoke](interface.md#i--action.invoke) and [action.params](language.md#l--action.params)). Only the runtime may insert such keys, and only as described in [operation.volume-param](#r--operation.volume-param) and [operation.volume-param.filename](#r--operation.volume-param.filename).

## Action Composition

> r[operation.composition]
> An action closure may invoke other actions by calling `rt.start()` on a resource of type Action.
> The invoked action's closure runs inline within the calling operation.
> Barriers within the invoked action are barriers of the overall operation.

> r[operation.composition.cycles]
> The runtime must detect cycles in action invocation.
> If an action closure invokes an action that is already on the current call stack (directly or transitively), the invocation must throw.

## Shell Sessions

> r[operation.shell]
> Shell actions are not lifecycle operations.
> They may run concurrently with a lifecycle operation and with other shell sessions.

> r[operation.shell.resources]
> Resources created within a shell session are dynamic.
> They are cleaned up when the session ends.

# Scheduled Actions

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

# Backup Scheduling

> r[backup.schedule]
> Backup strategies use named schedule buckets: `"every hour"`, `"twice a day"`, `"every day"`. Snapshots are taken on the round boundary:
>
> - `"every hour"`: top of each hour (xx:00 UTC).
> - `"every day"`: midnight UTC (00:00).
> - `"twice a day"`: midnight UTC and noon UTC (00:00, 12:00).

> r[backup.schedule.delay]
> Before executing a backup strategy, the runtime must apply a random delay of 0–10% of the schedule interval:
>
> - `"every hour"` (3600 s interval): 0–360 s delay.
> - `"twice a day"` (43200 s interval): 0–4320 s delay.
> - `"every day"` (86400 s interval): 0–8640 s delay.
>
> For manually triggered backups (`/backups/run`), no delay is applied.

> r[backup.schedule.catch-up]
> If the runtime detects that one or more scheduled fire times for a strategy have
> already passed (for example because the daemon was not running at the moment of
> the scheduled fire), the strategy fires exactly once to catch up, regardless of
> how many scheduled boundaries were missed. The strategy's last-fired timestamp
> is then set to the catch-up fire time, so subsequent cron boundaries are
> evaluated from there.

> r[backup.validation.fire-time]
> Before executing a backup strategy, the runtime must verify:
>
> 1. The registered backup app still exists in the backup app registry.
> 2. The backing app is registered in the app registry and exposes all required backup actions.
>
> If either check fails, a `backup_app_unavailable` fault is filed against the backing app name and execution is aborted.

> r[backup.execution]
> For each volume in the strategy (in declaration order), the runtime must:
>
> 1. Resolve the volume identifier to a filesystem path. If the path does not exist, file a
>    `backup_source_unavailable` fault against the backup app and skip the volume.
> 2. Create a read-only snapshot of the source volume at a temporary path.
> 3. Acquire the scheduler slot (waiting until no other operation is active).
> 4. Run the backup app's `save-snapshot` action with the snapshot handed in under the logical
>    binding key `"source"`, using the scheme described in [operation.volume-param](#r--operation.volume-param). The action closure reads the generated volume name from `param["source_volume"]`.
> 5. After the action completes (success or failure), remove the snapshot.
>
> On success, any existing `backup_failed` and `backup_source_unavailable` faults for the backup app are cleared.

> r[backup.execution.per-volume-failure]
> Volume backups within a strategy are independent: a failure for one volume must not prevent
> the remaining volumes from being backed up.

> r[backup.execution.retry]
> If the `save-snapshot` action fails, the runtime must retry exactly once after a fresh random
> delay (following the same rules as `r[backup.schedule.delay]`). If the retry also fails, a
> `backup_failed` fault is filed against the backup app with the failing volume noted in the
> description.

> r[backup.list]
> To list available snapshots for a volume, the runtime must synchronously invoke the backup app's
> `list-snapshots` action. Using the scheme described in [operation.volume-param](#r--operation.volume-param) and [operation.volume-param.filename](#r--operation.volume-param.filename), the action receives:
>
> - A `backup` param carrying the structured identity of the operation (see the interface spec's `backup.action.backup-param`).
> - A writable volume provided under the logical binding key `"output"`. The action closure reads
>   the generated volume name from `param["output_volume"]`.
> - A companion `param["output_filename"]` naming the file that the action must write. The
>   runtime chooses the filename; the action must write to exactly that name within the output
>   volume.
>
> The runtime reads and returns the contents of that file as the result.

> r[backup.restore]
> To restore a snapshot, the runtime must:
>
> 1. Create a new site volume with a generated name.
> 2. Synchronously invoke the backup app's `restore-snapshot` action. Using the scheme described in [operation.volume-param](#r--operation.volume-param), the action receives:
>    - A param `"snapshot"` containing the snapshot identifier.
>    - A `backup` param carrying the structured identity of the operation (see the interface spec's `backup.action.backup-param`).
>    - A writable volume provided under the logical binding key `"destination"` pointing at the new site volume. The action closure reads the generated volume name from `param["destination_volume"]`.
> 3. If the action fails, remove the site volume and propagate the error.
> 4. Return the site volume name.

# Action Closure Suspension

> r[barrier.suspension]
> When an action closure calls a barrier method (`.scheduled()`, `.running()`, `.ready()`, `.terminated()`) on a `Started` value, the closure must be suspended.
> Execution appears to block from the script's perspective.

> r[barrier.condition]
> A barrier condition specifies which resource instances must reach which lifecycle state.
> The barrier is satisfied when all specified resources have reached (or passed) the required state, as determined by the [world observation history](#r--history.world.state-derivation).

> r[barrier.deadline]
> Each barrier has a deadline.
> If the barrier condition is not satisfied within the deadline, the barrier must throw an exception within the action closure.

> r[barrier.resume]
> When a barrier condition is satisfied, the runtime must resume the suspended action closure.
> The closure continues from the point immediately after the barrier call.

## Replay

> r[barrier.replay]
> If the runtime restarts while a lifecycle operation is in progress, the operation must be replayed:
>
> 1. The BSL script is re-evaluated to reconstruct the AppDef.
> 2. The [action execution log](#r--history.action-log) is read from persistent storage.
> 3. The action closure is re-executed from the beginning.
> 4. `rt.*` calls that are already recorded in the log are idempotent: they produce the same desired state mutations but do not duplicate real-world operations.
> 5. Barrier calls whose conditions are already satisfied (according to the current world observation history) return immediately.
> 6. Execution fast-forwards to the first unsatisfied barrier, where it suspends normally.

> r[barrier.replay.determinism]
> BSL action closures must not have side effects beyond `rt.*` calls.
> Re-execution of a closure given the same AppDef and parameters must produce the same sequence of `rt.*` calls up to any given barrier point.

> r[barrier.replay.rt-stop]
> The `rt.stop()` method also blocks (until resources terminate).
> It must participate in the same suspension and replay mechanism as barrier methods.

# Autonomous Reconciliation

> r[autonomous.restart]
> When a container resource in the desired state reaches the Terminated lifecycle state and its `on_exit` or `on_terminate` policy requires recreation, the reconciler must start a replacement.

> r[autonomous.job-terminal]
> A Job instance that has naturally reached the Terminated lifecycle state must not be restarted by the reconciler.
> The reconciler must clean up any lingering container or unit state for such an instance but must not start a replacement.
> This applies both while a lifecycle operation is in progress (the job completed during the operation) and in steady state.

> r[autonomous.job-terminal.defense]
> The reconciler must remember which Job instances have completed within the current process lifetime.
> If a remembered completed Job instance is subsequently observed running again (e.g. restarted externally), the reconciler must stop it.

> r[autonomous.scale]
> When a Deployment resource's observed running instance count differs from its declared scale, the reconciler must start or stop instances to converge on the declared count.

> r[scaling.decision]
> The runtime maintains a persistent record of operator-chosen scale for each Deployment that has been explicitly scaled.
> When present, this record determines the effective scale used by the reconciler instead of the lower bound default.

> r[scaling.clamp]
> When an app's BSL script is re-evaluated (e.g. via an update), any stored scaling decision for a Deployment whose declared bounds have changed must be clamped to the new bounds.
> If the stored value falls below the new lower bound it is raised; if it exceeds the new upper bound it is lowered.
> Clamping is applied before the reconciler acts on the updated definition.

> r[autonomous.ingress]
> When an Ingress resource's configuration in the ingress controller does not match the desired configuration, the reconciler must update or rebuild the configuration.

> r[autonomous.network]
> When a Service resource's network plumbing is missing or misconfigured, the reconciler must recreate or repair it.

> r[autonomous.provenance-required]
> Every autonomous operation must record [provenance](#r--history.operations.provenance) before execution.

# Observation

> r[observe.facts]
> The runtime must collect timestamped observation facts for each resource instance by inspecting the backing system primitives.

> r[observe.deployment]
> For Deployment and Job resource instances, the runtime must observe pod network presence, container lifecycle state (missing, created, running, or exited), and systemd unit state.

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
> Cert observation drives both the [`Ready` lifecycle state](#r--lifecycle.ingress) of an ingress and the satisfaction of [`rt.warm_certs`](#l--rt.warm-certs) barriers.

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

# Actuation

> r[actuate.deployment.start]
> Starting a Deployment or Job instance must ensure the pod network exists for the instance, ensure the container image is present (pulling if absent), and start the container under process supervision.

> r[actuate.deployment.stop]
> Stopping a Deployment or Job instance must stop the supervised container process and, once stopped, remove the pod network and any anonymous volumes that were created for that instance.

> r[actuate.deployment.anon-volume.start]
> When starting a Deployment or Job instance, for each anonymous volume mounted by the container, the runtime must ensure the volume exists and apply any declared file writes to it before the container starts.

> r[actuate.container.hardening]
> Workload containers must be started with all Linux capabilities dropped, privilege escalation disabled, and a read-only root filesystem with a writable tmpfs at `/tmp`. A default PID limit of 256 and a file-descriptor limit of 65536 are applied. BSL configuration may adjust these defaults.

> r[actuate.container.journal-metadata]
> Workload containers must have their stdout and stderr directed to the system journal.
> Each supervised container process must be tagged with structured journal fields that
> identify the owning app, resource kind, resource name, and instance. These fields
> must be present on every journal entry produced by the container so that log queries
> can filter at any granularity (app, resource, or individual instance) without
> relying on unit name conventions.

> r[actuate.infra.journal-metadata]
> Infrastructure containers (proxy, resolver) must have their stdout and stderr directed
> to the system journal. Each infrastructure container must be tagged with a structured
> journal field that identifies the infrastructure component so that log queries can
> target infrastructure logs independently of workload logs.

> r[actuate.ingress.warm-certs]
> When an action closure invokes [`rt.warm_certs`](#l--rt.warm-certs) with a selection that contains TLS-terminating ingresses, the runtime must initiate certificate acquisition for those ingresses' hostnames without exposing the ingresses to live traffic.
> A typical implementation pushes a partial proxy configuration that requests certificate acquisition while not routing requests to any backend; once the certificate is `valid`, it is served from the proxy's cache when the same ingress is later started for real.
>
> The warm_certs call must be recorded in the [action execution log](#r--history.action-log) and must be idempotent on replay: a subsequent invocation with the same selection observes the existing cert state and returns immediately when `valid`, without re-initiating acquisition.

> r[actuate.volume.start]
> Starting a Volume instance must create the named volume if it does not already exist, then apply any declared file writes to the volume.

> r[actuate.volume.tmpfs]
> When starting a tmpfs-backed Volume, the runtime must create the volume with the tmpfs driver. Because tmpfs contents do not survive a host reboot, any declared file writes must be re-applied unconditionally — not only when the volume is first created.

> r[actuate.volume.storage]
> Named non-tmpfs volumes are stored as host-managed directories under the data directory and bind-mounted into containers. Anonymous volumes remain managed by the container runtime.

> r[actuate.volume.btrfs]
> When the data directory resides on a BTRFS filesystem, named non-tmpfs volumes must be created as BTRFS subvolumes.

> r[actuate.volume.hold]
> When a named non-tmpfs volume or a managed or snapshot site volume would be removed — either because the volume name has been removed from the app definition, because the volume's storage backend cannot be updated in-place, or because an operator has requested deletion of a managed or snapshot site volume — the runtime must preserve the volume's data in a held state instead of deleting it. The held volume remains linked to the originating context: app volumes to their app, and site volumes to the site (reported as app name `_site`). If the app still requires a volume under the same name, a fresh volume is created and the old one is held alongside it.

> r[actuate.volume.hold.confirm]
> An operator must explicitly confirm deletion of a held volume before its data is removed.

> r[actuate.volume.hold.events]
> Creation and confirmed deletion of held volumes must be observable on the event feed so that UIs can refresh held-volume counts without polling.

> r[volume.site]
> A site volume is a named volume managed by operators, independent of any app. Site volumes come in two kinds:
>
> - **Managed**: a host directory (or BTRFS subvolume) created and maintained by the runtime under the data directory.
> - **Bind**: an arbitrary host path provided by the operator, mounted as-is.
>
> Site volumes may be read-write or read-only. Tmpfs site volumes are not supported.

> r[volume.site.lifecycle]
> Site volumes are created and deleted exclusively through operator commands. The runtime must create the backing storage for managed site volumes at creation time. When a managed or snapshot site volume is deleted, its backing storage must be routed through the [held volume](#r--actuate.volume.hold) mechanism so that an operator must explicitly confirm final removal. Deleting a bind site volume must only drop the runtime's reference and must not affect the operator-provided host path.

> r[volume.site.lifecycle.events]
> Site volume creation and deletion must emit events on the event feed and be recorded in the [audit log](#r--audit.log). Deletion events must identify the site volume's kind and, when the deletion routed through the held-volume mechanism, the resulting held volume identifier.

> r[volume.site.snapshot]
> A snapshot site volume is a read-only point-in-time snapshot of a named volume (app volume or managed site volume) that supports snapshotting. Only BTRFS-backed volumes support snapshotting. Snapshot site volumes carry metadata identifying their source. They are inherently read-only: even when mapped without the read-only flag, mounts of snapshot site volumes are always read-only.

> r[volume.site.snapshot.events]
> Creating a snapshot site volume must emit an event identifying the new snapshot and the source volume (app-scoped or site-scoped).

> r[volume.site.promote]
> An operator may promote a snapshot site volume to a fresh managed site volume with an operator-chosen name. The runtime must materialise the new volume as a writable copy seeded from the snapshot's contents and record it as a managed site volume. The source snapshot is not modified and remains available; operators may independently delete it afterwards if they wish. Promotion is only supported on BTRFS-backed installations because it relies on the same snapshotting primitive as `volume.site.snapshot`.

> r[volume.site.promote.events]
> Promoting a snapshot site volume must emit an event identifying the new managed site volume and the source snapshot. The promoted volume is treated as a new managed site volume for audit purposes, distinct from the source snapshot which remains intact.

> r[volume.external.mapping.events]
> Creating, removing, or retargeting an operator-configured external volume mapping must emit an event identifying the app and external volume name, the new mapping target (or absence of one for removal), and — for retargeting — the previous target. These events feed both the event feed and the [audit log](#r--audit.log).

> r[actuate.volume.stop]
> Stopping a Volume instance must remove the named volume.

# Update Strategies

> r[update.spec-hash]
> Each running container carries a spec hash that captures its full configuration at start time.
> The reconciler must compare the observed spec hash of each running instance against the desired spec hash derived from the current definition.
> An instance whose observed hash differs from the desired hash (or whose hash is absent) is _stale_.

> r[update.rolling]
> When a Deployment's update strategy is Rolling and stale instances are detected, the reconciler must rotate instances incrementally:
>
> 1. Temporarily increase the effective instance count by one beyond the current scale.
> 2. Start a new instance with the current definition in the additional slot.
> 3. Wait until the new instance is running and healthy before proceeding.
> 4. Stop one stale instance.
> 5. Repeat from step 2 until no stale instances remain.
> 6. Return the effective instance count to the declared scale.
>
> At no point during a rolling update may the number of healthy instances drop below the Deployment's scale lower bound,
> except when all instances are stale and no healthy replacement exists yet (the initial ramp-up).

> r[update.rolling.over-provision]
> The temporary over-provisioning required by a rolling update must be reflected in the desired state computation
> so that the additional instance is not treated as excess by the scaling machinery.
> The reconciler must track which deployments have an active rolling update and feed this into the effective scale calculation.

> r[update.rolling.restart-resume]
> A rolling update must resume correctly after a runtime restart.
> Because the runtime re-derives rollout state from observed spec hashes on each tick, no explicit rollout-in-progress flag is required in persistent storage.
> If the runtime restarts mid-rollout, it must observe the running containers, detect any remaining stale instances, and continue the rotation from where it left off.

> r[update.rolling.reboot-resume]
> After a full node reboot (no containers surviving), all instances are missing.
> The reconciler must start all instances with the current definition.
> Because no stale containers exist, no rolling update coordination is needed and the deployment converges directly to the desired state.

> r[update.replace]
> When a Deployment's update strategy is Replace and stale instances are detected, the reconciler must stop all stale instances before starting replacements.
> This may temporarily violate the Deployment's scale lower bound.
> New instances are started only after all stale instances have been fully stopped.

> r[update.jobs]
> Jobs do not participate in rolling or replace update coordination.
> A stale Job instance is stopped and restarted immediately.

# Fault Handling

> r[fault.definition]
> A fault is a condition where the runtime determines that convergence is impossible or that a persistent failure pattern exists.
> Faults are not handled by BSL scripts.

> r[fault.detection]
> The runtime must detect faults from patterns in the [world observation history](#r--history.world) and [autonomous operations log](#r--history.operations).
>
> Examples of fault conditions:
> - A [barrier deadline](#r--barrier.deadline) expires.
> - A container repeatedly terminates shortly after starting and backoff has been exhausted.
> - A resource cannot be created due to a persistent external condition.

> r[fault.image-pull]
> When the reconciler fails to pull a container image required by a resource instance, it must file a fault of kind `image_pull_failed` associated with that instance.
> The fault is cleared automatically when a subsequent pull of the same image succeeds.

> r[fault.container-start]
> When the reconciler observes that a resource instance's backing unit is in a failed state while the desired state is active, it must file a fault of kind `container_start_failed` associated with that instance.
> The fault is cleared automatically when the unit is subsequently observed in an active or activating state.

> r[fault.external-volume-unmapped]
> When a deployment or job instance requires an external volume that has no mapping in the external volume mapping table, the reconciler must not start the instance and must file a fault of kind `external_volume_not_mapped` against that instance, identifying the missing volume name.
> The fault is cleared automatically when a subsequent reconciliation tick is able to start the instance (i.e. the mapping has been added and the start succeeds).

> r[fault.cert-acquisition]
> When TLS certificate acquisition for an ingress hostname fails persistently (after the proxy's own retry policy has been exhausted), the runtime must file a fault of kind `cert_acquisition_failed` associated with that ingress, identifying the hostname and the most recent acquisition error.
> The fault is cleared automatically when a subsequent acquisition for the same hostname is observed as `valid`.

> r[fault.surfacing]
> Faults must be surfaced to operators through the operator interface (defined in a separate spec).
> The runtime must not silently discard faults.

> r[fault.non-blocking]
> A fault on one resource must not prevent the reconciler from continuing to manage other resources.
> The faulted resource is excluded from active reconciliation until the fault is resolved.

# Resource Identity

> r[identity.stable]
> The runtime must assign a stable identity to each resource instance.
> The identity must be consistent across reconciliation ticks and runtime restarts.

> r[identity.components]
> A resource identity consists of:
>
> - An opaque instance ID, assigned once at creation and never changed. For most resource types the ID is randomly generated; see [Job instance identity](#r--identity.job) for the exception.
> - The application name.
> - The resource type.
> - The resource name (if not anonymous).
> - Whether the instance is a singleton or one of a scaled group.

> r[identity.scaled]
> Scaled resources (e.g. a Deployment with `scale(N)`) produce N instances.
> Each instance must have a distinct, stable opaque identifier so the runtime can track
> instances independently across restarts and scaling events.
> Instance identifiers must not imply ordering or position; a random value must be used
> so that removing one instance does not make the remaining identifiers appear discontinuous.
> A human-readable display name for each instance is derived at creation time from the
> instance ID and stored stably alongside the identity.

> r[identity.anonymous]
> Anonymous resources (those without a name) must receive a runtime-assigned identity that is stable for the lifetime of the resource but does not conflict with named resources.

> r[identity.job]
> Job instances have a deterministic identity scheme rather than a randomly-generated one, because concurrent executions of the same Job definition must be distinguishable without persisting state across restarts.
>
> - **Static Jobs** (defined in the top-level BSL scope) have a fixed all-zero instance ID. Their display name includes the all-zero suffix (e.g. `myapp-worker-00000000`). There is at most one static instance per Job name; the identity is fully deterministic.
> - **Dynamic Jobs** (defined inside an action closure) have an instance ID derived from the lifecycle operation's `OperationId` and the Job's name via a deterministic UUID v5 derivation: `UUID_v5(operation_id, "job:{name}")`. This gives a stable identity within one operation execution (including across barrier replay passes) while ensuring that distinct operation invocations produce distinct container names, allowing concurrent action runs involving the same Job name to coexist without collision.

> r[identity.job.shell]
> A Job used as the target of a shell `attach()` call receives a fresh randomly-generated instance ID chosen at the moment `attach()` is called, independent of any lifecycle operation ID. This allows multiple concurrent shell sessions to run against the same Job definition without collision.

# Startup

> r[startup.btrfs]
> At startup, the runtime must verify that the data directory resides on a BTRFS filesystem. If BTRFS is not available and the `--without-btrfs` flag has not been passed, the runtime must exit with an error that mentions the `--without-btrfs` flag.

# Infrastructure

> r[infra.key.file-permissions]
> Private key files must be created with owner-read/write-only permissions. The runtime
> must refuse to use a key file whose group or world permission bits are set, and must
> report an error.

> r[infra.db.file-permissions]
> The database file must be created with owner-read/write-only permissions. The runtime
> must refuse to open a database file whose group or world permission bits are set, and
> must report an error.

> r[infra.db.busy-timeout]
> When a database write is blocked by a concurrent writer, the runtime must wait and retry
> rather than failing immediately.

> r[infra.node.prefix]
> The runtime must derive a stable per-node /48 IPv6 prefix from the host machine identity.
> The prefix follows the ULA format `fd5e:edXX:XXXX::/48`, where the 24-bit host portion is
> derived from `/etc/machine-id`.

> r[infra.pod.network]
> Each pod instance must be connected to a dedicated IPv6-only bridge network. The network
> prefix is a /64 derived from the node prefix and the instance identity. No IPv4 subnet
> must be allocated on pod networks.

> r[infra.pod.mount]
> Pods that mount a service must resolve the `localmount` hostname to a stable node-wide
> address at `fd5e:XXYY:ZZWW:fffe::1`. nftables DNAT rules scope traffic to this address
> by source pod prefix, so multiple pods may mount the same service simultaneously without
> collision. The mount endpoint address does not need to be assigned to any network
> interface.

> r[infra.proxy.startup]
> The runtime must ensure the network proxy is running and healthy before beginning the
> reconciliation loop. On each startup, the runtime verifies proxy health and restarts the
> proxy if necessary.

> r[infra.proxy.upgrade]
> When the configured proxy image digest differs from the running container's image digest,
> the runtime must upgrade the proxy using a blue/green strategy: the replacement container
> is started and fully configured before any traffic is directed to it. The name of the
> currently-active proxy container is persisted in the database so that a crash at any
> point in the upgrade sequence can be recovered deterministically on the next startup.

> r[infra.proxy.upgrade.cache]
> The runtime must persist the last successfully applied proxy configuration so that it can
> be applied to a replacement container during an upgrade before the traffic cutover occurs,
> ensuring no window exists where the new container receives traffic without a valid
> configuration.

> r[infra.dataplane.output-nat]
> The runtime must install DNAT rules in an nftables `output` chain (in addition to the
> `prerouting` chain) so that host-originated traffic directed at ingress ports on any
> local address is redirected to the proxy. The output rules must be restricted to
> locally-destined packets (`fib daddr type local`) and to the IPv6 address family, and
> must mirror the port and protocol set of the prerouting ingress rules.

> r[infra.dataplane.service-dnat]
> The runtime must install DNAT6 rules in the nftables `prerouting` chain that translate
> traffic destined for a service's stable IPv6 address and declared service port to a
> backing pod's address and pod-side port. When multiple backends are available, the
> runtime must distribute new connections across them. These rules are applied atomically
> alongside ingress and mount rules.

> r[infra.dataplane.mount-dnat]
> Mount DNAT rules must translate directly from the pod's mount endpoint address and mount
> port to a backing pod's address and pod-side port, without an intermediate hop through
> the service address. When multiple backends are available, the runtime must distribute
> new connections across them.

> r[infra.dataplane.forward-policy]
> The nftables forward chain must drop unsolicited new inbound forwarded connections directed
> at any address within the node's /48 prefix. The forward chain must explicitly permit:
> conntrack-established and related connections; new connections that have been redirected by
> a prerouting NAT rule (ingress traffic); and traffic whose source and destination both fall
> within the node's /48 prefix. The chain's default policy must remain accept so that
> forwarded traffic unrelated to the node's /48 prefix is not affected.

## Resolver

> r[infra.resolver]
> The runtime must run a resolver infrastructure container that provides DNS forwarding and
> caching to all workload containers. The resolver container follows the same lifecycle as
> the proxy container: it is started when workloads are present and torn down when no
> workloads remain.

> r[infra.resolver.config]
> The runtime must generate a resolver configuration that forwards all queries to the
> configured upstreams (see [infra.resolver.upstreams](#r--infra.resolver.upstreams)) and
> caches responses. When [NAT64 is active](#r--infra.nat64.mode), the configuration must
> additionally enable DNS64 synthesis of AAAA records under the well-known prefix
> `64:ff9b::/96`.

> r[infra.resolver.upstreams]
> The runtime must accept a command-line argument that specifies an explicit list of
> upstream DNS servers (`host:port` addresses) for the resolver to forward to. When the
> argument is unset, the runtime must arrange for the resolver to forward to the host's
> system DNS (so containers inherit the host's split-DNS and search-domain configuration);
> this arrangement must work even when the host system DNS listens only on loopback
> addresses unreachable from inside the resolver container.

> r[infra.resolver.address]
> The resolver must listen on a stable node-wide IPv6 address derived from the node prefix,
> so that all workload containers can reach it at a predictable address.

> r[infra.resolver.startup]
> The runtime must ensure the resolver container is running and healthy before beginning the
> reconciliation loop. On each startup, the runtime verifies resolver health and restarts
> the resolver if necessary.

> r[infra.resolver.upgrade]
> When the resolver container image changes, the runtime must upgrade it using the same
> blue/green strategy used for the [proxy](#r--infra.proxy.upgrade).

## Container DNS

> r[infra.pod.dns]
> Every workload container must be configured to use the [resolver](#r--infra.resolver) as
> its DNS server. The runtime must pass the resolver's address to the container runtime so
> that the container's `/etc/resolv.conf` points at the resolver.

## NAT64

> r[infra.nat64.mode]
> The runtime must accept a NAT64 mode setting via a command-line argument. The accepted
> values are:
>
> - `auto` (default) — the runtime probes for existing NAT64 infrastructure on startup and
>   enables its own NAT64 only if none is detected.
> - `enabled` — the runtime always provides NAT64.
> - `disabled` — the runtime never provides NAT64.

> r[infra.nat64.detection]
> In `auto` mode, the runtime must detect existing NAT64 infrastructure by resolving the
> well-known name `ipv4only.arpa` (RFC 7050) for AAAA records. If the response contains a
> synthesised AAAA record (i.e. an address outside the `ipv4only.arpa` canonical addresses
> `192.0.0.170` and `192.0.0.171`), the network already provides NAT64 and DNS64; the
> runtime must not activate its own. Otherwise, the runtime must activate NAT64. Detection
> is performed once at startup.

> r[infra.nat64.translator]
> When NAT64 is active, the runtime must configure a stateful NAT64 translator using the
> well-known prefix `64:ff9b::/96`. The translator must be operational before any workload
> containers are started.

> r[infra.nat64.translator.lifecycle]
> The runtime must ensure the NAT64 translator is configured on every startup and must
> remove it during graceful shutdown. If the translator cannot be initialised and NAT64 is
> required (`enabled` mode or `auto` mode with no external NAT64 detected), the runtime
> must report an error and file a fault.

> r[infra.nat64.forwarding]
> When NAT64 is active, the runtime must ensure that IPv6 and IPv4 forwarding are enabled on
> the host, and that a route for `64:ff9b::/96` exists so that container traffic destined
> for the NAT64 prefix reaches the translator.

> r[infra.nat64.dns64]
> When NAT64 is active, the [resolver configuration](#r--infra.resolver.config) must enable
> DNS64 so that lookups for IPv4-only names return synthesised AAAA records under
> `64:ff9b::/96`. When NAT64 is not active, DNS64 synthesis must be disabled in the
> resolver configuration.

# Secret Parameter Storage

> r[secret.key]
> The runtime must provision and maintain a secret key for use in protecting secret parameter values.
> The key file must have the same file-permission restriction as the main database (access restricted to the owner).
> If no key file exists on startup the runtime must create one automatically with a freshly generated key.
> The key file must be stored on the same volume as the database.

> r[secret.storage]
> Secret parameter values (those whose effective `secret` flag is `true`) must be stored separately from non-secret parameters and must be protected at rest using the [secret key](#r--secret.key).
> Non-secret parameter values continue to be stored unprotected.
> The two storage locations are transparent to BSL scripts: `app.param(name).value()` returns the decrypted string regardless of whether the value is secret.

> r[secret.history]
> Entries in the [generation history](#r--generation.history) that record a previous or new value for a secret parameter must protect those values using the [secret key](#r--secret.key).
> History retrieval must decrypt these values internally before serving them to callers that are authorised to reconstruct past generations.

> r[secret.redaction]
> When a secret parameter value is requested via an operator interface (such as a describe or history RPC), it must never be returned to the caller.
> The redaction decision is based on whether the parameter is currently secret in the live AppDef, regardless of what the schema was when the value was originally stored.
> Absence of the value must be signalled by a machine-readable marker so clients can distinguish "not set" from "set but redacted".

> r[secret.migration]
> When a parameter transitions from non-secret to secret (because the BSL script is updated or the `secret` flag is changed), the runtime must move the existing stored value from non-secret storage to secret storage at the next opportunity, without requiring operator intervention.
