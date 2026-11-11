---
id: ACI
---

# Action invocation

Applies to the [Operator Interface](../overview.md#operator-interface) methods that invoke an [app](../overview.md#app)'s install and actions, and that cancel a [lifecycle operation](../overview.md#lifecycle-operation).

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

The server MUST reject with `not_found` a `name` of `"start"`, and a `name` that names no action the app declares.

A `params` key ending in `_volume` or `_filename` is [reserved](../language/action.md#param-delivery), and the server MUST reject a request carrying one with `requirements_invalid`.
Where the action declares parameters, the server MUST validate the submitted values and fill in defaults as for [install parameters](#install-parameters) before the operation is scheduled.
The value of a declared parameter MUST be a JSON string, and the server MUST reject a value of any other type with `requirements_invalid` rather than treat the parameter as absent.
If a declared parameter of kind `volume` names a site volume that does not exist, the server MUST reject the request with `not_found`.
The server MUST deliver `params`, with defaults filled in, to the action closure as `param`.

The server MUST schedule the operation with the trigger `"operator"`, subject to the [one-operation-at-a-time rules](../runtime/operations.md#concurrency).
The server MUST reject the request with `operation_in_progress` while an operation for the app is running or a change to the app is being applied.
The server MUST reject the request with `already_queued` while an operation for the app is queued.
The server MUST reject the request with `not_installed` once the app has begun uninstalling.

On success the result is:

| member | type | meaning |
| --- | --- | --- |
| `schedule` | string | `"accepted"` when the operation has started, `"queued"` when it waits behind another app's operation |
| `operation_id` | string | the operation's identifier |

The server MUST give every operation an `operation_id` that no other operation on the host holds.

## Install

`/apps/install/invoke` schedules an app's install, which moves it through the [install phases](../overview.md#install-phase).

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `params` | object of strings | no | install parameter values, keyed by parameter name |

The server MUST reject a request naming an app that is not registered with `not_found`.
The server MUST reject the request with `install_in_progress` while the app is `Installing`.
The server MUST reject the request with `already_installed` while the app is `Installed` or `Uninstalling`.
If the app has an active `script_error` fault, the server MUST reject the request with `script_error`.

If the app declares no install action, `params` MUST be absent or empty, and the server MUST reject the request with `requirements_invalid` otherwise.
The server MUST then move the app to `Installed` at once and answer `{ "schedule": "accepted" }`, with no `operation_id`.

If the app declares an install action, the server MUST validate `params` as [install parameters](#install-parameters) specifies and schedule the install as a lifecycle operation named `install` with the trigger `"operator"`.
The server MUST reject the request with `operation_in_progress` or `already_queued` on the same terms as [action invocation](#invoking-an-action), and its result is as for action invocation.

The app MUST be `Installing` from the time its install operation starts, whether the operation was accepted at once or started later from the queue.
A queued install MUST leave the app `NotInstalled` until the install starts.

## Install parameters

Each install parameter value MUST be a JSON string.
The server MUST validate each value against the parameter's declared [kind](../language/action.md#param-schemas) before the operation is scheduled.
The server MUST treat an empty string as no value.
The server MUST fill in a parameter with no value from its `default_value`, where it declares one.
A required parameter with neither a value nor a `default_value` MUST fail validation.
On any validation failure the server MUST reject the request with `requirements_invalid`, and its `message` MUST name each failing parameter and the reason.

The server MUST deliver the validated values to the install closure as `param`.
The server MUST persist the values with the operation record, as [operation parameters](../runtime/operations.md#persistence) specifies, so that a runtime restart during the install can replay it.
The persisted values MUST be protected as [secret storage](../runtime/secrets.md#storage) requires.
The server MUST discard the persisted values when the install operation ends, whatever its outcome.

## Install completion

When an install operation completes successfully, the runtime MUST move the app from `Installing` to `Installed`.
When an install operation fails or is cancelled, the runtime MUST move the app from `Installing` to `NotInstalled`.

## Uninstall

`/apps/uninstall` starts tearing an app down.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |

The server MUST reject a request naming an app that is not registered with `not_found`.
The server MUST reject the request with `operation_in_progress` while the app is `Installing`, and with `not_installed` while it is `NotInstalled` or `Uninstalling`.
The server MUST reject the request with `operation_in_progress` while [uninstall admission](../runtime/app-teardown.md#uninstall-admission) refuses it, with a `message` naming the action or change that holds the app.

Otherwise the server MUST durably move the app to `Uninstalling` before it answers, emit an `AppPhaseChanged` [event](events.md#event-types), and answer `{}`.
The runtime then tears the app down as [TRD](../runtime/app-teardown.md) specifies, and moves it to `NotInstalled` once teardown completes.

## Cancellation

`/apps/action/cancel` requests [cancellation](../runtime/operations.md#cancellation) of the app's running lifecycle operation.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |

On success the result is `{ "cancelled": true }`.
A request for an operation whose cancellation is already pending MUST succeed and have no further effect.

If no operation is running for the app and the app is `Installing`, the server MUST move the app to `Uninstalling`, as [stuck-install recovery](../runtime/operations.md#stuck-install-recovery) requires, and answer `{ "cancelled": true, "recovered_stuck_phase": "installing" }`.
If no operation is running for the app and the app is in any other phase, the server MUST reject the request with `not_found`.
