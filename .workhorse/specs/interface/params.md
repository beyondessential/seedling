---
id: PRM
---

# Parameter management

Applies to how an app's parameter values are stored, the methods that change and preview changes to them, and the validation that guards every change.

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

The runtime MUST store the value of a parameter whose effective [secret flag](../language/parameter.md#secret-parameters) is true in [protected storage](../runtime/secrets.md#storage), whether [`/apps/params/set`](#parameter-changes) or [`/apps/update`](apps.md#update) writes it.
The events a change causes withhold parameter values as [SEC](../runtime/secrets.md#generation-history) specifies.

A client MAY set a value for an unknown parameter.
The runtime MUST store it, and it takes effect once the app's definition declares the parameter.

## Validation

The server MUST validate a definition update and a parameter change before it persists any part of the change.

Validation runs the [validator](../language/parameter.md#validators) of every parameter that is set in the proposed values, against the proposed values.
The validators are those the proposed definition declares when evaluated with the proposed values.
For a parameter change whose proposed values fail to evaluate, the validators are those of the app's most recent successful evaluation.
If that evaluation fails too, no validator runs.

If any validator rejects, the server MUST refuse the request with `validation_failed`, with a `message` naming each rejecting parameter and its reason, and MUST persist nothing.

Registering an app and reloading a stored app run no validators.

> [!NOTE]
> A newly registered app has no set parameters, and a stored combination was validated when it was written.

## Parameter changes

`/apps/params/set` changes the stored values of one or more of an app's parameters, as one change.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `params` | object | yes | one member per parameter to change, named for the parameter, whose value is a string to set or `null` to unset |

The result is `{ "generation": <integer> }`, the app's current [generation](../overview.md#generation) once the request completes.

The server MUST refuse the request with `requirements_invalid` if `params` has no members, names a parameter more than once, has a member whose name breaks the [naming rules](../language/overview.md#names), or has a member whose value is neither a string nor `null`.
It MUST refuse it with `not_found` if the app is not registered.
It MUST refuse it with `operation_in_progress` if another change to the app is being applied.

A member that gives a parameter's stored value, or `null` for a parameter with no stored value, changes nothing.
If no member of `params` changes anything, the request is a no-op: the server MUST change nothing and MUST return the app's current generation.

Otherwise the server MUST validate the proposed values before it persists anything.
It MUST then store every value the request sets, remove every value it unsets, and bump the app's generation once, with a `param_change` [history entry](apps.md#generation-history), as one durable change.
Once the change is durable, it MUST emit a `ParamsChanged` [event](events.md#app-events).

The server MUST then evaluate the app's definition with its new values.
If the evaluation fails, the server MUST file a [`script_error`](../runtime/faults.md#app-faults) fault against the app, and the request still succeeds.

A change that moves the key of one of the app's [gates](../language/gate.md) leaves it unsatisfied, and the runtime runs it as [gate runs](../runtime/gates.md) specify.

> [!NOTE]
> Parameters that belong together, such as a host and the port served on it, move in one generation, so no generation holds one without the other and each validator sees them together.

## Change planning

`/apps/plan` evaluates a hypothetical change to an app and reports how it would differ from the app's current state.
It MUST NOT change any stored definition, parameter value or other state.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `proposed_script` | string | no | a definition to evaluate in place of the current one, as `script` in [definition sources](definitions.md#definition-sources) |
| `proposed_bundle` | object | no | as `bundle` in definition sources |
| `proposed_reference` | string | no | as `reference` in definition sources |
| `proposed_params` | object | no | parameter changes to evaluate, in the form of `params` in [parameter changes](#parameter-changes) |

A request MUST carry at most one of `proposed_script`, `proposed_bundle` and `proposed_reference`, and the server MUST refuse one that carries more with `requirements_invalid`.
Without a proposed definition, the server MUST evaluate the app's current one.
A parameter that `proposed_params` has no member for keeps its stored value.

The server MUST refuse the request with `not_found` if the app is not registered.
The server MUST refuse a proposed definition that cannot be fetched, breaks the bundle rules, or does not support the running Seedling version, with the error [`/apps/update`](apps.md#update) would give, before evaluating anything.
The server MUST NOT refuse the request because the evaluated definition, proposed or current, fails to parse or evaluate.

The result has these members.

| member | type | meaning |
| --- | --- | --- |
| `diff` | array | one entry per resource added, removed or modified |
| `gates_would_run` | array of strings | where the app is installed, in ascending order, the names of the gates the evaluated definition declares that would be [unsatisfied](../runtime/gates.md#gate-state) under it and the proposed values |
| `errors` | array of strings | the parse or evaluation errors, if the evaluated definition fails to parse or evaluate |
| `rejections` | array | one `{ "name": <string>, "reason": <string> }` for each parameter whose validator rejects the proposed values |

If the evaluated definition fails to parse or evaluate, the result MUST carry `errors` and no other member.
Otherwise `errors` MUST be empty.
If the request carries neither a proposed definition nor `proposed_params`, `diff` and `rejections` MUST be empty.

Each `diff` entry has these members.

| member | type | meaning |
| --- | --- | --- |
| `resource_type` | string | the resource's [type name](wire.md#resource-type-names) |
| `resource_name` | string | the resource's name |
| `change` | string | `"added"`, `"removed"` or `"modified"` |
| `fields` | array of strings | for a `modified` entry, in ascending order, the members of the resource's [`def`](apps.md#resources) whose values differ |

A resource present in both definitions MUST appear in `diff` only if a member of its `def` differs.

The server MUST NOT run any gate, gate check or action closure to answer the request.
