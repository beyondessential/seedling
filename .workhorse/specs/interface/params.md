---
id: PRM
---

# Parameter management

Applies to how an app's parameter values are stored, the methods that set, unset and preview changes to them, and the validation that guards every change.

## Terminology

### Proposed values

The parameter values an app would hold if a request changing them were accepted.

### Unknown parameter

A parameter with a stored value whose name the app's current definition does not declare.

## Parameter storage

The runtime MUST store parameter values durably, keyed by app name and parameter name.
A parameter value is a string.
A parameter with no stored value is unset.

Every evaluation of an app's definition MUST be given the app's stored parameter values.

The runtime MUST store the value of a parameter whose effective [secret flag](../language/parameter.md#secret-parameters) is true in [protected storage](../runtime/secrets.md#storage), whether `/apps/params/set`, `/apps/params/unset` or [`/apps/update`](apps.md#update) writes it.
An event a change causes MUST NOT carry the plaintext of a value that is secret after the change, nor of one that was secret before it.

A client MAY set a value for an unknown parameter.
The runtime MUST store it, and it takes effect once the app's definition declares the parameter.

## Validation

The server MUST validate a definition update, a parameter set and a parameter unset before it persists any part of the change.

Validation runs the [validator](../language/parameter.md#validators) of every parameter that is set in the proposed values, against the proposed values.
The validators are those the proposed definition declares when evaluated with the proposed values.
For a parameter set or unset whose proposed values fail to evaluate, the validators are those of the app's most recent successful evaluation.
If that evaluation fails too, no validator runs.

If any validator rejects, the server MUST refuse the request with `validation_failed`, with a `message` naming each rejecting parameter and its reason, and MUST persist nothing.

Registering an app and reloading a stored app run no validators.

> [!NOTE]
> A newly registered app has no set parameters, and a stored combination was validated when it was written.

## Parameter set

`/apps/params/set` sets one parameter's value.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `name` | string | yes | the parameter's name, following the [naming rules](../language/overview.md#names) |
| `value` | string | yes | the value to set |

The result is `{ "schedule": "accepted" | "not_scheduled", "generation": <integer> }`, where `generation` is the app's current [generation](../overview.md#generation) once the request completes.

The server MUST refuse the request with `not_found` if the app is not registered.
It MUST refuse it with `operation_in_progress` if a lifecycle operation is running or queued for the app, an install of the app is in progress, or another change to the app is being applied.

If `value` equals the parameter's stored value, the request is a no-op: the server MUST change nothing and MUST return `not_scheduled` with the app's current generation.

Otherwise the server MUST validate the proposed values before it persists anything.
It MUST then store the value and bump the app's generation, with a `param_set` [history entry](apps.md#generation-history), as one durable change.

The server MUST then evaluate the app's definition with its new values.
If the evaluation fails, the server MUST file a `script_error` fault against the app, and the request still succeeds.

If the app is installed and its definition registers an `on_change` handler for the parameter that the change's [transition](../language/parameter.md#change-handlers) triggers, the server MUST schedule the handler as a [lifecycle operation](../overview.md#lifecycle-operation) and return `accepted`.
A handler queued behind another app's operation is accepted.
Otherwise the server MUST return `not_scheduled`.

If the server cannot admit the handler once the change is committed, it MUST refuse the request with `internal`, with a `message` saying that the change was applied and at which generation.

## Parameter unset

`/apps/params/unset` removes one parameter's stored value.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `name` | string | yes | the parameter's name |

The result has the same shape as `/apps/params/set`'s.

The server MUST refuse the request with `not_found` or `operation_in_progress` on the same terms as `/apps/params/set`.

If the parameter has no stored value, the request is a no-op: the server MUST change nothing and MUST return `not_scheduled` with the app's current generation.

Otherwise the server MUST validate the proposed values before it changes anything.
It MUST then remove the value and bump the app's generation, with a `param_unset` history entry, as one durable change.
It MUST then evaluate the definition, file a `script_error` fault if the evaluation fails, and schedule an `on_change` handler, as `/apps/params/set` does.

## Change planning

`/apps/plan` evaluates a hypothetical change to an app and reports how it would differ from the app's current state.
It MUST NOT change any stored definition, parameter value or other state.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `proposed_script` | string | no | a definition to evaluate in place of the current one, as `script` in [definition sources](definitions.md#definition-sources) |
| `proposed_bundle` | object | no | as `bundle` in definition sources |
| `proposed_reference` | string | no | as `reference` in definition sources |
| `proposed_params` | array | no | parameter changes to evaluate, each `{ "name": <string>, "value": <string or null> }`, where `null` unsets the parameter |

A request MUST carry at most one of `proposed_script`, `proposed_bundle` and `proposed_reference`, and the server MUST refuse one that carries more with `requirements_invalid`.
Without a proposed definition, the server MUST evaluate the app's current one.
A parameter that `proposed_params` does not list keeps its stored value.

The server MUST refuse the request with `not_found` if the app is not registered.
The server MUST refuse a proposed definition on the same terms as [`/apps/update`](apps.md#update) would, with the same error, before evaluating anything.

The result has these members.

| member | type | meaning |
| --- | --- | --- |
| `diff` | array | one entry per resource added, removed or modified |
| `on_change_would_fire` | array of strings | in ascending order, the parameters whose value the change alters and for which the proposed definition registers an `on_change` handler |
| `errors` | array of strings | the evaluation errors, if the proposed definition fails to evaluate |
| `rejections` | array | one `{ "name": <string>, "reason": <string> }` for each parameter whose validator rejects the proposed values |

If the proposed definition fails to evaluate, the result MUST hold `errors` alone.
Otherwise `errors` MUST be empty.
If the request carries neither a proposed definition nor `proposed_params`, every array in the result MUST be empty.

Each `diff` entry has these members.

| member | type | meaning |
| --- | --- | --- |
| `resource_type` | string | the resource's [type name](wire.md#resource-type-names) |
| `resource_name` | string | the resource's name |
| `change` | string | `"added"`, `"removed"` or `"modified"` |
| `fields` | array of strings | for a `modified` entry, in ascending order, the members of the resource's [`def`](apps.md#resources) whose values differ |

A resource present in both definitions MUST appear in `diff` only if a member of its `def` differs.

The server MUST NOT run any `on_change` handler or action closure to answer the request.
