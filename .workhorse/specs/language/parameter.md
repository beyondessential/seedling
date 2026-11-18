---
id: PARAM
---

# Parameters

Applies to parameters: named string values that an [operator](../overview.md#operator) sets for an [app](../overview.md#app), and that its [script](overview.md#script) declares, reads, validates and reacts to.

## Terminology

### Parameter

A named string value that an operator may store for an app, declared by the app's script.

### Stored value

The value an operator has stored for a parameter, where there is one.

## Declaration

`app.param(name: string)` MUST return a `Param` for the parameter `name`.
It MUST throw if `name` breaks the [name rules](overview.md#names).
Calls with the same name MUST return a `Param` for the same parameter.

## Values

`param.is_set()` MUST return `true` if the parameter has a stored value, and `false` otherwise.

`param.value()` MUST return the parameter's stored value.
If the parameter has no stored value and a default is declared, `param.value()` MUST return the default.
If the parameter has neither, `param.value()` MUST throw.

`param.is_set()` MUST return `false` while a default is in effect, so that a script can tell an operator's value from a default.

## Schema

A script MAY attach schema metadata to a `Param` through the builder methods below.
The metadata governs how operator interfaces present and check the parameter.
Calling a schema method again MUST replace the value it set before.
A schema method MUST throw when called in the [action context](app.md#action-context).

| method | sets | default |
| --- | --- | --- |
| `kind(kind: string)` | how the parameter is presented and checked | `"text"` |
| `required(required: bool)` | whether the parameter needs a value | `false` |
| `default_value(value: string)` | the value [`value()`](#values) returns when none is stored | none |
| `description(description: string)` | a human-readable description | none |
| `secret(secret: bool)` | whether the value is [secret](#secret-parameters) | implied by `kind` |

`kind` MUST accept `"text"`, `"multiline"`, `"email"`, `"password"`, `"weak-password"` and `"random"`, each with the meaning [param kinds](action.md#param-kinds) give it.
`kind` MUST throw when given `"volume"`, which only action and shell [params](action.md#param-schemas) accept, or any value it does not define.

## Secret parameters

A parameter is secret when its `secret` flag is `true`.
A parameter whose kind is `"password"` or `"weak-password"` MUST be secret unless the script sets `secret(false)`.
A script MAY call `secret` before or after `kind`, and an explicit `secret` call MUST take precedence over the kind in either direction.

The runtime MUST protect a secret parameter's value as [secret storage](../runtime/secrets.md#storage) and [redaction](../runtime/secrets.md#redaction) specify.

## Change handlers

`param.on_change(fn: closure)` MUST register `fn` as the parameter's change handler, and returns no value.
`on_change` MUST throw when called in the action context.
Calling `on_change` a second time for the same parameter MUST throw.

While the app is `Installed`, the runtime MUST invoke the change handler as a [lifecycle operation](../overview.md#lifecycle-operation) whenever the parameter's stored value changes in one of these ways:

- the parameter had no stored value and is given one;
- the parameter's stored value is replaced by a different value;
- the parameter's stored value is removed.

Storing the value the parameter already holds is not a change.
The runtime MUST NOT invoke the handler while the app is in any other [install phase](../overview.md#install-phase).

A script MAY pass a closure taking up to two parameters.
The runtime MUST call `fn` with as many of these arguments as it takes, in this order: the [runtime handle](rt.md#runtime-handle), then an `App` named here `old`.
A closure declaring more than two parameters MUST fail when the runtime calls it.

`old` MUST reflect the previous [generation](../overview.md#generation): the previous generation's definition, evaluated with the parameter values held before the change.
When the change arrives together with a new definition, the runtime MUST invoke the handler the new definition registers, and `old` MUST reflect the previous definition at the previous values.
Within the handler, `old.param(name)` MUST report the parameter's state before the change, and `app.param(name)` its state after.
The static resources and actions reachable through `old` MUST reflect their definitions in the previous generation.

`old` is a read-only view.
Calling a resource method on `old` MUST NOT affect the app.

If the runtime cannot reconstruct the previous generation, it MUST NOT invoke the handler with an `old` that does not reflect that generation.
The outcome of the operation is then specified by [initiating events](../runtime/operations.md#initiating-events).

## Validators

`param.validate(fn: closure)` MUST attach `fn` as the parameter's validator, and MUST return the `Param`.
`validate` MUST throw when called in the action context.
Calling `validate` a second time for the same parameter MUST throw.

The runtime MUST call a validator with two arguments: the parameter's proposed value, and an object map from the name of each parameter that would be set to its proposed value.
A validator that returns accepts the value.
A validator that raises a script error rejects the value, whether the script threw deliberately or not.
Where the script threw a value, that value MUST be the reason reported for the rejection.

The runtime MUST run a validator only while its parameter would be set.
A parameter that would be unset passes, and its `required` flag governs whether it needs a value.

The runtime MUST NOT give a validator a runtime handle.
A validator runs in the [validator context](app.md#validator-context), where every call that changes the app throws, as [validator immutability](app.md#validator-immutability) specifies.

> [!NOTE]
> A validator only judges a proposed value, so a validator that could change the app would make the definition depend on which values were checked.

When the runtime runs validators, and which request a rejection refuses, is specified in [parameter validation](../interface/params.md#validation).
