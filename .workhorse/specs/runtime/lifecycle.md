---
id: LCY
---

# Resource lifecycle

Applies to the [lifecycle states](../overview.md#lifecycle-state) of [resource](../overview.md#resource) instances and how the [runtime](../overview.md#runtime) derives them.

## Lifecycle states

A resource instance's lifecycle state is one of the following.

| state | meaning |
| --- | --- |
| `Pending` | The instance is in the [desired state](../overview.md#desired-state), and nothing has yet been done to realise it. |
| `Scheduled` | The instance's underlying primitives exist, and it is not yet operational. |
| `Running` | The instance is operational, and has not yet been confirmed ready. |
| `Ready` | The instance is operational, and meets the readiness criteria that apply to its kind. |
| `Terminating` | The runtime has initiated the instance's termination, and the instance has not yet stopped. |
| `Terminated` | The instance has stopped. |
| `Unscheduled` | The instance's underlying primitives have been cleaned up. |

Lifecycle states apply to instances of deployments, jobs, services, ingresses, volumes and external volumes.
Each such instance MUST be in exactly one lifecycle state at any time.

## Transitions

An instance's lifecycle state MUST advance only in the order `Pending`, `Scheduled`, `Running`, `Ready`, `Terminating`, `Terminated`, `Unscheduled`, except as this section permits.

An instance MAY skip states.
For example, a container that exits by itself moves from `Running` or `Ready` directly to `Terminated`, without passing through `Terminating`.

A container MAY return from `Ready` to `Running`, as [containers](#containers) specifies.

An instance in `Terminated` or `Unscheduled` that is observed being set up or running again MUST begin a new cycle from `Pending`.

## Derivation

The runtime MUST derive each instance's lifecycle state, and the time of its last transition, from the instance's [world observation history](history.md#world-observation-history) alone.
The runtime MUST be able to re-derive any instance's current lifecycle state from that history at any time.
The runtime MUST NOT keep a lifecycle state that cannot be re-derived from the history.

A failure to read an instance's observation history MUST NOT be treated as an empty history.

## Containers

The lifecycle states of a deployment or job instance have these meanings.

| state | meaning |
| --- | --- |
| `Scheduled` | The container's image pull has started, or the container has been created. |
| `Running` | The container's process is executing. |
| `Ready` | The container is running and, if it declares a [healthcheck](rule:l/deployment.healthcheck), the healthcheck is passing. |
| `Terminated` | The container's process has exited, with an exit code. |
| `Unscheduled` | The container has been removed. |

A container with no declared healthcheck MUST become `Ready` as soon as it is `Running`.
A container with a declared healthcheck MUST stay `Running` until a passing healthcheck is observed.

A `Ready` container observed unhealthy MUST return to `Running`.
A later healthy observation MUST return it to `Ready`.

## Services

The lifecycle states of a service instance have these meanings.

| state | meaning |
| --- | --- |
| `Scheduled` | The service's network plumbing exists. |
| `Ready` | At least one backend is in the service's [routing pool](#routing-pool), and traffic can be routed. |
| `Terminated` | The service's network plumbing has been torn down. |
| `Unscheduled` | The service's network plumbing has been cleaned up. |

A service whose routing pool becomes empty MUST leave `Ready`.

## Routing pool

A service's routing pool for one of its ports is the set of running backends that receive that port's traffic.

The runtime MUST compose each pool from the backends running at the time, as follows.
If at least one running backend is currently observed healthy, the pool MUST hold exactly the running backends currently observed healthy.
Otherwise, the pool MUST hold every running backend, and the runtime MUST file the [service degraded fault](rule:r/fault.service-degraded).

A backend with no declared healthcheck MUST count as healthy while it is running.
A backend whose declared healthcheck has not yet passed, such as one in its start period, MUST count as not healthy.

> [!NOTE]
> The pool prefers healthy backends and falls back to any running backend, so the runtime never serves less capacity than is running.

## Ingresses

The lifecycle states of an ingress instance have these meanings.

| state | meaning |
| --- | --- |
| `Scheduled` | The ingress's configuration has been submitted to the proxy. |
| `Ready` | Proxy configuration including the ingress has been applied. |
| `Terminated` | The ingress's configuration has been removed. |
| `Unscheduled` | The ingress's configuration has been cleaned up. |

## Volumes

The lifecycle states of a volume instance have these meanings.

| state | meaning |
| --- | --- |
| `Scheduled` | The volume exists on the host, and any declared writes have been applied. |
| `Ready` | The volume is available for mounting. |
| `Terminated` | The volume has been removed. |
| `Unscheduled` | The volume has been cleaned up. |

## External volumes

The runtime manages an external volume's lifecycle state entirely itself.
An external volume MUST be `Ready` as soon as the desired state holds it `Ready`, and `Unscheduled` as soon as the desired state holds it `Unscheduled`.
An external volume's lifecycle state MUST NOT depend on whether a mapping exists for it.

> [!NOTE]
> A missing mapping is reported by the [external volume unmapped fault](rule:r/fault.external-volume-unmapped).
