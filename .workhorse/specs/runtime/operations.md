---
id: OPS
---

# Lifecycle operations

Applies to the [lifecycle operations](../overview.md#lifecycle-operation) through which an [app](../overview.md#app)'s action closures direct the [runtime](../overview.md#runtime), and to the shell sessions that run beside them.

## Terminology

### Operation generation

The [generation](../overview.md#generation) whose definition and parameter values an operation runs against.

### Operation-scoped volume binding

A volume name the runtime binds to a host path for the duration of one operation, so that the operation's action closure can resolve it.

## Operation

A lifecycle operation is one run of one action closure, or one [gate run](gates.md), together with every [replay](suspension.md#replay) of it after an interruption.
An operation ends when its action closure returns, throws, or is [cancelled](#cancellation), and a gate run ends as [its outcome](gates.md#outcome) specifies.

## Concurrency

The runtime MUST NOT have more than one lifecycle operation in progress on a host at any time.

If an operation is requested for an app that already has one in progress, the runtime MUST reject the request at once.

If an operation is requested for an app while another app's operation is in progress, the runtime MUST queue the request.
The runtime MUST NOT hold more than one queued operation for an app, and MUST reject a request for an app that already has one queued.
The runtime MUST start queued operations in the order they were requested, each once the operation in progress ends.

The runtime MUST persist a queued request durably, with its app, its action or gate, its trigger, its operation generation, and its parameters, before it accepts the request.
The runtime MUST protect a queued request's persisted parameters as it protects a persisted operation's, as [persistence](#persistence) specifies.
If the runtime cannot persist the request, it MUST reject it.
When the runtime starts, it MUST restore its queue from the persisted requests, in the order they were requested, after deciding whether to replay an interrupted operation.
The runtime MUST discard a restored request whose app is no longer registered.
The runtime MUST clear a persisted request when the operation it describes starts or is rejected.

Admission for an app that is being uninstalled is governed by [app teardown](app-teardown.md#uninstall-admission).

## Initiating events

| event | operation | trigger | operation generation |
| --- | --- | --- | --- |
| the runtime starts with an interrupted operation | the interrupted operation, [replayed](#interrupted-operations) | `replay` | as persisted |
| a [gate](../language/gate.md) is due to run, as [initiating runs](gates.md#initiating-runs) specifies | the gate's run | `gate` | current at dispatch |
| an operator request | the named action, or the named gate's run | `operator` | current at dispatch |
| a schedule falls due | the [scheduled action](scheduled-actions.md) | `schedule` | current at dispatch |

The runtime MUST record the operation generation of every operation as the generation current when the operation is dispatched, whether it starts at once or leaves the queue.

The runtime MUST run an operation against the definition and parameter values of its operation generation, and MUST pass a gate's body [`last`](../language/gate.md#body) reconstructed from the generation of the gate's last success.
The runtime MUST do so on every replay, including where later generations have been committed since the operation was dispatched.
If the runtime cannot reconstruct a generation the operation needs, it MUST fail the operation.
The runtime MUST NOT run the operation against another generation's state.

## Changes to an app

A change to an app's definition or parameter values initiates no operation.
A change that moves the key of one of the app's gates leaves the gate unsatisfied, and the runtime runs it as [gate runs](gates.md) specify.

The runtime MUST apply changes to an app one at a time, and MUST refuse a change to an app while another change to it is being applied.
An operation in progress for the app goes on against its operation generation.

## Completion

When an operation ends, the runtime MUST return the app to its steady-state [desired state](reconciliation.md#desired-state) at the app's current generation, and maintain it without direction.

When an operation other than a gate run fails, the runtime MUST file an [`operation_failed`](faults.md#app-faults) fault.
A gate run that fails or is cancelled files a `gate_failed` fault instead, as [retries](gates.md#retries) specify.

The runtime MUST keep tearing down an ended operation's [dynamic resources](../language/app.md#dynamic-resource-lifetime), retrying with a growing delay, until every one of them is gone.
It MUST forget a dynamic resource's record only once that resource is torn down.
While they are not all gone a minute after the operation ends, or while their records cannot be read, the runtime MUST hold an [`operation_cleanup_stalled`](faults.md#app-faults) fault.

## Persistence

Before an operation first does anything, the runtime MUST persist durably the operation's identifier, app, action or gate, operation generation, parameters, and [operation seed](secrets.md#operation-seeds).
If the runtime cannot persist them, it MUST NOT start the operation.

The runtime MUST encrypt the persisted parameters and seed with the cipher it uses for [stored secret parameters](secrets.md#storage).
The persisted parameters MUST be the [param map](../language/action.md#param-map) the action closure receives, holding parser results as [param delivery](../language/action.md#param-delivery) specifies.
On replay, the runtime MUST pass the action closure the persisted parameters, without validating or parsing them again.
The runtime MUST restore each `Version` or `SemVer` in the persisted parameters as a value of the same type with the same [text](../language/version.md#text-of-a-version).
The runtime MUST restore each `Regex` in the persisted parameters as a `Regex` of the same [pattern](../language/regex.md#compiling-a-pattern), without counting it toward any limit.

> [!NOTE]
> Restoring a version or a pattern depends on its text alone, so it cannot reject a value that was accepted when the operation began.

The runtime MUST clear the persisted operation when the operation ends.

## Interrupted operations

When the runtime starts, it MUST read the persisted operation, if any, and decide whether to replay it before the reconciliation loop first starts an operation.

The runtime MUST replay an interrupted operation only if the app is registered, the persisted parameters can be read, and the app is `Installed`.

Where the runtime does not replay an interrupted operation, it MUST clear the persisted operation and tear down the resources it [spared for the replay](suspension.md#abandoned-replay).

If the runtime cannot read whether an interrupted operation exists, it MUST NOT perform [leftover resource recovery](#leftover-resource-recovery) or [startup cleanup](suspension.md#abandoned-replay).

> [!NOTE]
> Both act on the interrupted operation's absence.

### Unreadable operation state

If the runtime cannot read the persisted operation, whether a cancel was persisted for it, or the persisted queue, it MUST file an [`operation_state_unreadable`](faults.md#host-faults) fault.
Until the read succeeds, the runtime MUST NOT start any lifecycle operation, and MUST retry the read on each reconciliation iteration.
Once the read succeeds, the runtime MUST decide replay and restore its queue as on starting.

## Leftover resource recovery

When it starts, after deciding whether to replay, the runtime MUST move to `Uninstalling` every app that is `NotInstalled` while resource instances are still recorded for it.

> [!NOTE]
> Reconciliation leaves the resources of a `NotInstalled` app alone.
> Moving the app to `Uninstalling` tears those resources down, and the app reaches `NotInstalled` once teardown completes.

## Cancellation

The runtime MUST let an operator [cancel](../interface/actions.md#cancellation) the operation in progress for an app.

When a cancel is requested, the runtime MUST wake the operation if it is suspended on a [barrier](suspension.md#barriers), including a barrier with no deadline, so the cancel takes effect without waiting for the barrier's next re-evaluation or deadline.
The runtime MUST stop a cancelled operation at the next barrier or `rt.*` call it reaches, or at once in the parts of a gate run that [gate cancellation](gates.md#cancellation) names.

The runtime MUST end a cancelled operation in an outcome distinct from success and from failure.
The runtime MUST clean up after a cancelled operation as after a failed one, tearing down its [dynamic resources](../language/app.md#dynamic-resource-lifetime) and clearing its persisted record.
For an operation other than a gate run, the runtime MUST file an [`operation_cancelled`](faults.md#app-faults) fault, not an `operation_failed` one.

### Cancel persistence

The runtime MUST persist an accepted cancel request durably against the identifier of the operation it cancels.
When the runtime replays an operation whose cancel was persisted, it MUST begin the replay cancelled, so that the operation ends at its first barrier or `rt.*` call without repeating work.
If the runtime cannot read whether a cancel was persisted for the operation it is about to replay, it MUST NOT replay the operation, and its state is [unreadable](#unreadable-operation-state).

The runtime MUST apply a persisted cancel only to the operation it was requested for.
The runtime MUST NOT let a later operation inherit it, whether that operation was dequeued after it or started afresh.

## Operation-scoped volumes

The runtime hands a volume to an action closure, such as the site volume an operator names for a `volume` action parameter, through an operation-scoped volume binding.

For each volume it hands over, the runtime MUST generate a name from the [reserved](identity.md#reserved-names) prefix `seedling-op-`, bind it to the host path and access mode for the duration of the operation, and pass the name to the closure as the parameter `<key>_volume`, where `<key>` is the binding's logical key.
The runtime MUST generate a fresh name for each binding, and MUST NOT generate a name that another operation-scoped volume binding uses.
The runtime MUST remove the binding when the operation ends.

The `<key>_volume` parameter holding a binding's name MUST be among the operation's [persisted parameters](#persistence).
On replay, the runtime MUST re-establish each binding under the name its persisted parameters hold.
A script resolves the volume with `app.external_volume(param["<key>_volume"])`.

The runtime MUST NOT let a script choose the generated name, and the name MUST NOT appear among the parameters the operation was requested with.

> [!NOTE]
> A script reads the generated name from its parameter and never constructs it, so the form of the name past its prefix is not part of the contract.

Where the runtime needs a closure to use a particular file within a bound volume, it MAY pass the file name as the parameter `<key>_filename`.
A script handed `<key>_filename` MUST use that file name.

## Action composition

A script MAY run another action from within an action closure by calling [`Action.invoke(params?)`](../language/action.md#action-invocation) on a handle from [`app.action(name)`](../language/action.md#action-handles) or `app.on_action()`.

The runtime MUST run the invoked action's closure inline, within the calling operation, sharing its `rt`, its operation identifier and its action log.
A barrier in the invoked closure is a barrier of the calling operation.

### Cycles

An `Action.invoke` call whose action is already on the operation's call stack throws, as [action invocation](../language/action.md#action-invocation) specifies.
The call stack begins with the operation's own action.
The runtime MUST identify the actions on the stack by name.

### Sub-action parameters

The supplied parameters are validated before the invoked closure runs, as [action invocation](../language/action.md#action-invocation) specifies.

On replay, the runtime MUST run a call already recorded in the [action log](history.md#action-calls) with the parameters its entry holds, restored as [persistence](#persistence) specifies, and MUST NOT validate them again nor check the invoked action's [breaker](engine-limits.md#breakers).
If the entry at the call's position records a different action than the one being invoked, the runtime MUST fail the replay as a [divergence](suspension.md#positional-replay).

> [!NOTE]
> Validation consults the world as it stands: the site volumes that exist and the schema the current definition declares.
> Validating a recorded call again would stop an operation replaying because an operator deleted the volume it named or an update changed the schema after the call was committed.

## Shell sessions

A shell session is not a lifecycle operation.
The runtime MUST allow a shell session to run beside a lifecycle operation and beside other shell sessions.

Resources a shell session creates are [dynamic](../language/app.md#dynamic-resource-lifetime), and the reconciler actuates those the session's closure starts as [desired state](reconciliation.md#desired-state) specifies.
Before it places a session's dynamic resource in the desired state, the runtime MUST record it durably against the session, and MUST NOT start a resource whose record it could not write.

When a session ends, the runtime MUST tear down its dynamic resources as it does an ended operation's, under [completion](#completion), holding [`operation_cleanup_stalled`](faults.md#app-faults) for the session where they are not all gone within a minute of its end or their records cannot be read.
A session's dynamic resources left behind by a previous run of the runtime are torn down as [abandoned replay](suspension.md#abandoned-replay) specifies.
The runtime MUST NOT hold a session open, nor delay its [exit frame](../interface/shells.md#session-end), while its dynamic resources are torn down.
The runtime MUST NOT persist a shell session's parameters, and MUST NOT replay a session after a restart.
