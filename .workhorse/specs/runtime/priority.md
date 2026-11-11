---
id: PRIO
---

# Resource priority

Applies to how the [runtime](../overview.md#runtime) orders workloads on a [host](../overview.md#host) when they compete for memory, CPU and I/O.

## Terminology

### App priority

An [app](../overview.md#app)'s standing against the other apps on its host, set by an [operator](../overview.md#operator).

## Priority setting

Each registered app MUST have an app priority of `high`, `normal` or `low`.
An app's priority MUST default to `normal`.
Only an operator sets an app's priority, [through the Operator Interface](../interface/app-control.md#app-priority); an app's definition does not declare it.

App priority orders apps against each other under contention.
The runtime MUST NOT use it to limit an app's consumption when capacity is free.

The runtime MUST store an app's priority durably.
When an app is uninstalled, the runtime MUST discard its stored priority, so that a later install starts at `normal`.

A change to an app's priority MUST take effect without restarting the runtime and without redeploying the app.
The runtime MUST apply the new division of contended CPU and I/O to the app's running workloads as soon as it re-derives their ordering.
A workload's place in the kill order MAY be fixed when its processes start, in which case the change MUST reach each running workload when it next restarts, and every workload started after the change.

## Unreadable priorities

The runtime MUST NOT treat a stored priority it cannot read as absent, and MUST NOT substitute `normal` for it.
If the runtime cannot read an app's stored priority, it MUST NOT actuate that app in that iteration, so that its running workloads keep running as they are.
If the runtime cannot read the stored priorities at all, it MUST NOT actuate any app in that iteration.
The runtime MUST hold an [`app_priorities_unreadable`](faults.md#host-faults) fault while any stored priority cannot be read.

> [!NOTE]
> A workload started at a standing nobody chose takes a place in the kill order and a share of contended capacity that the operator never set.

## Kill order

When the host runs out of memory, the order in which workloads are shed MUST be app-major.
Every workload of a lower-priority app MUST be preferred as a victim over every workload of a higher-priority app.
Within one app, a workload with a lower [Deployment priority](../language/constants.md#priority) MUST be preferred as a victim over one with a higher Deployment priority.

> [!NOTE]
> A `Critical` Deployment in a `low` app is therefore shed before a `Normal` Deployment in a `high` app.

The runtime's infrastructure containers MUST be shed last, after every app workload.
Where the platform allows it, a `Critical` workload MUST be the kernel's least preferred out-of-memory victim within the order above.
Where the platform allows it, a `Low` workload MUST be the kernel's most preferred out-of-memory victim within the order above.

## Contended capacity

When CPU or I/O is contended, the runtime MUST divide host capacity first between apps in proportion to their app priority, and then within each app between its workloads in proportion to their Deployment priority.
A higher-priority workload MUST receive a larger share of contended capacity than a lower-priority one.
The division MUST NOT throttle a workload when capacity is free.

Hard limits on a single container are set by [`container.memory`](../language/container.md#resource-limits) and [`container.cpus`](../language/container.md#resource-limits).

## Resource-control groups

The runtime MUST place its workloads in resource-control groups it creates and manages itself.
The runtime MUST NOT depend on, or place workloads in, groups the host defines for its own services beyond the platform's standard defaults.

The runtime MUST realise the kill order and the division of contended capacity through those groups and through the kill preference of each supervised workload.

The names of these groups are names the runtime grants itself, and the app names they would collide with are [reserved](identity.md#reserved-names).
