---
id: PARAM
---

# Parameters

## Parameter

<!-- from language: Parameter -->

> l[param.type]
> A Parameter is a value provided by the Seedling control plane to a BSL script, at a particular name.
>
> Parameters are defined using the `app.param(name: string)` method, which returns a `Param`.

> l[param.is-set]
> `param.is_set()` returns `true` if the operator has stored a value for this parameter, `false` otherwise.

> l[param.value]
> `param.value()` returns the parameter's current string value.
> If no value has been stored but a [default](rule:l/param.schema.default-value) has been declared on the `Param`, the default is returned.
> If no value has been stored and no default is declared, it throws.
>
> `param.is_set()` is not affected by the default: it remains `false` while the default is the effective value, so scripts can distinguish "operator-provided" from "defaulted".

> l[param.on-change]
> `param.on_change(fn: closure)` registers a handler that is called when the parameter's value changes.
>
> The `fn` closure may take up to two arguments: the [Runtime Instance](rule:l/rt.var) (typically named `rt`) and the previous `App` instance (typically named `old`).

> l[param.on-change.old]
> The `old` argument is an `App` value that reflects the state at the previous [generation](rule:r/generation.definition): the previous generation's definition, evaluated with the parameter values as they were before the change.
> When the change arrives together with a new definition, the handler is the one registered by the new definition, and `old` is the previous definition at the previous values.
>
> `old.param(name).is_set()` and `old.param(name).value()` return results consistent with the prior parameter state.
> Static resources defined in the script, and actions, are accessible via `old` and reflect their prior definitions. This is useful for resources whose shape depends on parameter values.
>
> Calling resource-definition methods on `old` (that would create or mutate resources) has no effect on the current app; `old` is a read-only view of the previous generation.

> l[param.on-change.transitions]
> The handler is invoked as a [lifecycle operation](rule:r/operation.lifecycle) when the effective value of the parameter transitions between states. For this purpose the value of a parameter is always `Option<Value>`: either unset (`None`) or set to some string (`Some(s)`).
>
> The handler is invoked on any of the following transitions:
>
> - `None` → `Some(s)`: the parameter was unset and has now been set.
> - `Some(s₁)` → `Some(s₂)` where `s₁ ≠ s₂`: the value has changed.
> - `Some(s)` → `None`: the parameter has been unset.
>
> Within the closure, `old.param(name).is_set()` reflects the prior half of the transition, and `app.param(name).is_set()` reflects the new half.

> l[param.on-change.not-on-install]
> The handler is not invoked during the initial install of an application, because there is no prior generation to compare against.

> l[param.on-change.constraints]
> `on_change` may only be called at the top level of the script (statically). Calling it from within an action closure must throw.
> Calling `on_change` more than once on the same parameter must throw.

> l[param.schema]
> A `Param` returned by `app.param()` may have schema metadata attached to it using builder methods. These methods return the same `Param` so calls can be chained.
>
> Schema metadata describes how the parameter should be presented and validated in management interfaces. Defaults: `kind` is `"text"`, `required` is `false`, `default_value` is absent, `description` is absent, `secret` is `false` (unless implied by `kind`).

> l[param.schema.kind]
> `param.kind(kind: string)` sets the kind of the parameter. Valid values are the same as for [Install Action params](rule:l/action.install.requirements): `"text"`, `"multiline"`, `"email"`, `"password"`, `"weak-password"`. Providing an unknown kind must throw.

> l[param.schema.required]
> `param.required(required: bool)` sets whether the parameter is required.

> l[param.schema.default-value]
> `param.default_value(value: string)` sets the default value for the parameter.

> l[param.schema.description]
> `param.description(description: string)` sets the human-readable description for the parameter.

> l[param.schema.secret]
> `param.secret(secret: bool)` marks the parameter as sensitive. When `true`, the parameter's value must be stored with confidentiality at rest, must never be returned to API clients or emitted in events, and must not appear in logs.

> l[param.schema.secret-from-kind]
> Parameters with `kind` `"password"` or `"weak-password"` are implicitly secret: their `secret` flag is treated as `true` unless explicitly overridden with `param.secret(false)`.
> The `secret` builder may be called after `kind` to override this implication in either direction.

> l[param.validate]
> `param.validate(fn: closure)` attaches a validator to the parameter, which decides whether a proposed value is acceptable.
> The closure takes two arguments: the parameter's proposed value, and an object map from each set parameter's name to its proposed value.
> Returning accepts the value.
> Throwing rejects it, and the thrown value is the reason reported for the rejection; any exception raised within the closure rejects the value, whether thrown deliberately or not.
> When the runtime runs validators, and what a rejection refuses, is defined in [param.validation](rule:i/param.validation).

> l[param.validate.unset]
> A validator runs only while its parameter is set.
> An unset parameter always passes, and [required](rule:l/param.schema.required) is what governs whether a value must be present.

> l[param.validate.pure]
> A validator is given no [runtime instance](rule:l/rt.var) and its result is its only effect.
> Defining a resource within a validator throws.

> l[param.validate.constraints]
> `validate` may only be called at the top level of the script (statically). Calling it from within an action closure must throw.
> Calling `validate` more than once on the same parameter must throw.
