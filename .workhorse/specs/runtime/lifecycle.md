---
id: LCY
---

# Resource lifecycle

## States

<!-- from runtime: Lifecycle / States -->

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
> Lifecycle states must be derived from the [world observation history](rule:r/history.world), not maintained as an independent state machine.
> The runtime must be able to re-derive the current lifecycle state of any resource from its observation history at any time.

## Resource Lifecycle Semantics

<!-- from runtime: Lifecycle / Resource Lifecycle Semantics -->

> r[lifecycle.container]
> For container resources (Deployments, Jobs):
>
> - **Scheduled**: the container image pull has been initiated and/or the container has been created.
> - **Running**: the container process is executing.
> - **Ready**: the container is running and, if a [healthcheck](rule:l/deployment.healthcheck) is declared, it is currently passing.
> - **Terminated**: the container process has exited (with an exit code).
>
> A container with no declared healthcheck becomes Ready as soon as it is Running. A container with a declared healthcheck remains in Running until the first passing observation is recorded.

> r[lifecycle.container.unhealthy-transition]
> A container observed as unhealthy must transition from Ready back to Running.
> A subsequent healthy observation must transition it forward to Ready again.
> These transitions are derived from the [world observation history](rule:r/history.world) in the same manner as all other lifecycle state derivations and must not require an independent state machine.

> r[lifecycle.service]
> For Service resources:
>
> - **Scheduled**: the internal network plumbing exists.
> - **Ready**: at least one backend is in the routing pool and traffic can be routed.
> - **Terminated**: the network plumbing has been torn down.

> r[lifecycle.service.routing-pool]
> The routing pool for a Service is the set of backend pod instances eligible to receive traffic. The runtime selects the pool from the running backends as follows:
>
> - A backend pod enters the pool only after it has been observed [healthy](rule:l/deployment.healthcheck) at least once. Pods still in start-period or that have never been observed healthy are not in the pool. A pod with no declared healthcheck is treated as healthy as soon as it is Running.
> - Once in the pool, a backend that is observed unhealthy is removed from the pool **only if** at least one other backend in the same Service is currently healthy. If no healthy alternative exists, all running backends remain in the pool (degraded mode) and the runtime files a fault per [fault.service-degraded](rule:r/fault.service-degraded).
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
