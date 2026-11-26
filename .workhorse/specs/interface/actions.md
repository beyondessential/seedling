---
id: ACI
---

# Action invocation

Applies to the [Operator Interface](../overview.md#operator-interface) methods that install an [app](../overview.md#app), invoke its actions, and cancel a [lifecycle operation](../overview.md#lifecycle-operation).

## Overlapping refusals

Where several of the refusals in this specification apply to one request, the server MAY answer with any of them.

## Invoking an action

`/apps/action/invoke` schedules one of an app's actions as a lifecycle operation.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `name` | string | yes | the action's name |
| `params` | object | no | parameter values for the action, keyed by parameter name |

The server MUST reject a request naming an app that is not registered with `not_found`.
The server MUST reject the request with `not_installed` unless the app is `Installed`.
If the app has an active `script_error` [fault](../overview.md#fault), the server MUST reject the request with `script_error`.
If the app has an active `evaluation_runaway` fault with an empty subject, the server MUST reject the request with `evaluation_runaway`.

The server MUST reject with `not_found` a `name` that names no action the app declares.

A `params` key ending in `_volume` or `_filename` is [reserved](../language/action.md#param-delivery), and the server MUST reject a request carrying one with `requirements_invalid`.
Where the action declares parameters, the server MUST validate the submitted values as [action parameters](#action-parameters) specifies before the operation is scheduled.
The value of a declared parameter MUST be of a type [action parameters](#action-parameters) allows, and the server MUST reject a value of any other type with `requirements_invalid` rather than treat the parameter as absent.
If a declared parameter of kind `volume` names a site volume that does not exist, the server MUST reject the request with `not_found`.
If the action handler's [breaker](../runtime/engine-limits.md#breakers) is tripped for the inputs the invocation would evaluate it with, the server MUST reject the request with `evaluation_runaway`.
The server MUST deliver `params` to the action closure as [param delivery](../language/action.md#param-delivery) specifies.

The server MUST schedule the operation with the trigger `"operator"`, subject to the [one-operation-at-a-time rules](../runtime/operations.md#concurrency).
The server MUST reject the request with `operation_in_progress` while an operation for the app is running or a change to the app is being applied.
The server MUST reject the request with `already_queued` while an operation for the app is queued.

On success the result is:

| member | type | meaning |
| --- | --- | --- |
| `schedule` | string | `"accepted"` when the operation has started, `"queued"` when it waits behind another app's operation |
| `operation_id` | string | the operation's identifier |

The server MUST give every operation an `operation_id` that no other operation on the host holds.

### Action parameters

Each value the server validates against a declared parameter MUST be a JSON string, except that a `boolean` parameter MAY take a JSON boolean and an `integer` or `quantity` parameter MAY take a JSON integer, each taken as its [param schema](../language/action.md#param-schemas) specifies.
A JSON integer is a JSON number written with no fraction and no exponent.
A JSON integer outside the signed 64-bit range MUST fail validation.
The server MUST validate each value against the parameter's declared [kind](../language/action.md#param-schemas).
The server MUST treat an empty string as no value.
The server MUST fill in a parameter with no value from its `default_value`, where it declares one.
A required parameter with neither a value nor a `default_value` MUST fail validation.
On any validation failure the server MUST reject the request with `requirements_invalid`, and its `message` MUST name each failing parameter and the reason.

## Install

`/apps/install` installs an app, moving it to `Installed`.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |

The server MUST reject a request naming an app that is not registered with `not_found`.
The server MUST reject the request with `already_installed` while the app is `Installed` or `Uninstalling`.
If the app has an active `script_error` fault, the server MUST reject the request with `script_error`, and if it has an active `evaluation_runaway` fault with an empty subject, with `evaluation_runaway`.
The server MUST reject the request with `operation_in_progress` while a change to the app is being applied.
Where several of these apply, the server MUST reject the request with the first in the order given.

Otherwise the server MUST durably move the app to `Installed`, emit an `AppPhaseChanged` [event](events.md#app-events), and answer `{}`.
The runtime then brings the app up as [desired state](../runtime/reconciliation.md#desired-state) specifies, running each of its [gates](../runtime/gates.md) for the first time.

> [!NOTE]
> An app needing an operator's input to finish setting up, such as an initial administrator, takes it through an action run after install.

## Uninstall

`/apps/uninstall` starts tearing an app down.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |

The server MUST reject a request naming an app that is not registered with `not_found`.
The server MUST reject the request with `not_installed` while the app is `NotInstalled` or `Uninstalling`.
The server MUST reject the request with `operation_in_progress` while [uninstall admission](../runtime/app-teardown.md#uninstall-admission) refuses it, with a `message` naming the action or change that holds the app.

Otherwise the server MUST durably move the app to `Uninstalling` before it answers, emit an `AppPhaseChanged` [event](events.md#app-events), and answer `{}`.
The runtime then tears the app down as [TRD](../runtime/app-teardown.md) specifies, and moves it to `NotInstalled` once teardown completes.

## Cancellation

`/apps/action/cancel` requests [cancellation](../runtime/operations.md#cancellation) of the app's running lifecycle operation.
It applies only to a running operation, and the server MUST NOT withdraw a queued operation in answer to it.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |

On success the result is `{ "cancelled": true }`.
A request for an operation whose cancellation is already pending MUST succeed and have no further effect.

If no operation is running for the app, the server MUST reject the request with `not_found`.
