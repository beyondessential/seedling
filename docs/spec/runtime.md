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

> r[audit.log.version-ids]
> App registration and update audit entries must include the version identifier of the new app version. Update entries must also include the previous version identifier.

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
> - **Boot, interrupted operation exists**: replay of the interrupted operation.
> - **Param change**: the `on_change` handler registered on the parameter that changed.
> - **Operator request**: a named action, including `install`.

> r[operation.lifecycle.param-change]
> A param change is a lifecycle operation.
> It is subject to the same [concurrency restrictions](#r--operation.lifecycle.single) as all other lifecycle operations.
> Only one parameter may be changed at a time.

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

# Observation

> r[observe.facts]
> The runtime must collect timestamped observation facts for each resource instance by inspecting the backing system primitives.

> r[observe.deployment]
> For Deployment and Job resource instances, the runtime must observe pod network presence, container lifecycle state (missing, created, running, or exited), and systemd unit state.

> r[observe.volume]
> For Volume resource instances, the runtime must observe whether the named volume exists.

> r[observe.ingress]
> For Ingress resource instances, the runtime must observe whether the proxy is reachable.

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

> r[actuate.volume.start]
> Starting a Volume instance must create the named volume if it does not already exist, then apply any declared file writes to the volume.

> r[actuate.volume.stop]
> Stopping a Volume instance must remove the named volume.

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

# Reconciliation of Resources

> r[reconcile.operation]
> The `rt.reconcile(old, new)` runtime method must convert one resource into another while minimising disruption.

> r[reconcile.supported-pairs]
> The runtime must define which resource type pairs support reconciliation.
> Unsupported pairs must fall back to stop-then-start, as specified in the language spec.

> r[reconcile.ingress]
> Reconciling one Ingress into another must not drop in-flight traffic.
> The runtime must update the ingress configuration atomically, transitioning backends from old to new as new backends become available.

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
> The runtime must run a CoreDNS infrastructure container that provides DNS forwarding and
> caching to all workload containers. The resolver container follows the same lifecycle as
> the proxy container: it is started when workloads are present and torn down when no
> workloads remain.

> r[infra.resolver.config]
> The runtime must generate a CoreDNS configuration (Corefile) that always includes
> forwarding to the host's upstream resolvers and response caching. When
> [NAT64 is active](#r--infra.nat64.mode), the configuration must additionally include the
> `dns64` plugin synthesising AAAA records under the well-known prefix `64:ff9b::/96`.

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
> When NAT64 is active, the runtime must configure a Jool stateful NAT64 translator instance
> using the well-known prefix `64:ff9b::/96`. The translator must be operational before any
> workload containers are started.

> r[infra.nat64.translator.lifecycle]
> The runtime must ensure the Jool instance exists on every startup and must remove it during
> graceful shutdown. If the Jool kernel module is not available and NAT64 is required
> (`enabled` mode or `auto` mode with no external NAT64 detected), the runtime must report
> an error and file a fault.

> r[infra.nat64.forwarding]
> When NAT64 is active, the runtime must ensure that IPv6 and IPv4 forwarding are enabled on
> the host, and that a route for `64:ff9b::/96` exists so that container traffic destined
> for the NAT64 prefix reaches the translator.

> r[infra.nat64.dns64]
> When NAT64 is active, the [resolver configuration](#r--infra.resolver.config) must include
> the `dns64` plugin so that DNS lookups for IPv4-only names return synthesised AAAA records
> under `64:ff9b::/96`. When NAT64 is not active, the `dns64` plugin must be omitted from
> the resolver configuration.
