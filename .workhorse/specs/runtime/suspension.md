---
id: SUSP
---

# Suspension and replay

Applies to how the [runtime](../overview.md#runtime) suspends a [lifecycle operation](../overview.md#lifecycle-operation)'s action closure on a barrier, and replays it from the [action log](rule:r/history.action-log) to resume it or to recover it after a restart.

## Terminology

### Barrier

A call in an action closure that waits for resource instances to reach a [lifecycle state](../overview.md#lifecycle-state): a [state method](rule:l/rt.started.state-methods) on a `Started`, or [`rt.stop`](#rtstop).

### Pass

One run of an action closure from its beginning, which ends when the closure suspends on a barrier, returns or throws.

### Replay

A pass over a closure whose earlier passes have committed entries to the action log, during which calls already recorded are matched to their entries rather than performed again.

### Anchoring entry

The action log entry of the call that produced the `Started` a barrier is called on.

## Barriers

When an action closure reaches a barrier whose condition is not satisfied, the runtime MUST suspend the closure.
To the script, the barrier call blocks.

A barrier's condition names resource instances and a required lifecycle state.
The condition is satisfied when every named instance has reached or passed the required state, as derived from the [world observation history](rule:r/history.world.state-derivation).

When a suspended barrier's condition is satisfied, the runtime MUST resume the closure, which continues from the point immediately after the barrier call.

### Deadlines

A barrier MAY have a deadline, measured from when the barrier began to wait.
If a barrier's condition is not satisfied when its deadline passes, the runtime MUST throw from the barrier call within the closure.
A barrier without a deadline, as for `.ready_eventually()` and `.terminated_eventually()`, MUST wait until its condition is satisfied or the operation is [cancelled](operations.md#cancellation).

The runtime MUST evaluate a barrier's condition before its deadline.
A barrier whose condition is satisfied MUST succeed even if its deadline has passed.

> [!NOTE]
> The action log can lag the world.
> A short-lived resource can complete during a pass whose record of the barrier's satisfaction never reaches the log, and checking the deadline first would time out a barrier whose resources have reached the required state.

### Re-evaluation cadence

The runtime MUST re-evaluate a suspended barrier's condition repeatedly until it is satisfied, its deadline passes, or the operation is cancelled.
The interval between re-evaluations SHOULD follow the longest time any of the operation's barriers has been recorded as waiting:

| wait | interval |
| --- | --- |
| up to 2 minutes | 2 seconds |
| 2 minutes to 1 hour | rising linearly from 2 to 30 seconds |
| 1 hour to 6 hours | rising linearly from 30 to 300 seconds |
| over 6 hours | 300 seconds |

> [!NOTE]
> A short interval keeps quick barriers responsive, and a long one bounds the cost of re-running a closure throughout an operation that waits for hours.

## Replay

If the runtime restarts while an operation is in progress and decides to [replay it](operations.md#interrupted-operations), it MUST:

1. Evaluate the app's definition to reconstruct the operation's [resource graph](../overview.md#resource-graph).
2. Read the operation's action log from durable storage.
3. Run the action closure from its beginning.
4. Match each `rt.*` call already recorded in the log to its entry, applying the same desired-state changes without repeating the call's effect on the host.
5. Return at once from each barrier whose condition is already satisfied.
6. Suspend at the first barrier whose condition is not satisfied, as on any other pass.

The runtime MAY resume a suspended operation by a replay at any time, and not only after a restart.

A script's action closures MUST NOT have side effects other than through `rt.*` calls.
Given the same resource graph and parameters, a closure MUST make the same sequence of `rt.*` calls up to any barrier.

### Positional replay

The runtime MUST match a call made during a replay to the committed entry at the call's own position in the closure's sequence of calls, and never to an entry elsewhere in the log.
Two identical calls at different points in a closure are two calls, and a call whose resolved arguments differ between passes is still the same call.

The runtime MUST fail the replay if the entry at a call's position records a different kind of call, or records an argument that is a literal in the script and differs from the call's.
The runtime MUST NOT compare arguments that may legitimately resolve differently between passes, such as the set of instances a call addresses.

### Barrier anchoring

The runtime MUST decide whether a barrier has already been satisfied from its anchoring entry alone, and never from another entry that records the same resources and required state.

An entry MUST hold a record for each barrier asserted on its call, identified by the barrier's position among them in the order the closure asserts them.
The runtime MUST NOT identify a barrier by its resources or its required state.
A barrier on a `Started` that a [Collection](rule:l/collection.interface) method has narrowed is a barrier on the original call, with its own record on that call's entry.

> [!NOTE]
> A replica can be added or retired between passes and the barrier is still the same one, and a closure can assert the same state twice on one call.
> Distinct calls can resolve to the same resource set, and `rt.stop` records the same terminal state a later `.terminated()` waits for.

Where its anchoring entry records no satisfied barrier at its position, the runtime MUST decide the barrier against the world observation history.

The runtime MUST record that a barrier's condition was met against the barrier's own record, whether the condition was met on the pass that first reached the barrier or on a later one.
The runtime MUST record when each barrier began to wait, and MUST keep that time until the barrier is satisfied.

> [!NOTE]
> Recording the start again on each pass would restart the wait, so the deadline could never pass.
> A barrier that passed without being recorded as passed goes on counting as waiting, and slows the re-evaluation of every barrier after it.

## rt.stop

[`rt.stop`](rule:l/rt.stop) is a barrier: the runtime MUST suspend the closure until every resource it names has terminated, under the same deadline, recording and replay rules as any other barrier.

## Side-effecting calls

[`rt.signal`](rule:l/rt.signal), [`rt.write`](rule:l/rt.write) and [`rt.exec`](rule:l/rt.exec) act on the host directly, and the runtime MUST record each call in the action log.

| call | entry records | entry omits |
| --- | --- | --- |
| `rt.signal` | the instances signalled and the signal's name | |
| `rt.write` | the target volume and the path written | the contents written |
| `rt.exec` | the target instance and the command's exit code | the command, its arguments and its options |

The runtime MUST commit each such entry no later than the end of the pass that made the call.
Once an entry is committed, a replay MUST NOT repeat the call's effect.
A replayed `rt.exec` MUST return the exit code its entry records.

> [!NOTE]
> A host that stops after a call's effect and before the end of the pass replays the call and repeats its effect.

The runtime MUST run an `rt.exec` command inside the target's running container, with the container's namespaces, working directory, user and environment, and MUST block the closure until the command exits.

The runtime MUST resolve an `rt.write` target to the volume's storage on the host as for a static `Volume.write`, and MUST write under the [confinement](rule:r/storage.confinement) that holds every path the runtime owns.

## Abandoned replay

When it starts, the runtime MUST tear down the [dynamic resources](rule:l/app.resources.dynamic) that previous runs recorded, except those recorded against the operation it is about to replay, which the replay adopts.
Where the replay does not then start, for any reason, the runtime MUST tear down the resources it spared and forget their records before it goes on serving.

The runtime MUST forget only the records of resources it has torn down.
A record it could not read, or whose resource it could not tear down or could not confirm was gone, MUST stay recorded.

> [!NOTE]
> A resource spared for a replay that never happens is recorded against an operation nothing will claim, and the next start spares it again on the same reasoning.
> Dropping a record whose teardown did not happen leaves a resource running with no record, no fault and nothing to reclaim it.
