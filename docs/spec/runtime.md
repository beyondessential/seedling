The Beset Runtime is the component responsible for making the real world match what a BSL script declares.

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

# Desired State

> r[desired-state.definition]
> The desired state is the set of resources that should exist and their intended [lifecycle states](#r--lifecycle.states).
>
> The desired state is derived from two inputs:
>
> 1. The AppDef: the resource graph produced by evaluating the BSL script.
> 2. The progress of the current [lifecycle operation](#r--operation.lifecycle), if any: which resources have been started, stopped, or reconciled by the action closure so far.

> r[desired-state.steady]
> When no lifecycle operation is active, the desired state is the full AppDef.
> The reconciler maintains it autonomously: restarting terminated containers according to their restart policy, maintaining scale, and keeping networking and ingress configuration consistent.

> r[desired-state.during-operation]
> When a lifecycle operation is in progress, the desired state is built incrementally by the action closure's `rt.start()`, `rt.stop()`, and `rt.reconcile()` calls.
> The reconciler must only act on resources that the operation has placed into the desired state so far.

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

# Persistent History

> r[history.persistence]
> All history records must be stored durably and must survive runtime restarts, including unexpected termination and node power loss.

> r[history.storage]
> The storage mechanism must support transactional writes and efficient queries by resource identity and time range.

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
> - The `rt.*` call that was made (start, stop, reconcile), and on which resources.
> - The barrier condition, if a barrier was reached (which resources, which state, what deadline).
> - Whether the barrier has been satisfied.

> r[history.action-log.replay]
> The action execution log must contain enough information to replay an interrupted lifecycle operation from the beginning and fast-forward to the interruption point.

# Lifecycle Operations

> r[operation.lifecycle]
> A lifecycle operation is the top-level unit of scripted orchestration.
> It is a single execution (possibly interrupted and replayed) of an action closure.

> r[operation.lifecycle.single]
> At most one lifecycle operation may be in progress at any time.
> If a new lifecycle-initiating event arrives while an operation is in progress, it must be rejected.

> r[operation.lifecycle.events]
> Lifecycle operations are initiated by these events:
>
> - **First boot** (no prior state exists): the `install` action, if defined; otherwise the `start` action.
> - **Normal boot** (prior state exists, no interrupted operation): the `start` action.
> - **Restart with interrupted operation**: replay of the interrupted operation.
> - **Version change**: the `upgrade` action.
> - **Operator request**: the named action.

> r[operation.lifecycle.completion]
> When a lifecycle operation completes (the action closure returns), the full [desired state](#r--desired-state.steady) takes effect and the reconciler maintains it autonomously.

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
> 4. `rt.start()`, `rt.stop()`, and `rt.reconcile()` calls that are already recorded in the log are idempotent: they produce the same desired state mutations but do not duplicate real-world operations.
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

> r[autonomous.scale]
> When a Deployment resource's observed running instance count differs from its declared scale, the reconciler must start or stop instances to converge on the declared count.

> r[autonomous.ingress]
> When an Ingress resource's configuration in the ingress controller does not match the desired configuration, the reconciler must update or rebuild the configuration.

> r[autonomous.network]
> When a Service resource's network plumbing is missing or misconfigured, the reconciler must recreate or repair it.

> r[autonomous.provenance-required]
> Every autonomous operation must record [provenance](#r--history.operations.provenance) before execution.

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
> - The application name.
> - The resource type.
> - The resource name (if not anonymous).
> - The instance ordinal (for scaled resources).

> r[identity.scaled]
> Scaled resources (e.g. a Deployment with `scale(N)`) produce N instances.
> Each instance must have a distinct, stable ordinal so the runtime can track instances independently across restarts and scaling events.

> r[identity.anonymous]
> Anonymous resources (those without a name) must receive a runtime-assigned identity that is stable for the lifetime of the resource but does not conflict with named resources.

# Reconciliation of Resources

> r[reconcile.operation]
> The `rt.reconcile(old, new)` runtime method must convert one resource into another while minimising disruption.

> r[reconcile.supported-pairs]
> The runtime must define which resource type pairs support reconciliation.
> Unsupported pairs must fall back to stop-then-start, as specified in the language spec.

> r[reconcile.ingress]
> Reconciling one Ingress into another must not drop in-flight traffic.
> The runtime must update the ingress configuration atomically, transitioning backends from old to new as new backends become available.