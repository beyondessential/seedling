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
> - **Ready**: the container is running and, if a [healthcheck](language.md#l--deployment.healthcheck) is declared, it is currently passing.
> - **Terminated**: the container process has exited (with an exit code).
>
> A container with no declared healthcheck becomes Ready as soon as it is Running. A container with a declared healthcheck remains in Running until the first passing observation is recorded.

> r[lifecycle.container.unhealthy-transition]
> A container observed as unhealthy must transition from Ready back to Running.
> A subsequent healthy observation must transition it forward to Ready again.
> These transitions are derived from the [world observation history](#r--history.world) in the same manner as all other lifecycle state derivations and must not require an independent state machine.

> r[lifecycle.service]
> For Service resources:
>
> - **Scheduled**: the internal network plumbing exists.
> - **Ready**: at least one backend is in the routing pool and traffic can be routed.
> - **Terminated**: the network plumbing has been torn down.

> r[service.http.route.routing]
> URL-prefix routing is a property of the Service. When an HTTP-terminating Ingress (whether declared by the service's own app or attached as a [site ingress](#r--ingress.site.attachment)) fronts a Service whose backing pods declare per-prefix [HTTP route](language.md#l--service.http.route) bindings, the runtime must route each request to the pods that bound the matching URL prefix, not to the Service's general routing pool.
>
> Concretely: for every `deployment.http(pod_port, svc.route(prefix))` binding on a pod backing the ingress's target service, the runtime emits one ingress route at `prefix` with upstreams set to the running backend pods that hold that binding. The proxy must select the longest matching prefix for each incoming request.
>
> When the backing service has no `http_bindings` at all (e.g. an HTTPS-fronted TCP-only service), the runtime falls back to a single `/` route through the Service's general routing pool.

> r[lifecycle.service.routing-pool]
> The routing pool for a Service is the set of backend pod instances eligible to receive traffic. The runtime selects the pool from the running backends as follows:
>
> - A backend pod enters the pool only after it has been observed [healthy](language.md#l--deployment.healthcheck) at least once. Pods still in start-period or that have never been observed healthy are not in the pool. A pod with no declared healthcheck is treated as healthy as soon as it is Running.
> - Once in the pool, a backend that is observed unhealthy is removed from the pool **only if** at least one other backend in the same Service is currently healthy. If no healthy alternative exists, all running backends remain in the pool (degraded mode) and the runtime files a fault per [fault.service-degraded](#r--fault.service-degraded).
> - When all backends are stopped, the pool is empty and the Service is no longer Ready.
>
> The intent is "prefer healthy, fall back to anything running": a single-server platform should never reduce serving capacity below what's currently available, but should always prefer healthy capacity when it exists.

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

> r[history.world.ordering]
> When multiple observations are emitted within the same tick they may share a timestamp. Queries that return observations for derivation must use a stable secondary ordering — insertion order — so that consumers see them in the order they were emitted, not in storage-engine-defined order.
> Without this, lifecycle derivation becomes non-deterministic for any resource that produces multiple observations per tick (e.g. a container that observes both `stop_sent` and `container_removed` in the same reconciliation pass).

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
>
> The runtime must additionally emit a `SubActionInvoked` entry immediately before each [`Action.invoke()`](language.md#l--action.call) runs the called closure. The entry records the called action's name and the validated params map. On replay, the entry is treated as already-emitted: the runtime re-enters the FnPtr but recovers the params from the log rather than re-validating, so a schema change between operation start and replay does not desync the call. The `SubActionInvoked` entries are exposed in `apps history` so operators can inspect the nested call chain.

> r[history.action-log.replay]
> The action execution log must contain enough information to replay an interrupted lifecycle operation from the beginning and fast-forward to the interruption point.

> r[rt.signal]
> The runtime persists each [`rt.signal`](language.md#l--rt.signal) invocation to the action execution log so the call is not re-delivered when an interrupted operation replays. The persisted entry records the target instances and the signal name. On replay, the runtime treats a signal entry as already delivered and does not re-issue it; this is the at-most-once-across-replays guarantee from `l[rt.signal]`.

> r[rt.write]
> The runtime persists each [`rt.write`](language.md#l--rt.write) invocation to the action execution log. The persisted entry records the target volume and the write path; the file contents are not stored in the log. On replay, the runtime treats a write entry at the same call site as already applied and does not re-execute it; this is the at-most-once-across-replays guarantee from `l[rt.write]`.
>
> The runtime resolves the target volume to a host filesystem mountpoint using the same mechanism the actuator uses to apply static `Volume.write` content, then writes through a kernel-confined `openat2(RESOLVE_BENEATH)` so a malicious or buggy path cannot escape the volume's root.

> r[rt.exec]
> The runtime persists each [`rt.exec`](language.md#l--rt.exec) invocation to the action execution log. The persisted entry records the target container instance and the exit code. The argv and any options (e.g. env) are not stored in the log; only the outcome is needed for replay correctness. On replay, the runtime treats an exec entry at the same call site as already executed and recovers the exit code from the entry rather than re-running the command; this is the at-most-once-across-replays guarantee from `l[rt.exec]`.
>
> The runtime executes the command inside the target's running container via the container runtime (e.g. `podman exec`), inheriting the container's namespaces, working directory, user, and environment. Stdout and stderr are forwarded to the container-log sink for the target instance. The runtime blocks the action closure until the command exits, then returns control with the exit code attached to the `Executed` value.

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

> r[gc.instances.atomic]
> The deletion of an instance's records must be atomic across the world observation rows, fault rows, and the resource instance row.
> A partial deletion that leaves any of these three out of sync with the others is a defect: it allows orphan faults and registry rows to linger indefinitely, since subsequent garbage-collection passes select instances by observation history that no longer exists.

> r[gc.instances.never-actuated]
> A scaled instance that the desired state has demoted to Unscheduled must be retired immediately by the reconciler in any of the following cases:
>
> - The instance's lifecycle state, as derived from observations, is Unscheduled.
> - The instance has no observations at all (it was never actuated).
> - The instance has at least one terminal observation (`container_removed` or `network_cleaned_up`) in its history, even if the derived lifecycle state is something else.
>
> The third case is required because multiple observations recorded within the same tick may share a timestamp, and the lifecycle derivation is order-sensitive: a non-deterministic ordering can leave the derived state at `Pending` even though terminal observations exist. The presence of any terminal observation is sufficient evidence that the actuator successfully tore the instance down at some point, and waiting for the lifecycle derivation to "agree" would leave the registry slot occupied indefinitely.
> Without this rule the registry slot lingers forever, along with any faults filed against the instance during its brief actuation attempt.

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

> r[operation.cancel]
> The runtime must expose a way for operators to cancel an in-progress lifecycle operation.
>
> When cancellation is requested, the runtime must wake any currently-suspended barrier — including deadline-less barriers such as [`.terminated_eventually()`](#l--rt.started.terminated-eventually) — so that the cancel takes effect within the same observation cycle rather than waiting for the next poll or deadline.
>
> A cancelled operation reaches a terminal state distinct from success and from failure. Cleanup (dynamic resource teardown, current-operation clearing) must run as for a failed operation, and the outcome must be recorded so operators can tell a cancel apart from an ordinary failure after the fact.

> r[operation.cancel.persistence]
> A cancel request must persist across a runtime restart. When the daemon crashes after the cancel was accepted but before the operation observed it, the replay must resume into a pre-cancelled state so the operation terminates at its next barrier rather than re-executing work the operator has already asked to abandon.
>
> The persisted cancel flag is scoped to a single `operation_id`. A later operation — whether resumed after completion of the current one or spawned fresh — must not inherit a cancel from a stale row.

> r[operation.cancel.stuck-recovery]
> An app whose persisted phase is `Installing` but for which no `current_operation` row matches must be recoverable: the operator must always have a path to an actionable state.
>
> The runtime must implement this in two places. On daemon startup, after loading apps and the persisted current operation, any app stuck in `Installing` with no matching current_operation must be transitioned to `Uninstalling`. The cancel-action endpoint must, when called on an app in this state, perform the same transition and return success rather than failing with "no active operation".
>
> The transition target is `Uninstalling` rather than `NotInstalled` because a partially-installed app may already have running containers or units from `rt.start()` calls the previous operation made before its interruption. The reconciler skips `NotInstalled` apps, so a direct revert would strand those resources. `Uninstalling` engages the normal teardown path; the reconciler converges to `NotInstalled` once the teardown completes.
>
> Stuck states arise from a previous run whose cancel cleanup cleared `current_operation` but whose phase-revert persist was interrupted by crash or restart, or from a daemon that died between persisting the phase and persisting the operation row.

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
> An action closure may invoke other actions by calling [`Action.invoke(params?)`](language.md#l--action.call) on an Action handle obtained from [`app.action(name)`](language.md#l--action.lookup) or returned by `app.on_action()` / `app.on_start()`.
> The invoked action's closure runs inline within the calling operation, sharing its `rt`, its operation_id, and its action log.
> Barriers within the invoked action are barriers of the overall operation.

> r[operation.composition.cycles]
> The runtime must reject cycles in action invocation before the called closure runs.
> A `.call()` whose action name is already on the current call stack — directly (the action calling itself) or transitively (any earlier frame on the stack) — must throw.
> The check uses the action name; renaming a captured closure does not bypass it.
> The thrown error must name the offending chain, e.g. `start → foo → bar` for the `bar.call()` that closes the cycle, so the operator can identify the script bug.

> r[operation.composition.params]
> The runtime applies the called action's declared param schema to the supplied `params` map before invoking the closure, using the same validation rules as operator invocation: required-field enforcement, default application, [reserved-key](language.md#l--action.params) rejection, and `kind: "volume"` resolution against the [operation-scoped volume bindings](#r--operation.volume-param). Validation errors throw before the closure runs.
> The result of validation must be deterministic across replays so that the action log entry recorded for the call (see [history.action-log.entries](#r--history.action-log.entries)) faithfully describes what the closure observed.

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

> r[schedule.fire.installed-only]
> Schedules must only fire for apps in the `Installed` [phase](#r--lifecycle.app). Schedules attached to apps that are `NotInstalled`, `Installing`, or `Uninstalling` must be skipped. This prevents a permanently-failing app's schedule from monopolising the per-app scheduler slot and blocking operator updates: a script with a faulty install whose every cancel is followed by a fresh schedule fire would otherwise be unable to ever accept a corrected script.

> r[schedule.state]
> The runtime stores `(app_name, action_name, cronexpr, last_fired_at)` tuples durably. `last_fired_at` is updated on each successful fire.

> r[schedule.catch-up]
> If the runtime detects that a scheduled fire time has already passed (for example because the daemon was not running at the moment of the scheduled fire), the schedule fires exactly once to catch up, regardless of how many scheduled boundaries were missed. The schedule's `last_fired_at` is then set to the catch-up fire time, so subsequent cron boundaries are evaluated from there.

> r[schedule.prune]
> When a BSL script is evaluated, the runtime must prune schedule state rows that no longer match any `(action, cronexpr)` pair declared in the script.

> r[schedule.audit]
> Scheduled action fires must be recorded in the audit log as lifecycle operations with the `"schedule"` trigger.

> r[schedule.start-reject]
> Calling `on_schedule` on the Start Action (action name `"start"`) must throw at script evaluation time.

# Backup Scheduling

> r[backup.schedule]
> Backup strategies use named schedule buckets: `"every hour"`, `"twice a day"`, `"every day"`. Snapshots are taken on the round boundary in the system local timezone:
>
> - `"every hour"`: top of each hour (xx:00 local time).
> - `"every day"`: midnight local time (00:00).
> - `"twice a day"`: midnight and noon local time (00:00, 12:00).

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

> r[backup.run.last-fired]
> Both scheduled fires and manually triggered fires via `/backups/run` must
> update the strategy's last-fired timestamp to the moment the fire was
> initiated, before any per-volume work begins. This keeps the scheduler's
> next-fire computation consistent across triggers (so an immediately-due
> scheduled fire does not double-run right after a manual invocation) and
> lets the operator surface see the most recent fire promptly.

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

> r[backup.execution.startup-cleanup]
> The snapshot cleanup mandated by [backup.execution](#r--backup.execution) is best-effort within a single process: if the runtime is killed between snapshot creation and the post-action removal step, the snapshot is left on disk and would otherwise accumulate invisibly across restarts.
> On startup, the runtime must scan for any orphaned snapshots produced by [backup.execution](#r--backup.execution) — site volumes whose name identifies them as belonging to the backup-execution path rather than to a user-visible site volume — and delete them.

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
> When an action closure calls a barrier method (`.scheduled()`, `.running()`, `.ready()`, `.ready_eventually()`, `.terminated()`, `.terminated_eventually()`) on a `Started` value, the closure must be suspended.
> Execution appears to block from the script's perspective.

> r[barrier.condition]
> A barrier condition specifies which resource instances must reach which lifecycle state.
> The barrier is satisfied when all specified resources have reached (or passed) the required state, as determined by the [world observation history](#r--history.world.state-derivation).

> r[barrier.deadline]
> Each barrier has an optional deadline.
> When a deadline is set and the barrier condition is not satisfied within it, the barrier must throw an exception within the action closure.
> When the deadline is absent (as it is for `.terminated_eventually()` and `.ready_eventually()`), the barrier waits indefinitely; it resumes only when the condition becomes satisfied or when the operation is [cancelled](#r--operation.cancel).
> The condition check takes precedence over the deadline: a barrier whose condition is currently satisfied resumes successfully even if the deadline has elapsed since the original suspension. Otherwise, a short-lived resource that completed during a replay window where the satisfied flag never landed in the action log could be spuriously timed out on the next replay.

> r[barrier.suspension.poll-backoff]
> The cadence at which the runtime re-evaluates a suspended barrier (replaying the action closure up to the suspend point) may vary with how long the barrier has been waiting.
> Short initial cadence keeps quick barriers responsive; longer cadence for protracted waits bounds the aggregate replay cost of multi-hour operations.
> The exact schedule is not prescribed by this spec.

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

> r[autonomous.restart.backoff]
> Per-unit restarts must be paced so that a crash-looping container does not exhaust systemd's start-rate limit before the reconciler has a chance to detect the problem. Container units must specify:
>
> - A non-default `RestartSec` (no shorter than several seconds) so the unit does not retry at the systemd default cadence (~100ms).
> - A `StartLimitIntervalSec` and `StartLimitBurst` that allow several attempts within a window measured in minutes, not seconds, before systemd gives up.
>
> The exact values are an implementation concern — they need only be loose enough that a slow-failing container (one that takes seconds to crash) gets multiple chances, and tight enough that a permanently broken container reaches the start limit on a human-meaningful timescale.

> r[autonomous.restart.start-limit-hit]
> When a container unit reaches `failed/start-limit-hit` (systemd has refused further restarts because the unit exhausted [`StartLimitBurst`](#r--autonomous.restart.backoff)) the reconciler must:
>
> - File a `crash_loop` fault scoped to the offending instance, distinct from `container_start_failed`.
> - Stop attempting to auto-recover the instance (no `reset_failed_unit` + restart cycle) until the fault is cleared. The expected recovery path is operator intervention — fixing the underlying cause, redeploying with new config, or explicitly clearing the fault.
> - Clear the fault automatically if the instance is later observed healthy.

> r[autonomous.job-terminal]
> A Job instance that has naturally reached the Terminated lifecycle state must not be restarted by the reconciler.
> The reconciler must clean up any lingering container or unit state for such an instance but must not start a replacement.
> This applies both while a lifecycle operation is in progress (the job completed during the operation) and in steady state.
> A Job that the reconciler asked systemd to start in a prior tick and that the reconciler currently observes as gone counts as "naturally terminated" for the purposes of this rule, even when no `container_running` observation was ever recorded — short-lived jobs may complete and be auto-removed faster than the observer's poll interval, and forcing a re-start in that case would loop indefinitely.

> r[autonomous.job-terminal.defense]
> The reconciler must remember which Job instances have completed within the current process lifetime.
> If a remembered completed Job instance is subsequently observed running again (e.g. restarted externally), the reconciler must stop it.

> r[autonomous.scale]
> When a Deployment resource's observed running instance count differs from its declared scale, the reconciler must start or stop instances to converge on the declared count.

> r[autonomous.healthcheck-replace]
> When a Deployment instance with [`on_failure: "replace"`](language.md#l--deployment.healthcheck.on-failure) is observed unhealthy, the reconciler must:
>
> - Spawn a replacement instance for the same Deployment so that the unhealthy instance has a healthy sibling-in-progress.
> - Inhibit any stop of the unhealthy instance while the replacement is starting; the unhealthy instance must keep its routing-pool membership in degraded mode (per [lifecycle.service.routing-pool](#r--lifecycle.service.routing-pool)) until the replacement is observed healthy.
> - Once the replacement is observed healthy, retire the unhealthy instance through the normal scale-down path so the routing pool converges on the healthy backend.
>
> Instances with `on_failure: "monitor"` must not trigger this behaviour. Their routing-pool membership still follows the prefer-healthy rule, but no replacement is spawned.

> r[autonomous.healthcheck-replace.guard]
> A fresh instance brought up by either a [rolling update](#r--update.rolling) or by [autonomous.healthcheck-replace](#r--autonomous.healthcheck-replace) must itself become healthy within its declared `start_period + retries × interval`. If it does not, the reconciler must:
>
> - Stop the failed instance so it does not accumulate a permanent footprint.
> - File a fault per [fault.healthcheck-replace-failed](#r--fault.healthcheck-replace-failed).
> - Refuse to spawn further replacement attempts for this Deployment — whether rolling-driven or healthcheck-driven — until the AppDef generation changes (i.e. the operator pushes new code) or the operator clears the fault.
>
> The pre-existing instance must continue to run in degraded mode (under its old spec hash if it was a rolling-update target). The runtime must not allow the guard to trigger an indefinite churn of replacement attempts; the guard exists precisely to bound the cost of a permanently-broken workload — whether the breakage was introduced by a script change or arose spontaneously.

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

> r[actuate.image.warm]
> When an action closure invokes [`rt.warm_images`](#l--rt.warm-images) with a selection that contains container resources, the runtime must collect the distinct image references from those resources' container definitions and initiate pulls for each image that is not already present locally.
> For each extracted image reference the runtime must durably record a [pin](#r--image.pin) tying the calling app to the reference, upserted idempotently.
>
> The warm_images call must be recorded in the [action execution log](#r--history.action-log) and must be idempotent on replay: a subsequent invocation with the same selection observes the existing local image state and returns immediately when all selected images are present.

> r[image.pin]
> A _pin_ is a durable `(app, image_reference)` record indicating that an app has requested the referenced image be warmed.
> While a pin exists it must protect its image reference from autonomous removal (see [image.gc](#r--image.gc)).
> The runtime must remove a pin whenever it observes a running container whose image reference matches that pin, so that a warmed image transitions from _pinned_ to _in use_ automatically on first observed use.
> A pin must not be automatically re-created when a workload stops using an image; a subsequent `rt.warm_images` call is required to re-pin.
> Operators may clear any pin explicitly via the operator interface.

> r[image.pin.expiry]
> A pin may carry an optional expiration timestamp. When an expiration is set and passes, the reconciler must delete the pin on a subsequent tick exactly as if an operator had cleared it explicitly.
> Setting, clearing, and reading an expiration does not change any other aspect of the pin's lifecycle — in particular, an expired-but-not-yet-swept pin still protects its image from [autonomous removal](#r--image.gc) until the reconciler sweeps it.
> Expirations are only set by the post-update reconciliation rule (see [`image.pin.update-reconcile`](#r--image.pin.update-reconcile)); they are cleared whenever a pin's reference is observed to be valid again for the owning app.

> r[image.pin.update-reconcile]
> Whenever an app's `AppDef` changes because of an operator-driven edit (a script update or a parameter change that triggers re-evaluation), the runtime must re-probe the new script and use the result to reconcile that app's pins:
>
> 1. Build the _safe set_: every image reference declared by a `Deployment` or `Job` in the new `AppDef`, plus every image reference in the probe's [`all_images`](#r--image.discover) output.
> 2. For any pin whose reference is in the safe set, clear its expiration (if any) so the pin is kept indefinitely.
> 3. For any pin whose reference is _not_ in the safe set:
>    - If the probe ran without errors _and_ without skipped handlers, the safe set is authoritative: delete the pin.
>    - Otherwise the safe set is incomplete: set the pin's expiration to 30 days from now, unless an earlier expiration is already set.
>
> A pin's reference is "in the safe set" based on exact string equality; a tag-pinned reference and a digest-pinned reference to the same image id are treated as distinct.
> The rule fires only on operator-driven AppDef changes; autonomous reconciliation ticks do not run probes.

> r[image.track]
> The runtime must maintain, for each locally-present container image, a _last-used_ timestamp initialised to the time the image was first observed locally.
> On every reconciliation pass, the last-used timestamp of every image whose `image_id` appears on at least one running container must be updated to the current time.

> r[image.gc]
> At a regular interval matching the background garbage collector cadence, the runtime must remove locally-present container images that satisfy all of the following:
>
> - No [pin](#r--image.pin) exists for any reference pointing at the image.
> - No running container is currently observed using the image.
> - The image's [last-used timestamp](#r--image.track) is at least 30 days in the past.
>
> Removal failures must be logged and must not prevent removal of other images in the same pass.
> Autonomous image GC must not be extended with shorter retention periods or more aggressive policies without explicit operator configuration; operators remove images they no longer need through the operator interface.

> r[image.discover]
> The runtime must provide a _probe_ execution mode that runs an app's handler closures without any world-modifying side effects, in order to enumerate every container image reference the handler might pull. The probe must:
>
> - Drive every `rt.*` call whose normal semantics would mutate state or wait on the world. `rt.start(...)` and `rt.warm_images(...)` extract image references from the supplied container resources and record them to the probe output without scheduling anything or writing pins. `rt.stop(...)`, `rt.warm_certs(...)`, `rt.restart(...)`, and `rt.query(...)` are no-ops.
> - Satisfy every barrier immediately: `.scheduled()`, `.running()`, `.ready()`, `.ready_eventually()`, `.terminated()`, `.terminated_eventually()`, and `rt.stop(...)`'s implicit barrier all return as if their condition is met. `Termination::ensure_success()` succeeds unconditionally.
> - Not record anything to the action log, the autonomous operations log, the world observation history, or the `image_pins` table.
>
> The probe is invoked per handler and operates on a handler's captured closure, the current `AppDef`, and a resolved param map supplied by the caller. Error-path image references (those only reached via a closure branch predicated on `termination.ensure_success()` failing, or a similar live-state signal) are necessarily missed by the probe, since the probe always returns the success path.

> r[image.discover.params]
> The probe accepts a param map per handler. Parameter resolution proceeds in priority order:
>
> 1. Values supplied by the probe caller override everything.
> 2. Otherwise, the app's persisted param store supplies the value.
> 3. Otherwise, the param schema's `default_value` is used.
> 4. Otherwise, the param is unset.
>
> Probing may operate in either of two modes:
>
> - **Strict**: when a handler declares a required parameter that remains unresolved after step 3, the probe for that handler must return an error identifying the missing parameters without invoking the closure.
> - **Lenient**: the same condition causes the handler to be reported as _skipped_ with the same error payload, without invoking the closure or producing any images.
>
> Closures that throw during probing must be reported with the thrown message as their error; images accumulated before the throw must be returned alongside.

> r[actuate.volume.start]
> Starting a Volume instance must create the named volume if it does not already exist, then apply any declared file writes to the volume.

> r[actuate.volume.tmpfs]
> Tmpfs-backed Volumes (named or anonymous) must be materialised as a host-managed directory under a tmpfs-backed location and bind-mounted into containers. The runtime must not delegate tmpfs storage to the container runtime's own tmpfs volume driver, because such drivers create a fresh tmpfs at every container mount and so silently drop any declared file writes that were applied to the volume's host backing.
> Because tmpfs contents do not survive a host reboot, any declared file writes must be re-applied unconditionally each time the volume is materialised — not only when the volume is first created.

> r[actuate.volume.storage]
> Named non-tmpfs volumes are stored as host-managed directories under the data directory and bind-mounted into containers. Anonymous non-tmpfs volumes remain managed by the container runtime.

> r[actuate.volume.btrfs]
> When the data directory resides on a BTRFS filesystem, named non-tmpfs volumes must be created as BTRFS subvolumes.

> r[actuate.volume.hold]
> When a named non-tmpfs volume or a managed site volume would be removed — either because the volume name has been removed from the app definition, because the volume's storage backend cannot be updated in-place, or because an operator has requested deletion of a managed site volume — the runtime must preserve the volume's data in a held state instead of deleting it. The held volume remains linked to the originating context: app volumes to their app, and site volumes to the site (reported as app name `_site`). If the app still requires a volume under the same name, a fresh volume is created and the old one is held alongside it. Snapshot site volumes are excluded from the hold mechanism (see [volume.site.snapshot.delete](#r--volume.site.snapshot.delete)).

> r[actuate.volume.hold.confirm]
> An operator must explicitly confirm deletion of a held volume before its data is removed.

> r[actuate.volume.hold.restore]
> An operator may restore a held volume's data into a fresh managed [site volume](#r--volume.site) instead of confirming its deletion. Restoration moves the held data into place under an operator-chosen site volume name, defaulting to the held volume's recorded name; the resulting volume is recorded as a managed site volume regardless of whether the hold originated from an app volume or a site volume. Restoration must be rejected when a site volume with the target name already exists, so operators surface and resolve the collision instead of silently overwriting either record. The held volume's tracking record is removed once its data has been moved.

> r[actuate.volume.hold.events]
> Creation, confirmed deletion, and restoration of held volumes must be observable on the event feed so that UIs can refresh held-volume counts without polling. A restore event must identify the held volume that was consumed and the name of the new site volume it became.

> r[volume.site]
> A site volume is a named volume managed by operators, independent of any app. Site volumes come in two kinds:
>
> - **Managed**: a host directory (or BTRFS subvolume) created and maintained by the runtime under the data directory.
> - **Bind**: an arbitrary host path provided by the operator, mounted as-is.
>
> Site volumes may be read-write or read-only. Tmpfs site volumes are not supported.

> r[volume.site.lifecycle]
> Site volumes are created and deleted exclusively through operator commands. The runtime must create the backing storage for managed site volumes at creation time. When a managed site volume is deleted, its backing storage must be routed through the [held volume](#r--actuate.volume.hold) mechanism so that an operator must explicitly confirm final removal. Deleting a snapshot site volume removes its backing storage directly without routing through the hold mechanism (see [volume.site.snapshot.delete](#r--volume.site.snapshot.delete)). Deleting a bind site volume must only drop the runtime's reference and must not affect the operator-provided host path.

> r[volume.site.lifecycle.events]
> Site volume creation and deletion must emit events on the event feed and be recorded in the [audit log](#r--audit.log). Deletion events must identify the site volume's kind and, when the deletion routed through the held-volume mechanism, the resulting held volume identifier.

> r[volume.site.snapshot]
> A snapshot site volume is a read-only point-in-time snapshot of a named volume (app volume or managed site volume) that supports snapshotting. Only BTRFS-backed volumes support snapshotting. Snapshot site volumes carry metadata identifying their source. They are inherently read-only: even when mapped without the read-only flag, mounts of snapshot site volumes are always read-only.

> r[volume.site.snapshot.events]
> Creating a snapshot site volume must emit an event identifying the new snapshot and the source volume (app-scoped or site-scoped).

> r[volume.site.snapshot.delete]
> Deletion of a snapshot site volume removes the backing storage directly and does not route through the [held volume](#r--actuate.volume.hold) mechanism. Snapshots are read-only by construction (which prevents the rename used by the hold path) and the source volume is unaffected by snapshot deletion, so there is no data the operator could meaningfully recover from a held copy of a snapshot. The corresponding [site volume deletion event](#r--volume.site.lifecycle.events) must therefore record no held volume identifier for the snapshot kind.

> r[volume.site.promote]
> An operator may promote a snapshot site volume to a fresh managed site volume with an operator-chosen name. The runtime must materialise the new volume as a writable copy seeded from the snapshot's contents and record it as a managed site volume. The source snapshot is not modified and remains available; operators may independently delete it afterwards if they wish. Promotion is only supported on BTRFS-backed installations because it relies on the same snapshotting primitive as `volume.site.snapshot`.

> r[volume.site.promote.events]
> Promoting a snapshot site volume must emit an event identifying the new managed site volume and the source snapshot. The promoted volume is treated as a new managed site volume for audit purposes, distinct from the source snapshot which remains intact.

> r[volume.external.mapping.events]
> Creating, removing, or retargeting an operator-configured external volume mapping must emit an event identifying the app and external volume name, the new mapping target (or absence of one for removal), and — for retargeting — the previous target. These events feed both the event feed and the [audit log](#r--audit.log).

> r[service.site]
> A site service is a named service managed by operators, independent of any app. A site service carries a set of one or more endpoints, each a 4-tuple `(service_port, protocol, remote_host, remote_port)`.
>
> `service_port` is the port on which the site service exposes this backend; `remote_host` and `remote_port` are the address traffic is forwarded to. The two ports may differ (e.g. a site service exposes 80/tcp in front of backends listening on 8080). The protocol is one of `tcp`, `udp`, or `http`.
>
> The `(service_port, protocol)` pairs across a site service's endpoints define the ports the service exposes. Traffic destined for `(service, service_port, protocol)` is distributed across the `(remote_host, remote_port)` pairs of all endpoints whose `(service_port, protocol)` matches, as for any other multi-backend service (see [service.routing](language.md#l--service.routing)). Two endpoints sharing a `(service_port, protocol)` are peers for that port's traffic; endpoints differing in `(service_port, protocol)` back different exposed ports on the same site service.

> r[service.site.lifecycle]
> Site services are created, retargeted, and deleted exclusively through operator commands. Creating a site service registers its name and an optional human-readable description; endpoints are added and removed independently so operators may adjust the backing set without recreating the service.
>
> Deleting a site service must be rejected while any [external service mapping](#r--service.external.mapping.events) still targets it; operators must unmap or remap those slots first. Deletion removes the service record and its endpoints; no held-resource mechanism applies because site services carry no persistent state.

> r[service.site.lifecycle.events]
> Site service creation and deletion, and endpoint add/remove operations, must emit events on the event feed and be recorded in the [audit log](#r--audit.log). Endpoint events identify the site service name and the `(service_port, protocol, remote_host, remote_port)` tuple.

> r[service.external.mapping.events]
> Creating, removing, or retargeting an operator-configured external service mapping must emit an event identifying the app and external service name, the new mapping target (or absence of one for removal), and — for retargeting — the previous target. These events feed both the event feed and the [audit log](#r--audit.log).

> r[ingress.site]
> A site ingress is a named entry point managed by operators, independent of any app. A site ingress carries a hostname, an optional human-readable description, a TLS provisioning mode, and a source.
>
> A site ingress has one of two sources:
>
> - **Manual**: created and deleted by the operator.
> - **Discovered**: created and deleted automatically by the daemon based on host configuration. Each discovered site ingress is owned by a *provider* (initially Tailscale; see [ingress.site.tailscale](#r--ingress.site.tailscale)) and carries an opaque, stable provider key that survives hostname changes.
>
> The TLS provisioning mode determines how certificates for the hostname are obtained when an attachment requires TLS termination. Valid modes are public-PKI ACME issuance, internal CA, the discovering provider's own certificate facility (only available on discovered ingresses), and "no TLS" (only plaintext attachments are permitted).

> r[ingress.site.lifecycle]
> Manual site ingresses are created, updated, and deleted exclusively through operator commands. Discovered site ingresses are created, updated, and deleted by the daemon's discovery loop; an operator command to delete a discovered site ingress while its source remains active must be rejected. Deletion of a site ingress, regardless of source, cascades to remove all of its attachments.
>
> When a discovery source temporarily disappears (e.g. the underlying provider becomes unreachable) the corresponding site ingress must be marked stale rather than deleted, so that the operator's attachments persist across transient outages and resume serving when the source returns.

> r[ingress.site.lifecycle.events]
> Site ingress creation, update, and deletion (whether by the operator or by a discovery provider) and attachment add, update, and remove operations must emit events on the event feed and be recorded in the [audit log](#r--audit.log). Events identify the site ingress name and, for attachment events, the `(port, protocol)` tuple and the attachment target.

> r[ingress.site.attachment]
> A site ingress carries zero or more *attachments*. Each attachment is keyed by `(port, protocol)` within the parent site ingress and binds that key to a target. The target is one of:
>
> - **Forward**: traffic for the attachment's `(port, protocol)` is routed to a specified app service.
> - **Redirect**: requests for the attachment's `(port, protocol)` are answered with an HTTP redirect response to a configured target URL, using a configured response code, optionally preserving the request path.
>
> The set of supported protocols matches that of app-declared ingresses. The hostname and TLS provisioning mode are inherited from the parent site ingress; the attachment chooses only the listening `(port, protocol)` and the target. Multiple attachments differing in `(port, protocol)` may coexist on the same site ingress.

> r[ingress.site.tailscale]
> The Tailscale discovery provider creates a single discovered site ingress representing the host's Tailscale identity. The site ingress's hostname is the host's tailnet DNS name; its provider key is the host's stable Tailscale node identity. When the operator renames the node, the site ingress's hostname must be updated in place and its existing attachments preserved. When the underlying node identity changes (e.g. the node is re-created on the tailnet), the existing discovered site ingress is removed and a fresh one created.
>
> The Tailscale-provided site ingress's TLS mode resolves to certificates obtained from the local Tailscale facility. The runtime must not attempt to obtain certificates for tailnet hostnames through public ACME.

> r[ingress.site.conflict]
> When an app ingress and a site-ingress attachment both claim the same `(hostname, port)`, the runtime must drop both entries from the proxy configuration on each tick on which the conflict persists, and must file a fault against each side identifying the conflicting party and the `(hostname, port)` tuple. Faults filed for a `(hostname, port)` conflict must be cleared automatically on the first subsequent tick on which that `(hostname, port)` is no longer in conflict. Independent `(hostname, port)` tuples on the same hostname are not affected by a conflict on a different port.

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

> r[fault.crash-loop]
> When the reconciler observes that a resource instance's backing unit has reached the start-limit-hit terminal state (per [autonomous.restart.start-limit-hit](#r--autonomous.restart.start-limit-hit)), it must file a fault of kind `crash_loop` associated with that instance, distinct from `container_start_failed`.
> The fault is cleared automatically when the instance is subsequently observed healthy. While the fault is active, the reconciler must not auto-restart the affected instance.

> r[fault.external-volume-unmapped]
> When a deployment or job instance requires an external volume that has no mapping in the external volume mapping table, the reconciler must not start the instance and must file a fault of kind `external_volume_not_mapped` against that instance, identifying the missing volume name.
> The fault is cleared automatically when a subsequent reconciliation tick is able to start the instance (i.e. the mapping has been added and the start succeeds).

> r[fault.cert-acquisition]
> When TLS certificate acquisition for an ingress hostname fails persistently (after the proxy's own retry policy has been exhausted), the runtime must file a fault of kind `cert_acquisition_failed` associated with that ingress, identifying the hostname and the most recent acquisition error.
> The fault is cleared automatically when a subsequent acquisition for the same hostname is observed as `valid`.

> r[fault.healthcheck]
> When a container instance has a declared [healthcheck](language.md#l--deployment.healthcheck) and has been continuously unhealthy for longer than its grace window (`start_period` plus `retries × interval`), the runtime must file a fault of kind `health_check_failed` associated with that instance.
> The fault is cleared automatically when the instance is next observed as healthy, or when the instance is unscheduled.
>
> A container without a declared healthcheck, or one still within its grace window, must not cause this fault to be filed.

> r[fault.healthcheck-replace-failed]
> When the replace-loop guard ([autonomous.healthcheck-replace.guard](#r--autonomous.healthcheck-replace.guard)) trips for a Deployment, the runtime must file a fault of kind `health_check_replace_failed` associated with that Deployment, identifying the failed replacement instance and the persistently-unhealthy original.
> The fault is cleared automatically when the AppDef generation changes for the app, or manually by the operator.
> While the fault is active, the runtime must not spawn further replacements for this Deployment.

> r[fault.service-degraded]
> When a Service's routing pool contains only unhealthy backends (i.e. the prefer-healthy rule has fallen back to "anything running" per [lifecycle.service.routing-pool](#r--lifecycle.service.routing-pool)), the runtime must file a fault of kind `service_degraded` associated with that Service.
> The fault is cleared automatically when at least one backend in the pool becomes healthy or when the Service is unscheduled.

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
> The runtime must ensure the NAT64 translator is configured whenever workloads can run
> and removed when they cannot. Specifically: the translator must be set up at daemon
> startup (when NAT64 is required) and on every transition from no-workloads-registered
> back to workloads-registered. It must be torn down on the transition to
> no-workloads-registered. The translator must be operational before any workload
> container is started. The runtime must not remove the translator on daemon shutdown:
> workload containers continue running across daemon restarts and depend on NAT64 to
> reach the IPv4 internet, so the kernel translator state must survive the daemon
> process. If the translator cannot be initialised and NAT64 is required (`enabled` mode
> or `auto` mode with no external NAT64 detected), the runtime must report an error and
> file a fault.

> r[infra.nat64.forwarding]
> When NAT64 is active, the runtime must ensure that IPv6 and IPv4 forwarding are enabled on
> the host, and that a route for `64:ff9b::/96` exists so that container traffic destined
> for the NAT64 prefix reaches the translator.

> r[infra.nat64.dns64]
> When NAT64 is active, the [resolver configuration](#r--infra.resolver.config) must enable
> DNS64 so that lookups for IPv4-only names return synthesised AAAA records under
> `64:ff9b::/96`. When NAT64 is not active, DNS64 synthesis must be disabled in the
> resolver configuration.

> r[infra.nat64.ipv6-egress]
> At startup, the runtime must determine whether the host has working IPv6 egress to the
> internet. The determination must be based on observable host network state (such as the
> presence of a default IPv6 route and at least one globally-scoped IPv6 source address on a
> non-loopback interface) and must not depend on DNS resolution or outbound internet traffic.
> The outcome is recorded once at startup and does not change for the lifetime of the
> process.

> r[infra.nat64.dns64.force-translation]
> When the runtime is providing its own NAT64 translator (i.e. is not relying on external
> NAT64+DNS64 infrastructure) and the host lacks working IPv6 egress (see
> [infra.nat64.ipv6-egress](#r--infra.nat64.ipv6-egress)), the resolver configuration must
> synthesise AAAA records under `64:ff9b::/96` for all IPv4-resolvable names, including
> names that already have a real AAAA record. This ensures workload containers reach
> dual-stack destinations via the NAT64 translator rather than attempting native IPv6 that
> the host cannot route. In all other cases, real AAAA records must be preserved so that
> native IPv6 remains the preferred path.

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

# TLS Certificate Strategy

The runtime obtains and serves TLS certificates for hostnames declared by [TLS-terminating ingresses](language.md#l--ingress.tls).
Operators may, via the operator interface, override how a given hostname's certificate is obtained on a per-hostname basis.
This section defines the available strategies and their contracts.
The BSL surface is intentionally strategy-agnostic: scripts declare only that an ingress wants TLS for a hostname, never how the certificate is acquired.

> r[tls.strategy.default]
> When no operator-defined strategy applies to a hostname declared by a TLS-terminating ingress, the runtime must request a public ACME certificate using the HTTP-01 challenge.
> This is the default and applies to any hostname not bound to a different strategy.
> For hostnames that no public CA can issue for — IP literals, single-label hostnames, and the `.localhost`, `.local`, `.internal` TLDs (the same set excluded from wildcard auto-binding in [tls.policy.wildcard](#r--tls.policy.wildcard)) — the runtime must instead use the proxy's internal CA, producing a self-signed certificate that the proxy serves directly without contacting any external authority.

> r[tls.strategy.acme-dns]
> Operators may bind a hostname or wildcard pattern to a [DNS provider](#r--tls.dns-provider.lifecycle); the runtime must drive ACME for any matching hostname using the DNS-01 challenge against the bound provider.
> The runtime must perform the ACME flow itself rather than configuring the proxy to do so, so that DNS-provider credentials are not exposed to the proxy or its persistent configuration.

> r[tls.policy.wildcard]
> Policies may name a hostname pattern instead of an exact hostname.
> Two pattern shapes must be supported: the catch-all `*`, and a left-anchored shell-glob-style subdomain wildcard `*.<suffix>` matching any hostname that ends in `.<suffix>` (and is not equal to `<suffix>` itself).
> The wildcard covers multiple subdomain levels: `*.example.com` matches both `foo.example.com` and `a.b.example.com`. This is *not* the RFC 6125 DNS-wildcard semantic (which would match exactly one extra label); the runtime is matching operator policy, not a certificate's SAN, so the broader semantic lets a single rule cover an entire zone.
> When several patterns match a hostname, the most-specific match wins: an exact pattern beats any wildcard, a longer `*.<suffix>` beats a shorter one, and `*` is least specific.
> Operators that want to override a sub-zone may add a more-specific pattern (e.g. `*.internal.example.com` or `host.example.com`) alongside.
> Wildcard patterns must not auto-bind hostnames that the proxy itself manages with an internal CA — IP literals, single-label hostnames, and the `.localhost`, `.local`, `.internal` TLDs — because no public CA can issue for them and DNS-01 has nowhere to put the challenge record.
> An exact (non-wildcard) policy still applies to such hostnames so an operator can deliberately override.

> r[tls.policy.auto-default]
> When the operator configures the first DNS provider on a node and no `*` policy already exists, the runtime must automatically create a catch-all `*` policy bound to that provider with the `acme_dns` strategy.
> The auto-policy is operator-mutable: it may be cleared, re-bound, or replaced like any other policy, and it must not be re-created on subsequent provider additions.

> r[tls.settings.contact-email]
> The runtime must persist a single global operator contact email used whenever it registers an ACME account on the operator's behalf.
> The setting must be readable and writable through the operator interface, take effect on the next renewal pass without restart, and default to empty.
> Issuance and renewal that need to register a new ACME account must fail with a clear error when no contact email is configured.

> r[tls.settings.cert-profile]
> The runtime must persist an optional global ACME profile name and forward it on every issuance order via the ACME profiles extension.
> When unset, the runtime must omit the profile field so the CA selects its default profile (typically a ~90-day certificate at Let's Encrypt).
> When set, every subsequent ACME-DNS order — first issuance, renewal, and operator-driven retry — must include that profile name; the CA chooses validity and any other profile-defined attributes accordingly. Let's Encrypt's `shortlived` profile, for example, yields ~6-day certificates, and the runtime's existing renewal threshold (1/3 of remaining lifetime, or RFC 9773 ARI when supplied) handles the resulting shorter renewal cadence without further configuration.
> The setting must be readable and writable through the operator interface and take effect on the next renewal pass without restart.

> r[tls.cert.attempt-log]
> The runtime must record every certificate-issuance attempt — successful or failed — in a durable log, scoped per hostname.
> Each entry must capture the hostname, the trigger (`on_demand`, `manual`, or `renewal`), start and finish timestamps, the outcome (`pending`, `success`, or `failure`), the resulting certificate id (on success), and the error string (on failure).
> The operator interface must expose this log so that an unsuccessful issuance is visible without having to inspect daemon logs.

> r[tls.cert.retry-block]
> Operators may set a per-hostname retry block to pause runtime-driven issuance.
> While a block is set, the issuance coordinator must skip the hostname.
> The block persists across restarts and is removed only by explicit operator action (clear or operator-driven retry).

> r[tls.cert.eager-issuance]
> The runtime — not the proxy — must drive every certificate-issuance attempt for runtime-managed strategies.
> The reconciler must hand each TLS-terminating ingress hostname to the issuance coordinator on every tick.
> The coordinator must dedup in-flight requests, skip hostnames whose policy is not `acme_dns` or that already have an active certificate, debounce after a recent failure, and run the rest in the background.
> The cert-serving endpoint must remain a pure lookup: it must never trigger an issuance flow itself (see [tls.cert.serve](#r--tls.cert.serve)).

> r[tls.cert.force-retry]
> Operator-driven retry must be expressible as a persistent state row keyed by hostname, so the request survives a daemon restart between the operator clicking retry and the reconciler picking it up.
> The issuance coordinator must consume the row atomically at the start of an issuance run, bypassing the recent-failure debounce.
> A successful issuance must remove the row; subsequent operator-driven retries write a fresh one.

> r[tls.acme.account.persist]
> The runtime must persist ACME account state — at minimum the account private key and the URL returned by the directory's newAccount endpoint — for each ACME directory the runtime issues against, encrypted at rest using the [secret key](#r--secret.key).
> All issuance against a given directory must reuse a single persisted account rather than creating a new one per strategy, hostname, or contact-email change.

> r[tls.acme.account.contact-update]
> When the operator changes [tls.settings.contact-email](#r--tls.settings.contact-email) and a persisted account already exists for the directory, the runtime must update the contact information on the existing account (RFC 8555 §7.3.2) rather than registering a new account.
> The persisted contact email must be updated to match only after the directory accepts the change; if the directory rejects the update, the runtime must continue using the existing account, log the failure, and leave the persisted email unchanged so a later attempt can retry.

> r[tls.acme.renewal.auto]
> The runtime must autonomously renew certificates it has issued via ACME (DNS-01 strategy) before they expire.
> Renewal must be attempted while the certificate is still valid, with sufficient lead time that a transient failure does not cause expiry.
> Successful renewal must replace the prior certificate atomically so that in-flight TLS handshakes are unaffected.

> r[tls.cert.ari]
> When the issuing CA advertises ACME Renewal Information (RFC 9773), the runtime should use the CA's suggested renewal window to schedule renewal in preference to its own fixed-fraction-of-lifetime heuristic.
> The runtime must persist the suggested window alongside the certificate (start, end, and the timestamp at which it was last polled) and treat the certificate as due for renewal once the current time has reached the window's start.
> When submitting a renewal order for a certificate whose predecessor's RFC 9773 identifier (authority key identifier plus serial) is available, the runtime must include that identifier in the order's `replaces` field so the CA can correlate the renewal.
> If the CA does not support ARI or the lookup fails, the runtime must fall back to its fixed-fraction-of-lifetime renewal heuristic without surfacing the absence as an error.

> r[tls.strategy.manual]
> Operators may upload a PEM-encoded certificate chain and matching private key.
> The runtime must auto-bind the uploaded cert to every hostname its SubjectAlternativeName list covers — literally for exact entries, and per RFC 6125 single-label rules for wildcard SANs — and cause the proxy to serve that exact pair for TLS handshakes whose SNI matches a covered hostname.
> Auto-binding requires no per-hostname operator action: a `*.example.com` cert covers `foo.example.com` and `bar.example.com` as soon as it is uploaded; further hostnames added later are picked up automatically.
> When more than one stored cert covers the same hostname, the most recently created active row wins.
> The runtime does not auto-renew manual certs on its own; however, if an `acme_dns` policy applies to a covered hostname and the manual cert is past its renewal threshold, the runtime must initiate the normal ACME-DNS issuance flow so a renewable cert can take over before the manual cert expires.

> r[tls.csr.flow]
> Operators may instruct the runtime to generate a keypair and a Certificate Signing Request for a hostname.
> The runtime must:
>
> - Generate the keypair on the server, store the private key encrypted at rest using the [secret key](#r--secret.key), and never expose it via any operator interface.
> - Produce a PEM-encoded CSR whose Subject Alternative Name set covers the target hostname, and make the CSR retrievable via the operator interface for as long as the request is pending.
> - Accept a signed certificate uploaded later, verify it matches the stored private key and satisfies [SAN coverage](#r--tls.cert.validation.san-coverage), and on success transition the hostname's strategy to manual using the uploaded certificate paired with the held private key.
> - Permit cancellation of a pending CSR by the operator, which must destroy the stored private key.

> r[tls.dns-provider.lifecycle]
> The runtime must provide named, separately addressable DNS-provider entries that hold the credentials needed for DNS-01 challenges.
> Provider credentials must be stored encrypted at rest using the [secret key](#r--secret.key).
> Multiple hostnames may reference the same provider entry.
> Deletion of a provider entry must be refused while any hostname's strategy references it.

> r[tls.cert.metadata]
> For every hostname declared by a TLS-terminating ingress, the runtime must surface to the operator interface: the active strategy, the certificate's issuer (when known), `notBefore`, `notAfter`, and the acquisition status as defined in [observe.ingress.certs](#r--observe.ingress.certs).
> Metadata for default-strategy (ACME HTTP-01) certificates is derived from the proxy's certificate cache; metadata for runtime-managed certificates (ACME DNS-01, manual, CSR) is derived from the runtime's own certificate store.

> r[tls.cert.hostname-view]
> The runtime must expose a per-hostname rollup, keyed by the set of TLS-terminating ingress hostnames currently declared by apps the runtime is actively managing — i.e. apps in any phase except `NotInstalled` — combining for each hostname the originating app(s), the resolved policy, the active certificate (if any), the latest issuance attempt's outcome and error, any operator retry block or queued operator retry, and an expected next-issuance time.
> The set of in-scope hostnames must be computed from a single shared enumeration over the app registry; the issuance coordinator (driven by the reconciler), the expiring-cert fault sweep, and the operator interface rollup must all consult the same function so they cannot disagree about which hostnames the runtime owns. A hostname declared only by a `NotInstalled` app must not appear in any of them.
> The rollup must be a *projection* of the same per-hostname decision the issuance coordinator and renewal scheduler use to decide what to act on; the operator interface, the reconciler-driven coordinator, and the renewal task must all consult one decision function over one DB snapshot, so what the operator sees and what the runtime does are structurally identical rather than separately maintained.
> The next-issuance time accordingly follows the ARI suggested window when present (see [tls.cert.ari](#r--tls.cert.ari)), otherwise the same lifetime-fraction fallback the coordinator applies.
> The rollup must support filtering to a single app so the same view can be embedded on per-app surfaces without re-implementing the rollup logic.

> r[tls.cert.serve]
> For runtime-managed certificates (ACME DNS-01, manual, and CSR-derived), the runtime must deliver certificate and key material to the ingress proxy through a mechanism that does not require including private key material in the proxy's persistent configuration or its restart-replay cache.
> The proxy must be able to obtain the appropriate certificate by SNI hostname at TLS handshake time.
> The serving endpoint must be a pure lookup: a stored cert returns 200 with PEM, an unknown hostname returns 204 (no content), and the runtime must never trigger an issuance flow from this path. Issuance is the issuance coordinator's job (see [tls.cert.eager-issuance](#r--tls.cert.eager-issuance)).

> r[tls.cert.validation.san-coverage]
> Whenever the runtime accepts an operator-supplied certificate (manual upload or CSR cert upload), it must validate that the leaf certificate's Subject Alternative Name DNS entries either contain the target hostname literally or contain a wildcard entry that covers it under RFC 6125.
> A wildcard SAN `*.example.com` covers exactly one additional left-most label (it covers `foo.example.com` but not `example.com` and not `a.b.example.com`).
> Uploads that fail this check must be rejected and must not alter any existing policy or certificate.

> r[tls.cert.validation.self-signed]
> The runtime must accept a self-signed leaf certificate (issuer DN equal to subject DN, no chain) on operator upload, but must annotate the stored certificate so that the operator interface can flag it.

> r[tls.cert.validation.expired]
> The runtime must reject an operator-supplied certificate whose `notAfter` is already in the past at upload time, and must accept (with annotation) a certificate whose `notBefore` is in the future.

> r[tls.fault.expiring]
> For certificates that the runtime cannot autonomously renew (manual uploads and CSR-derived certificates), if `notAfter` is within fourteen days, the runtime must file a fault of kind `cert_expiring_soon` against each ingress that uses the affected hostname, identifying the hostname, the strategy, and the expiry timestamp.
> ACME-issued certificates are exempt — the proxy is responsible for autonomous renewal.
> The fault is cleared automatically when a non-expiring certificate replaces the expiring one, or when no ingress references the hostname.

> r[tls.policy.apply]
> Changes to operator-defined TLS policy (binding a hostname to a strategy, changing its parameters, or clearing it) must take effect on a subsequent reconciliation tick without operator-initiated apply steps.
> The runtime must rebuild the proxy configuration accordingly.
