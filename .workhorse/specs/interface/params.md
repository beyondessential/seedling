---
id: PRM
---

# Parameter management

## Change Planning

<!-- from interface: Change Planning -->

> i[plan.dry-run]
> `/apps/plan { app, proposed_script?, proposed_params? }` evaluates a hypothetical change against the app's current generation and returns a structured diff.
> Neither the script nor the parameter values stored on the server are modified by this call.
>
> Parameters:
>
> - `proposed_script`, `proposed_bundle`, `proposed_reference`: optional, at most one of them; a definition to evaluate in place of the current one, given as for `script`, `bundle`, and `reference` in [definition.source](rule:i/definition.source). If all are omitted, the current definition is used.
> - `proposed_params`: optional array of `{ name, value }` objects. `value` is `null` to model an unset; a string to model a set. Parameters not listed are taken from the current parameter map. If no proposed definition is given and `proposed_params` is omitted, the response is an empty diff.
>
> A proposed definition that cannot be fetched or whose bundle is refused returns the error an update with it would.
>
> The response contains:
>
> - `diff`: an array of resource diff entries, each with fields `resource_type`, `resource_name`, and `change` (`"added" | "removed" | "modified"`). For `modified` entries, a `fields` array lists the resource attributes that differ between current and proposed.
> - `on_change_would_fire`: an array of parameter names whose `on_change` handlers would be scheduled if the proposed change were committed.
> - `errors`: an array of evaluation errors, if the proposed script fails to evaluate. When present, `diff`, `on_change_would_fire`, and `rejections` are absent.
> - `rejections`: an array of `{ name, reason }` objects, one for each parameter whose [validator](rule:i/param.validation) rejects the proposed values. Empty when every validator accepts.
>
> The dry-run does not simulate the execution of `on_change` handlers or any action closures; it reports only the static diff and which handlers would be triggered.

## Param Management

<!-- from interface: Param Management -->

> i[param.store]
> Param values are stored durably, keyed by `(app_name, param_name)`.
> They are restored into the script scope on every script evaluation.
> A param with no stored value is treated as absent.

> i[param.store.secret]
> The `set` and `unset` operations must route secret parameter values (those whose effective `secret` flag is `true`) through protected storage as defined in [secret.storage](rule:r/secret.storage).
> Events emitted for secret parameter changes must never carry the plaintext values.

> i[param.set]
> `/apps/params/set { app, name, value }` persists the value and bumps the app's [generation](rule:r/generation.definition) with a `ParamSet` history entry. If the change matches one of the [transitions](rule:l/param.on-change.transitions) defined in the language spec, an `on_change` handler is registered for that param, and the app is installed, the handler is scheduled as a lifecycle operation.
>
> If a lifecycle operation is in progress for the app, or one is queued, the request is rejected with `operation_in_progress`; neither the value nor the generation is changed.
>
> If the requested value is equal to the current value, the request is a no-op: nothing is persisted, no generation bump occurs, and no handler is scheduled.
>
> Otherwise the proposed values are [validated](rule:i/param.validation) before anything is persisted; a rejection refuses the request with `validation_failed`, and neither the value nor the generation is changed.
>
> The script is re-evaluated after the value is persisted; if evaluation fails, a `script_error` app-level fault is filed and the request still succeeds.
>
> Returns `{ "schedule": "accepted" | "not_scheduled", "generation": <int> }` on success, or an error. `not_scheduled` means the generation was bumped but no `on_change` handler ran (for example, no handler is registered for the parameter, or the app is not installed). The returned `generation` is the app's current generation after the call (unchanged if the call was a no-op).

> i[param.validation]
> A definition update, parameter set, or parameter unset is validated before anything it changes is persisted.
> Validation runs the [validator](rule:l/param.validate) of every set parameter against the proposed parameter values, which are the values the app would hold if the request were accepted.
> If any validator rejects, the request is refused with `validation_failed`, the message naming each rejecting parameter and its reason, and nothing is persisted.
> The validators are those declared by the proposed definition evaluated with the proposed values.
> For a parameter set or unset whose proposed values fail to evaluate, they are those of the app's most recent successful evaluation.
> Registration and reload run no validators: a newly registered app has no set parameters, and a stored combination was validated when it was written.

> i[param.unknown]
> Setting a param whose name does not appear in the app's current script evaluation is permitted.
> The value is stored and will take effect when the script is next evaluated.

> i[param.unset]
> `/apps/params/unset { app, name }` removes the stored value for the named parameter and reloads the script.
>
> If a lifecycle operation is in progress for the app, or one is queued, the request is rejected with `operation_in_progress`; neither the value nor the generation is changed.
>
> If the parameter has no stored value, the request is a no-op: nothing is changed, no generation bump occurs, and no handler is scheduled.
>
> Otherwise the proposed values are [validated](rule:i/param.validation) before anything is changed; a rejection refuses the request with `validation_failed`, and neither the value nor the generation is changed.
>
> Once validated, the app's [generation](rule:r/generation.definition) is bumped with a `ParamUnset` history entry. If an `on_change` handler is registered for the parameter and the app is installed, the handler is scheduled as a lifecycle operation.
>
> The script is re-evaluated after the value is removed; if evaluation fails, a `script_error` app-level fault is filed and the request still succeeds.
>
> Returns `{ "schedule": "accepted" | "not_scheduled", "generation": <int> }` on success, or an error.
