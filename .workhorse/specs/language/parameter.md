---
id: PARAM
---

# Parameters

Applies to parameters: named string values that an [operator](../overview.md#operator) sets for an [app](../overview.md#app), and that its [script](overview.md#script) declares, reads and validates.

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

`param.parsed()` MUST return the [parser result](action.md#param-kinds) of the parameter's kind, as declared at the time of the call, for the value `param.value()` returns.
`param.parsed()` MUST throw where `param.value()` throws.
`param.parsed()` MUST throw the parser's error where the kind rejects the value.

> [!NOTE]
> A stored value passed its kind's check when it was written, but a kind whose arguments the script derives from something that has since changed can reject it.

## Schema

A script MAY attach schema metadata to a `Param` through the builder methods below.
The metadata governs how operator interfaces present and check the parameter.
Calling a schema method again MUST replace the value it set before.
A schema method MUST throw when called in the [action context](app.md#action-context).

| method | sets | default |
| --- | --- | --- |
| `kind(kind: string \| map)` | how the parameter is presented and checked, as a [kind](action.md#kind-arguments) | `"text"` |
| `required(required: bool)` | whether the parameter needs a value | `false` |
| `default_value(value: string \| bool \| int \| Duration)` | the value [`value()`](#values) returns when none is stored | none |
| `description(description: string)` | a human-readable description | none |
| `secret(secret: bool)` | whether the value is [secret](#secret-parameters) | implied by `kind` |

`default_value("")` MUST declare no default, and MUST remove any default declared before.
`default_value` given a boolean, an integer or a `Duration` MUST declare its [canonical form](action.md#kind-parsers) as the default.

`kind` MUST accept every valid [kind](action.md#kind-arguments) other than `volume`, with the meaning [param kinds](action.md#param-kinds) give it.
`kind` MUST throw when given the `volume` kind, which only action and shell [params](action.md#param-schemas) accept, or anything that is not a valid kind.

## Secret parameters

A parameter is secret when its `secret` flag is `true`.
A parameter whose kind is `"password"` or `"weak-password"` MUST be secret unless the script sets `secret(false)`.
A script MAY call `secret` before or after `kind`, and an explicit `secret` call MUST take precedence over the kind in either direction.
A parameter whose kind is `boolean` or `choice` MUST NOT be secret.
Once a script's evaluation has run to completion, the runtime MUST fail the evaluation if such a parameter is secret, naming the parameter.

> [!NOTE]
> Every value such a parameter can take is one the definition lists, so withholding it hides nothing, and masking it wherever it appears would mask the listed values themselves and show by elimination which one is set.

The runtime MUST protect a secret parameter's value as [secret storage](../runtime/secrets.md#storage) and [redaction](../runtime/secrets.md#redaction) specify.

## Kind checks

The runtime MUST check a parameter's proposed value with its kind's [parser](action.md#kind-parsers), and MUST reject a value the parser rejects, with the parser's error as the reason.
The runtime MUST check a value only while its parameter would be set, and an empty stored value is a value it checks.
The runtime MUST NOT run any [validator](#validators) while any parameter that would be set has a value its kind rejects.

> [!NOTE]
> Validators then always see every set parameter as its kind's parser result, so none mistakes a rejected value for an unset one.

Once a script's evaluation has run to completion, the runtime MUST check each declared default against its parameter's kind, and MUST fail the evaluation if the kind rejects one, naming the parameter and giving the parser's error.
The runtime MUST likewise fail the evaluation where a default was given as a boolean and the kind is not `boolean`, as an integer and the kind is neither `integer` nor `quantity`, or as a `Duration` and the kind is not `duration`.

> [!NOTE]
> `kind` and `default_value` may be called in either order, so the pair can be checked only once the script has finished declaring them.

When the runtime checks kinds, and which request a rejection refuses, is specified in [parameter validation](../interface/params.md#validation).

## Validators

`param.validate(fn: closure)` MUST attach `fn` as the parameter's validator, and MUST return the `Param`.
`validate` MUST throw when called in the action context.
Calling `validate` a second time for the same parameter MUST throw.

The runtime MUST call a validator with two arguments: the [parser result](action.md#param-kinds) of the parameter's proposed value, and an object map from the name of each declared parameter that would be set to the parser result of its proposed value.
A validator that returns accepts the value.
A validator that raises a script error rejects the value, whether the script threw deliberately or not.
A validator stopped at the [evaluation time limit](../runtime/engine-limits.md#measuring-evaluation-time) neither accepts nor rejects the value, and the request that ran it fails with `evaluation_runaway`, as [error codes](../interface/wire.md#error-codes) specify.
Where the script threw a value, that value MUST be the reason reported for the rejection.

The runtime MUST run a validator only while its parameter would be set.
A parameter that would be unset passes, and its `required` flag governs whether it needs a value.

The runtime MUST NOT give a validator a runtime handle.
A validator runs in the [validator context](app.md#validator-context), where every call that changes the app throws, as [validator immutability](app.md#validator-immutability) specifies.

> [!NOTE]
> A validator only judges a proposed value, so a validator that could change the app would make the definition depend on which values were checked.

When the runtime runs validators, and which request a rejection refuses, is specified in [parameter validation](../interface/params.md#validation).
