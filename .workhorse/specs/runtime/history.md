---
id: HIST
---

# Runtime history

Applies to the records the [runtime](../overview.md#runtime) keeps of what it has observed on a [host](../overview.md#host) and what it has done there.

## Terminology

### World observation history

The timeline of observations the runtime has recorded about each resource instance.

### Autonomous operations log

The record of the operations the runtime has performed on a host without direction from an action closure.

### Action execution log

The record, in order, of the runtime calls each [lifecycle operation](../overview.md#lifecycle-operation)'s action closure has made.

### Operation provenance

The observations and the rule that caused the runtime to perform an autonomous operation.

## Durability

The runtime MUST store the world observation history, the autonomous operations log and the action execution log durably.
Every record MUST survive a restart of the runtime, including one after unexpected termination or loss of power to the host.
A write that changes several records together MUST be atomic.

A writer that updates a stored record that another writer also updates MUST change only the fields it owns.
It MUST leave every other field as it was, including fields added to the record after the writer was written.

## Observations

The runtime MUST record each observation in the world observation history against the [resource instance](identity.md) it concerns.

| member | meaning |
| --- | --- |
| time | when the runtime recorded the observation |
| instance | the identity of the resource instance observed: its resource type, its name, and its instance identifier |
| kind | the kind of observation, such as a container's status, a process exit, a healthcheck result or network reachability |
| payload | the observed value |

Every observation MUST correspond to a check the runtime made, an event it received, or an action it took on the host.
The runtime MUST NOT record an observation it has not made.

Observations recorded in one iteration of the [reconciliation loop](reconciliation.md#reconciliation-loop) MAY share a time.
The runtime MUST order an instance's observations by time, and observations with equal times in the order it recorded them.

> [!NOTE]
> [Lifecycle derivation](lifecycle.md#derivation) is sensitive to the order of observations, so an order left to storage would make it non-deterministic for an instance with several observations in one iteration.

## Autonomous operations

The runtime MUST record each operation it performs without direction from an action closure in the autonomous operations log.

| member | meaning |
| --- | --- |
| time | when the runtime recorded the operation |
| instance | the resource instance the operation affected |
| operation | the operation performed, such as starting a container or rebuilding ingress configuration |
| provenance | its operation provenance |
| outcome | success, failure with details, or interrupted; absent while the operation is in progress |

An entry's operation provenance MUST identify the observations that triggered the operation and the rule that applied.

> [!NOTE]
> Examples of operation provenance: "Container exited (observation at T), OnTerminate policy is Recreate."; "Observed 1 running instance, scale requires 2."; "Ingress controller unreachable (observation at T), rebuilding configuration."

When the runtime starts, it MUST complete every entry still in progress with the outcome interrupted, before it performs any autonomous operation.
The runtime MUST NOT record an interrupted operation as a success or as a failure, since its real outcome is unknown.

The runtime MUST detect repeated operations of one kind on one resource instance from the autonomous operations log, and MUST back off between them.
The delay between repetitions MUST grow with each repetition, up to a bounded maximum.
The back-off MUST prevent a tight loop, such as restarting at full speed a container that exits immediately after starting.

## Action calls

The runtime MUST record in the action execution log each runtime call a lifecycle operation's action closure makes, in the order the closure makes them.

| member | meaning |
| --- | --- |
| time | when the runtime recorded the call |
| operation | the identity of the lifecycle operation |
| position | the call's position in the closure's sequence of calls |
| call | the `rt` method called, such as `start`, `stop` or `warm_certs`, and the resources it named |
| barriers | each [barrier](suspension.md#barriers) the call reached: its resources, the lifecycle state it requires, and its deadline |
| satisfied | for each barrier, whether it has been satisfied |

Before [`Action.invoke()`](../language/action.md#action-invocation) runs the called closure, the runtime MUST record an entry for the invocation holding the called action's name and its validated parameter map.
Replay of that entry is specified by [composition replay](operations.md#sub-action-parameters).

The action execution log MUST hold enough to [replay](suspension.md#replay) an interrupted lifecycle operation from its beginning and fast-forward to the point of interruption.
