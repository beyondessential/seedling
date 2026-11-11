---
id: OPS
---

# Lifecycle operations

Applies to the [lifecycle operations](../overview.md#lifecycle-operation) through which an [app](../overview.md#app)'s action closures direct the [runtime](../overview.md#runtime), and to the shell sessions that run beside them.

## Terminology

### Source generation

The [generation](../overview.md#generation) current immediately before the change that initiated an operation.

### Target generation

The generation whose definition and parameter values an operation runs against.

### Operation-scoped volume binding

A volume name the runtime binds to a host path for the duration of one operation, so that the operation's action closure can resolve it.

## Operation

A lifecycle operation is one run of one action closure, together with every [replay](suspension.md#replay) of that closure after an interruption.
An operation ends when its action closure returns, throws, or is [cancelled](#cancellation).

## Concurrency

The runtime MUST NOT have more than one lifecycle operation in progress on a host at any time.

If an operation is requested for an app that already has one in progress, the runtime MUST reject the request at once.

If an operation is requested for an app while another app's operation is in progress, the runtime MUST queue the request.
The runtime MUST NOT hold more than one queued operation for an app, and MUST reject a request for an app that already has one queued.
The runtime MUST start queued operations in the order they were requested, each once the operation in progress ends.

The runtime MUST persist a queued request durably, with its app, action, trigger, source and target generations, and parameters, before it accepts the request.
If the runtime cannot persist the request, it MUST reject it.
When the runtime starts, it MUST restore its queue from the persisted requests, in the order they were requested, after deciding whether to replay an interrupted operation.
The runtime MUST discard a restored request whose app is no longer registered.
The runtime MUST clear a persisted request when the operation it describes starts or is rejected.

Admission for an app that is being uninstalled is governed by [app teardown](app-teardown.md#uninstall-admission).

## Initiating events

| event | operation | trigger | source generation | target generation |
| --- | --- | --- | --- | --- |
| the runtime starts | the [start action](../language/action.md#start-action), for each installed app with no interrupted operation | `boot` | current | current |
| an install operation completes successfully | the start action | `install` | current | current |
| the runtime starts with an interrupted operation | the interrupted operation, [replayed](#interrupted-operations) | `replay` | as persisted | as persisted |
| a parameter change | the changed parameter's `on_change` handler | `param_change` | before the change | produced by the change |
| an operator request | the named action, or `install` | `operator` | current | current |
| a schedule falls due | the [scheduled action](scheduled-actions.md) | `schedule` | current | current |

Until an app's start action ends, its [desired state](reconciliation.md#desired-state) holds only what the start action has started or stopped.

> [!NOTE]
> Holding the whole resource graph back until the start action has run lets the start action bring an app up in its own order after the host restarts.

The runtime MUST initiate an operation for a parameter change only when the app is installed, a handler is registered for the parameter, and the change matches one of the handler's [transitions](../language/parameter.md#change-handlers).
A parameter changed alongside a definition update is a parameter change.

The runtime MUST record the source and target generations of every operation.
For an operation that a [generation bump](generation.md#generation-numbers) initiated, the runtime MUST record as the source generation the generation current immediately before the bump, and as the target generation the generation the bump produced.
For any other operation, the runtime MUST record the generation current when the operation is dispatched as both.

The runtime MUST run an operation against the definition and parameter values of its target generation, and MUST pass an `on_change` handler `old` reconstructed from the source generation.
The runtime MUST do so on every replay, including where later generations have been committed since the operation was dispatched.
If the runtime cannot reconstruct a generation the operation needs, it MUST fail the operation rather than run it against another state.

## Parameter changes

The runtime MUST refuse a parameter change for an app that has an operation in progress or queued, or another change being applied.
The runtime MUST accept a change only if it sets or unsets exactly one parameter, including a change made alongside a definition update.

The runtime MUST make checking that nothing holds the app and admitting the change's handler one decision with respect to admitting any other operation for the app.
The runtime MUST NOT admit another operation for the app after the change is committed and before its handler is admitted.

## Completion

When an operation ends, the runtime MUST return the app's [desired state](reconciliation.md#desired-state) to the whole resource graph at the app's current generation, and maintain it without direction.

Where a generation bump initiated the operation, the runtime MUST record the operation's outcome on that generation's [history entry](generation.md#history-entries).

When an operation fails, the runtime MUST file an [`operation_failed`](faults.md#app-faults) fault.

## Persistence

Before an operation's action closure first runs, the runtime MUST persist durably the operation's identifier, app, action, source and target generations, and parameters.
If the runtime cannot persist them, it MUST NOT run the closure.

The runtime MUST encrypt the persisted parameters with the cipher it uses for [stored secret parameters](secrets.md#storage).
On replay, the runtime MUST pass the action closure the persisted parameters.

The runtime MUST clear the persisted operation when the operation ends.

## Interrupted operations

When the runtime starts, it MUST read the persisted operation, if any, and decide whether to replay it before the reconciliation loop first starts an operation.

The runtime MUST replay an interrupted install only if the app is registered, the persisted parameters can be read, and the app is `Installing` or `NotInstalled`.
The runtime MUST replay any other interrupted operation only if the app is registered, the persisted parameters can be read, and the app is `Installed`.

Where the runtime does not replay an interrupted operation, it MUST clear the persisted operation and give back the resources it [spared for the replay](suspension.md#abandoned-replay).
Where the operation not replayed was an install of an app in `Installing`, the runtime MUST file an [`install_interrupted`](faults.md#app-faults) fault and recover the app as a [stuck install](#stuck-install-recovery).

If the runtime cannot read whether an interrupted operation exists, it MUST NOT perform stuck-install recovery or [startup cleanup](suspension.md#abandoned-replay), since both act on that operation's absence.

### Unreadable operation state

If the runtime cannot read the persisted operation, whether a cancel was persisted for it, or the persisted queue, it MUST file an [`operation_state_unreadable`](faults.md#host-faults) fault.
Until the read succeeds, the runtime MUST NOT start any lifecycle operation, and MUST retry the read on each reconciliation iteration.
Once the read succeeds, the runtime MUST decide replay and restore its queue as on starting.

## Stuck-install recovery

The runtime MUST always give an operator a path out of an `Installing` phase that no operation holds.

When it starts, after deciding whether to replay, the runtime MUST move to `Uninstalling` every app that is `Installing` with no operation being replayed for it.
It MUST likewise move to `Uninstalling` every app that is `NotInstalled` while resource instances are still recorded for it.

The runtime MUST answer a cancel request for an app that is `Installing` with no operation in progress by moving the app to `Uninstalling`, and the request MUST succeed.

> [!NOTE]
> A partly installed app can already have running resources, and reconciliation leaves the resources of a `NotInstalled` app alone.
> Moving the app to `Uninstalling` tears those resources down, and the app reaches `NotInstalled` once teardown completes.

## Cancellation

The runtime MUST let an operator [cancel](../interface/actions.md#cancellation) the operation in progress for an app.

When a cancel is requested, the runtime MUST wake the operation if it is suspended on a [barrier](suspension.md#barriers), including a barrier with no deadline, so the cancel takes effect without waiting for the barrier's next re-evaluation or deadline.
The runtime MUST stop a cancelled operation at the next barrier or `rt.*` call it reaches.

The runtime MUST end a cancelled operation in an outcome distinct from success and from failure.
The runtime MUST clean up after a cancelled operation as after a failed one, tearing down its [dynamic resources](../language/app.md#dynamic-resource-lifetime) and clearing its persisted record.
The runtime MUST file an [`operation_cancelled`](faults.md#app-faults) fault, not an `operation_failed` one.

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
A script resolves the volume with `app.external_volume(param["<key>_volume"])`.

The runtime MUST NOT let a script choose the generated name, and the name MUST NOT appear among the parameters the operation was requested with.

> [!NOTE]
> A script reads the generated name from its parameter and never constructs it, so the form of the name past its prefix is not part of the contract.

Where the runtime needs a closure to use a particular file within a bound volume, it MAY pass the file name as the parameter `<key>_filename`.
A script handed `<key>_filename` MUST use that file name.

## Action composition

A script MAY run another action from within an action closure by calling [`Action.invoke(params?)`](../language/action.md#action-invocation) on a handle from [`app.action(name)`](../language/action.md#action-handles), `app.on_action()` or `app.on_start()`.

The runtime MUST run the invoked action's closure inline, within the calling operation, sharing its `rt`, its operation identifier and its action log.
A barrier in the invoked closure is a barrier of the calling operation.

### Cycles

An `Action.invoke` call whose action is already on the operation's call stack throws, as [action invocation](../language/action.md#action-invocation) specifies.
The call stack begins with the operation's own action.
The runtime MUST identify the actions on the stack by name.

### Sub-action parameters

The supplied parameters are validated before the invoked closure runs, as [action invocation](../language/action.md#action-invocation) specifies.

On replay, the runtime MUST run a call already recorded in the [action log](history.md#action-calls) with the parameters its entry holds, and MUST NOT validate them again.
If the entry at the call's position records a different action than the one being invoked, the runtime MUST fail the replay as a [divergence](suspension.md#positional-replay).

> [!NOTE]
> Validation consults the world as it stands: the site volumes that exist and the schema the current definition declares.
> Validating a recorded call again would stop an operation replaying because an operator deleted the volume it named or an update changed the schema after the call was committed.

## Shell sessions

A shell session is not a lifecycle operation.
The runtime MUST allow a shell session to run beside a lifecycle operation and beside other shell sessions.

Resources a shell session creates are dynamic, and the runtime MUST clean them up when the session ends.
The runtime MUST NOT persist a shell session's parameters, and MUST NOT replay a session after a restart.
