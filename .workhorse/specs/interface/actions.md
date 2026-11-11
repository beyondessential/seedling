---
id: ACI
---

# Action invocation

## Action Invocation

<!-- from interface: Action Invocation -->

> i[action.not-installed-gate]
> While an app is `NotInstalled` or `Installing`, all action and shell invocations except `/apps/install/invoke` are rejected with `not_installed`.

> i[action.invoke]
> `/apps/action/invoke { app, name, params? }` schedules the named action as a lifecycle operation.
> `params` is an optional JSON object. Keys ending in `_volume` or `_filename` are reserved for internal use (see [operation.volume-param](rule:r/operation.volume-param)) and must be rejected.
> If the app has an active `script_error` fault, the request is rejected with `script_error`.
> If the action has a declared param schema, schema-defined params are validated and defaults applied before the operation is enqueued; validation failure returns `requirements_invalid`.
> Shell actions must not be invoked via this method; `not_found` is returned if a shell name is provided.
> The Start Action (`name` = `"start"`) must not be invoked via this method; `not_found` is returned.
> Returns `{ "schedule": "accepted", "operation_id": "<string>" }` or `{ "schedule": "queued", "operation_id": "<string>" }` on success, or an error. The `operation_id` is always present and uniquely identifies this operation.

> i[action.cancel]
> `/apps/action/cancel { app }` requests cancellation of the currently-active lifecycle operation for the named app.
> The semantics of cancellation are defined by [operation.cancel](rule:r/operation.cancel): the runtime wakes any suspended barrier and drives the operation to a terminal cancelled state.
> If no operation is active for the app, the request is rejected with `not_found`.
> If a cancel is already pending for the active operation, the request is still accepted and is a no-op.
> On success, the response is `{ "cancelled": true }`.

> i[action.invoke.install]
> `/apps/install/invoke { app, params? }` schedules the install action.
> It is only valid when the app is `NotInstalled`.
> If the app is already `Installing`, the request is rejected with `install_in_progress`.
> If the app is `Installed` or `Uninstalling`, the request is rejected with `already_installed`.
> If the app has an active `script_error` fault, the request is rejected with `script_error`.
> `params` is an optional JSON object of param key to string value. The values are delivered to the install closure as `param`.
> If the app has no explicit install action, `params` must be absent or empty.
> Params are validated before the operation is enqueued; validation failure returns `requirements_invalid`.
> Returns `{ "schedule": "accepted" }` or `{ "schedule": "queued" }` on success, or an error.
> On `accepted`, the app atomically transitions to `Installing`.

> i[action.invoke.install.validation]
> Params are validated according to the kinds defined in the language spec before the operation is enqueued.
> A required field with no provided value and no `default_value` is a validation error.
> The params object is passed to the install action closure and is persisted alongside the operation record (see [operation.params](rule:r/operation.params)) so that a runtime restart during the install can replay the operation.
> Values of params whose effective `secret` flag is `true` must be protected in persisted form as defined in [secret.storage](rule:r/secret.storage).
> The persisted params are cleared when the install operation completes, regardless of outcome.

> i[action.invoke.install.completion]
> When an install operation completes successfully, the app transitions `Installing → Installed`.
> When an install operation fails, the app transitions `Installing → NotInstalled` and an `operation_failed` fault is filed carrying the failure detail.
> On subsequent runtime restarts, the runtime will initiate the `start` action for `Installed` apps automatically, as specified in the runtime spec.
