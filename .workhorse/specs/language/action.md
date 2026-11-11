---
id: ACN
---

# Actions

## Action

<!-- from language: Action -->

> l[action.type]
> An Action is a mechanism made available to operators (and in some cases, used autonomously by the Seedling control plane) to perform a structured task on an application.
>
> Actions are defined using the `app.on_action(name: string, fn: closure, options?: object)` method, which returns an `Action`.
>
> An `Action` is an opaque handle that can be invoked from a dynamic context via [`Action.invoke()`](rule:l/action.call). It is not a Resource: it cannot be passed to `rt.start`, `rt.stop`, or any other resource-scheduling method, and it does not appear in resource collections produced by `app.select()` or `col(app)`.
>
> The `fn` closure must take exactly two arguments: the [Runtime Instance](rule:l/rt.var) (typically named `rt`) and the [param map](rule:l/action.params) (typically named `param`).
>
> The `options` [object map](https://rhai.rs/book/language/object-maps.html)'s available properties are described below:

> l[action.option-description]
> An Action's `description` option is free-form text provided to operators. It may describe what the action does or is for.

> l[action.option-params]
> An Action may declare a `params` schema in its `options` object. The value is an object map of param key to definition object, with the same structure and validation kinds as [Install Action params](rule:l/action.install.requirements).
>
> Params declared in the schema are validated against their kind and required/default rules before the action is scheduled. Additional params not mentioned in the schema are permitted and passed through as-is.
>
> If a `kind` value is provided but does not match any of the defined kinds, `on_action()` must throw.

> l[action.params]
> All action closures receive exactly two arguments: the [Runtime Instance](rule:l/rt.var) (`rt`) and a `Param` object map (`param`).
>
> The `param` is an arbitrary key-value map provided by the invoker. When no params are provided, `param` is an empty map (`#{}`).
>
> Param keys ending in `_volume` or `_filename` are reserved for internal use by the Seedling runtime. The runtime must reject operator-provided params whose keys end in either suffix. See [operation.volume-param](rule:r/operation.volume-param) for how the runtime uses these suffixes to hand operation-scoped volumes to action closures.

> l[action.lookup]
> `app.action(name: string)` returns the `Action` previously registered with that name. The handle returned is the same value that the original `app.on_action()` (or `app.on_start()`) call returned.
>
> `app.action()` is only valid in dynamic context (inside an action or shell closure). Calling it from the static context throws.
>
> If `name` does not match a registered action, `app.action()` throws. The Install Action is not registered under a name and is not reachable through this method (see [action.install](rule:l/action.install)).

> l[action.call]
> `Action.invoke(params?: object)` invokes the action's closure inline, in the calling context's runtime, with `params` (defaulting to an empty map). The method is named `invoke` rather than `call` because the host scripting engine reserves `.call(...)` for function-pointer dispatch and intercepts it before user-registered methods are consulted.
>
> Before the closure runs, the runtime validates `params` against the action's declared schema (see [action.option-params](rule:l/action.option-params)): required fields must be present, defaults are applied to absent optional fields, unknown or [reserved keys](rule:l/action.params) are rejected, and `kind: "volume"` references must resolve to an existing site volume. Validation errors throw before the closure runs. The validation rules are exactly those applied to operator invocations.
>
> The closure runs with the same `rt` as the caller and accumulates into the same operation log. There is no separate operation_id and no separate top-level history entry, but the runtime records a `SubActionInvoked` action-log entry capturing the called action's name and validated params (see [history.action-log.entries](rule:r/history.action-log.entries)).
>
> Exceptions thrown by the called closure propagate to the caller.
>
> `Action.invoke()` throws when invoked outside a dynamic context, and throws if the called action is already on the active call stack — directly (`bar` invoking `bar`) or transitively (`foo` → `bar` → `foo`). The error names the offending chain. See [operation.composition](rule:r/operation.composition).

## Start Action

<!-- from language: Action / Start Action -->

> l[action.start]
> The specialised Start Action is used to define how the application is started. It is used autonomously by the Seedling control plane.
>
> It may be defined using the `app.on_action()` method with a `name` of `"start"`, or with the shorthand `app.on_start(fn: closure, options?: object)`, which returns an `Action`.
>
> The `fn` closure must take exactly two arguments: `rt` and `param`. When fired autonomously (boot, schedule), `param` is an empty map.
>
> If it is not defined, it defaults to the equivalent of:
> ```rhai
> rt.start(app);
> ```

> l[action.start.no-manual-invoke]
> The Start Action is a lifecycle-only action. It must not be manually invokable via the action invocation RPC. Attempting to invoke it must return `not_found`.

## Shell Action

<!-- from language: Action / Shell Action -->

> l[action.shell]
> A Shell Action is a specialised _kind_ of Action, which provide an interactive terminal session to an operator. Shells are never used autonomously by the Seedling control plane.
>
> Shells must be defined using the `app.on_shell(name: string, fn: closure, options?: object)` method, and cannot be defined using `on_action()`.
>
> Shells exist in a separate namespace as other actions: their names do not conflict.
>
> The `fn` closure must take exactly three arguments: the [Runtime Instance](rule:l/rt.var) (typically named `rt`), the [Shell Control](rule:l/action.shell.control) (typically named `shell`), and the param map (typically named `param`).
>
> A shell closure that returns without calling `shell.attach` or `shell.error` is invalid. The runtime must return an error to the client.

> l[action.shell.control]
> The Shell Control is the second argument of the Shell Action. It is a custom type with two methods:
>
> - `shell.attach(job: Job)`: bridges the operator's input/output to the Job. Blocks until the operator closes the session or the connection is interrupted. Must be called exactly once per shell invocation. Calling `attach` a second time must throw.
> - `shell.error(msg: string)`: sends an error message to the client and terminates the shell session. This call is terminal: it throws an exception to end the closure.

> l[action.shell.attach]
> The Shell Attacher is the second argument of the Shell Action. It is a host function which bridges the operator to the [Job](rule:l/job.type) provided as argument, attaching the operator's input/output to that of the Job.
>
> The Shell Attacher returns once the operator closes the shell, or if the connection is interrupted in some other way.
> <!-- TODO: consider a return value to indicate how it exited -->

## Scheduled Actions

<!-- from language: Action / Scheduled Actions -->

> l[action.schedule]
> An Action returned by `on_action()` may be given one or more cron schedules via the `.on_schedule(expr: string)` builder method.
>
> `on_schedule` returns the Action for chaining. It may be called multiple times to attach multiple schedules to the same action.
>
> The `expr` is a 5-field cron expression (minute, hour, day-of-month, month, day-of-week) with 1-minute minimum resolution. The Jenkins `H` extension is supported: `H` is replaced with a stable hash-derived value within the field's range, computed from `(app_name, action_name)`. The value depends on those two names alone, so the same pair resolves `H` identically on every daemon, build and release: upgrading the daemon or its toolchain never moves a schedule. For example, `H 2 * * *` fires once daily at a stable minute during the 02:xx hour.
>
> An optional sixth field specifies the IANA timezone (e.g. `0 2 * * * Pacific/Auckland`). When omitted, the expression is interpreted in the system local timezone of the daemon.
>
> `on_schedule` must not be called on the Start Action (name `"start"`); doing so must throw. `on_schedule` is not available on Shell Actions.
>
> When a scheduled action fires, it is invoked as a normal lifecycle operation with an empty `param` map. Operators may also invoke a scheduled action manually via the action invocation RPC, in which case operator-provided params are passed through.

## Install Action

<!-- from language: Action / Install Action -->

> l[action.install]
> The specialised Install Action is used to define how the application is first set up. It is used autonomously by the Seedling control plane.
>
> It must be defined using the `app.on_install(fn: closure, config?: object)` method, and cannot be defined using `on_action()`.
>
> The `fn` closure must take exactly two arguments: `rt` and `param`. Param values are delivered through `param`. The `config` object defines the validation schema; it does not change the closure signature.
>
> The Install Action is not registered under a name in the app's action map. It is not reachable from [`app.action()`](rule:l/action.lookup) or [`Action.invoke()`](rule:l/action.call); it is invoked only by the control plane via the install RPC.
>
> If it is not defined, it defaults to the equivalent of:
> ```rhai
> rt.start(app);
> ```
> This schedules the App's static resources but does not invoke any custom action closure (such as `on_start`). The Start Action is dispatched by the control plane after `Installing` completes, not from inside the install fallback.

> l[action.install.requirements]
> The Install Action can define special parameters which are only requested from the operator when installation is requested. The values of the params are only known to Seedling for the duration of the Install process, and are discarded afterwards.
>
> The param schema is declared under the `params` key of the `config` object passed to `on_install()`: `#{ params: #{ key: #{ ... }, ... } }`.
>
> The `param` argument of the `fn` _closure_ is an object map of param key to string value.
>
> Each param definition has these fields:
> - `kind` (optional): how to present/validate the field, defaults to `"text"`;
> - `required` (optional): boolean, defaults to `true`;
> - `description` (optional): free-form text, for the operator to understand what the value is or is for;
> - `default_value` (optional): string, the value to use if none is provided;
> - `secret` (optional): boolean; defaults to `false`, but `"password"` and `"weak-password"` kinds imply `true` unless explicitly set to `false`. When `true`, the value is treated as sensitive for the duration of the install operation.
>
> If `default_value` is set and `required` is `true`, the default value is pre-populated in the field input (but the field is still mandatory and cannot be submitted empty).

> l[action.install.requirements.kind-text]
> A param kind of `"text"` is a free-form text field.
> No validation is applied.

> l[action.install.requirements.kind-multiline]
> A param kind of `"multiline"` is a free-form text field that may contain multiple lines.
> No validation is applied.
> It is semantically equivalent to `"text"` but hints to presentation layers (such as a web UI) that a multi-line input control is appropriate.

> l[action.install.requirements.kind-email]
> A param kind of `"email"` is an email address field.
> Basic validation is applied, and hints may be provided for more outlandish values ([for example](https://en.wikipedia.org/wiki/Email_address#Valid_email_addresses), `" "@example.org` is a valid email address, but probably not what an user meant).

> l[action.install.requirements.kind-password]
> A param kind of `"password"` is a strong password field.
> Weak passwords must not be accepted (what makes a weak password is implementation-defined, but should be something like [zxcvbn](https://lowe.github.io/tryzxcvbn/)).
> The field should have a strong password generator available.

> l[action.install.requirements.kind-weak-password]
> A param kind of `"weak-password"` is a free-form password field.
> Password strength should be hinted, but must not restrict submission.
> The field should have a strong password generator available.

> l[action.install.requirements.kind-random]
> A param kind of `"random"` is a free-form text field that hints to presentation layers (such as a web UI) that a one-click generator should be offered for producing a random value, alongside manual text entry.
> When a generator is offered, its default output is 32 bytes encoded as lowercase hexadecimal (a 64-character string).
> No validation is applied; the kind is semantically equivalent to `"text"` for storage and submission.
> The kind does not imply [`secret`](rule:l/param.schema.secret) — scripts that store sensitive material in a `"random"` param must set `secret(true)` explicitly.

> l[action.install.requirements.kind-unknown]
> If a param `kind` is provided but does not match any of the defined kinds, `on_install()` must throw.

> l[action.params.volume]
> Action and shell param schemas accept an additional kind, `"volume"`, that the static schemas (`app.param`, install requirements) must reject. When an action or shell schema declares a param with `kind: "volume"`, the runtime asks the operator to pick a site volume at invocation time, validates that the chosen volume exists, and at dispatch time builds an operation-scoped binding under the reserved key `<param-name>_volume` per [operation.volume-param](rule:r/operation.volume-param). The closure consumes it via `app.external_volume(param["<param-name>_volume"])`.
>
> The static schemas reject this kind because their bindings outlive any single operation; the equivalent for static configuration is a declared `external_volume` mapping wired up by the operator.
