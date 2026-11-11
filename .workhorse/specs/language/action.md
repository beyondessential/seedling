---
id: ACN
---

# Actions

Applies to the actions, shells and install action a BSL script declares on an [app](../overview.md#app), the closures they run, and the params those closures receive.

## Terminology

### Action

A named task an app declares for an [operator](../overview.md#operator) or the [runtime](../overview.md#runtime) to run, carried out by an action closure.

### Action closure

A closure a script hands to the runtime to run with a [runtime handle](rt.md#runtime-handle-1): the closure of an action, a shell or the install action, or a parameter's [change handler](rule:l/param.on-change).

### Shell

A named interactive terminal session an app offers to operators, set up by an action closure.

### Param map

The object map from param names to values that an action closure receives from its invoker.

### Param schema

The definitions of the params an action, shell or install action accepts, keyed by param name.

## Action declaration

A script declares an action with `app.on_action(name: string, fn: closure)` or `app.on_action(name: string, fn: closure, options: map)`.
Either method returns an `Action` handle for the declared action.
A `name` that is not a valid [name](rule:l/bsl.name) MUST throw.

The `options` map MAY carry these keys:

| key | type | meaning |
| --- | --- | --- |
| `description` | string | free-form text telling operators the action's purpose |
| `params` | map | the action's param schema, as [param schemas](#param-schemas) describe |

An `options` map carrying any other key MUST throw.
A `description` that is not a string MUST throw.
A `params` that is not a map MUST throw.

The runtime MUST call `fn` with two arguments: a runtime handle, then the [param map](#param-map).
A closure declaring any other number of parameters MUST fail when the runtime calls it.

An action is not a [resource](../overview.md#resource).
An `Action` handle given where a runtime handle method expects a [selection](rt.md#selections) MUST make that method throw.

## Action handles

`app.action(name: string)` returns the `Action` handle of the action declared under `name`.
The handle MUST be equivalent to the one the declaring `app.on_action` or `app.on_start` call returned.
`app.action` called while no action closure is running MUST throw.
A `name` naming no declared action MUST throw.
The install action has no name, and `app.action` MUST NOT return it.

### Action invocation

`action.invoke()` and `action.invoke(params: map)` run the action's closure inline, within the [lifecycle operation](../overview.md#lifecycle-operation) of the calling action closure, and return no value.
`action.invoke()` MUST behave as `action.invoke(#{})`.
The runtime MUST call the invoked closure with the caller's runtime handle.
How the invoked closure shares the caller's operation is specified by [action composition](rule:r/operation.composition).

Every value in `params` MUST be representable in JSON: a unit, a boolean, an integer, a finite floating-point number, a string, or an array or map of such values.
Any other value MUST make `invoke` throw.
Before running the closure, the runtime MUST validate `params` against the action's param schema exactly as it validates an operator's invocation of the action.
If validation fails, `invoke` MUST throw without running the closure.

An exception the invoked closure throws MUST propagate to the caller of `invoke`.

`invoke` called while no action closure is running MUST throw.
`invoke` on an action already on the operation's call stack, directly or through other invocations, MUST throw before the closure runs.
The error SHOULD name the chain of actions that forms the cycle.

## Param map

The runtime MUST pass an action's, shell's or install action's closure the params its invoker supplied, after validation against its param schema, as the param map.
If the invoker supplies no params, the param map MUST be empty.

Param keys ending in `_volume` or `_filename` are reserved for the runtime.
The runtime MUST reject any invocation, by an operator or by `invoke`, whose supplied params include a reserved key.
The runtime MUST insert reserved keys only as [operation volume params](rule:r/operation.volume-param) specify.

> [!NOTE]
> A declared param's name is a valid name, which cannot contain `_`, so no declared param collides with a reserved key.

## Param schemas

A param schema is an object map from param name to a param definition, which is a map with these keys:

| key | type | default | meaning |
| --- | --- | --- | --- |
| `kind` | string | `"text"` | how the value is presented and validated, one of the [param kinds](#param-kinds) |
| `required` | boolean | `true` | whether the invocation must supply a value |
| `default_value` | string | none | the value taken when none is supplied |
| `description` | string | none | free-form text telling operators the value's meaning or purpose |
| `secret` | boolean | by kind | whether the value is sensitive |

Declaring a param schema MUST throw if any param name is not a valid name, any definition is not a map, a definition carries any other key, a key's value is not of the type in the table, or `kind` names no param kind.

`secret` defaults to `true` for the `password` and `weak-password` kinds and to `false` for every other kind.
The runtime MUST handle a param whose `secret` is `true` as it handles a [secret parameter](rule:l/param.schema.secret).

The runtime MUST validate an invocation's params against the schema, in this order, before the closure runs:

1. A declared param supplied as anything other than a string MUST be rejected.
2. A declared param that is absent or supplied as the empty string MUST take its `default_value`, if it has one.
3. A required param that is still absent or empty MUST be rejected.
4. Each declared param's value MUST then be validated as its kind specifies.

A param the schema does not declare MUST be passed to the closure as supplied.

A presentation layer SHOULD pre-fill a param's input with its `default_value`.
A presentation layer SHOULD NOT let an operator submit a required param empty.

### Param kinds

| kind | value |
| --- | --- |
| `text` | free-form text |
| `multiline` | free-form text that may span several lines |
| `email` | an email address |
| `password` | a strong password |
| `weak-password` | a password of any strength |
| `random` | free-form text, usually a generated random value |
| `volume` | the name of a site volume, as [volume params](#volume-params) specify |

The runtime MUST accept any string as a `text`, `multiline`, `weak-password` or `random` value.

A presentation layer SHOULD offer a multi-line input for a `multiline` param.

The runtime MUST reject an `email` value that contains no `@`, that has nothing before its first `@`, or whose part after the first `@` contains no `.` or begins or ends with `.`.
The runtime SHOULD NOT reject any other `email` value.
A presentation layer MAY warn about an `email` value that is valid but unlikely to be meant, such as `" "@example.org`.

The runtime MUST reject a `password` value it judges weak.
The runtime SHOULD judge a value weak when [zxcvbn](https://www.usenix.org/conference/usenixsecurity16/technical-sessions/presentation/wheeler) scores it below 3.
A presentation layer SHOULD indicate the strength of a `weak-password` value as it is entered.
A presentation layer SHOULD offer a generator of strong passwords for `password` and `weak-password` params.

A presentation layer SHOULD offer a one-click generator for a `random` param, beside manual entry.
That generator's default output SHOULD be 32 random bytes written as 64 lowercase hexadecimal digits.

> [!NOTE]
> The `random` kind does not imply `secret`, so a script keeping sensitive material in a `random` param sets `secret: true`.

### Volume params

The `volume` kind MUST be accepted in the param schemas of actions and shells.
It MUST throw in the install action's param schema and in a [parameter's kind](rule:l/param.schema.kind).

The runtime MUST reject an invocation whose `volume` param names a site volume that does not exist.
When it dispatches the invocation, the runtime MUST bind the named site volume for the operation, as [operation volume params](rule:r/operation.volume-param) specify.
In the param map the closure receives, the runtime MUST replace the param's own key with the key `<name>_volume`, whose value names that binding.
If the binding cannot be made at dispatch, the runtime MUST fail the invocation without running the closure.
A closure obtains the bound volume with `app.external_volume(param["<name>_volume"])`, as [operation-scoped external volumes](rule:l/volume.external.dynamic) specify.

> [!NOTE]
> A volume binding lasts for one operation, while a parameter or an install param outlives it.
> A script needing a permanent volume declares an external volume for the operator to map.

## Start action

The start action is the action named `start`, and brings the app up.
A script declares it with `app.on_start(fn: closure)`, `app.on_start(fn: closure, options: map)` or `app.on_action("start", …)`, each of which returns its `Action` handle.
The `options` of `app.on_start` MAY carry `description`, as for `app.on_action`.
An `options` map passed to `app.on_start` carrying any other key MUST throw.

The runtime MUST run the start action as a lifecycle operation when an install operation for the app completes.
The runtime MUST also run it when the runtime starts with the app installed.
The start action's closure MUST receive an empty param map when the runtime runs it.
If the script declares no start action, the runtime MUST run it as though its closure called `rt.start(app)`.

The runtime MUST NOT let an operator invoke the start action, and MUST answer an [action invocation](rule:i/action.invoke) naming it with `not_found`.
`on_schedule` on the start action's handle MUST throw.

## Shells

A script declares a shell with `app.on_shell(name: string, fn: closure)` or `app.on_shell(name: string, fn: closure, options: map)`, which return no value.
A `name` that is not a valid name MUST throw.
The `options` map takes the same keys as for `app.on_action`, and MUST throw on the same conditions.

Shells and actions MUST have separate namespaces.
A shell and an action MAY share a name, and each MUST remain reachable under it as its own kind.

The runtime MUST run a shell's closure only when an operator opens a session of the shell.
The runtime MUST call `fn` with three arguments: a runtime handle, a [shell control](#shell-control), then the param map.
A closure declaring any other number of parameters MUST fail when the runtime calls it.
The runtime MUST validate a shell's params as it validates an action's.
How a shell session relates to lifecycle operations is specified by [shell sessions](rule:r/operation.shell).

### Shell control

A shell control is a `ShellControl` value with two methods.

`shell.attach(job: Job)` designates the [job](rule:l/job.type) the session runs, and returns no value.
`shell.attach` given anything other than a Job MUST throw.
A second `shell.attach` within one run of the closure MUST throw.

`shell.error(message: string)` ends the session with an error carrying `message`, and MUST throw to end the closure.

Once the closure returns having called `shell.attach`, the runtime MUST start the designated job and connect the operator's input and output to it.
The session MUST last until the operator closes it or the connection is lost.
If the closure returns without calling `shell.attach`, or fails, the runtime MUST end the session with an error to the operator.
If the closure called `shell.error`, that error MUST carry `message`.

## Scheduled actions

`action.on_schedule(expr: string)` attaches a schedule to the action, and returns the same `Action` handle.
An action MAY carry several schedules.
An `expr` that is not a valid [schedule expression](#schedule-expressions) MUST throw.

Each time a schedule comes due, the runtime MUST run its action as [scheduled actions](../runtime/scheduled-actions.md) specify.
The closure of an action run by a schedule MUST receive an empty param map.

### Schedule expressions

A schedule expression is five fields separated by whitespace, minute, hour, day of month, month and day of week, optionally followed by a time zone.
Each field MUST be accepted as [POSIX `crontab`](https://pubs.opengroup.org/onlinepubs/9799919799/utilities/crontab.html) defines it, with these extensions:

| syntax | field | meaning |
| --- | --- | --- |
| `r/n` | any | every `n`th value of the range `r`, where `r` is `*`, `a-b`, or `a` standing for `a` to the field's maximum |
| `JAN`…`DEC` | month | months by name, in any case |
| `SUN`…`SAT` | day of week | days by name, in any case |
| `7` | day of week | Sunday |
| `L` | day of month | the last day of the month |
| `nW` | day of month | the weekday nearest day `n` that lies within the month |
| `nL` | day of week | the last day `n` of the month |
| `n#k` | day of week | the `k`th day `n` of the month |
| `H` | any | the field's hashed value |

A time zone, where present, MUST be a name from the [IANA Time Zone Database](https://www.iana.org/time-zones).
Where it is absent, the expression MUST be evaluated in the host's local time zone.

`H` MUST stand for the single value `min + (h mod (max - min + 1))`, where `h` is the schedule hash and `min` and `max` are the field's bounds: 0 and 59 for minute, 0 and 23 for hour, 1 and 31 for day of month, 1 and 12 for month, and 0 and 7 for day of week.
The schedule hash MUST be [SipHash-1-3](https://www.aumasson.jp/siphash/siphash.pdf) under an all-zero key, taken as an unsigned 64-bit integer, over the bytes of the app's name, a `0xff` byte, the bytes of the action's name and a `0xff` byte.

> [!NOTE]
> The hash depends on the two names alone, so a schedule fires at the same times on every host and across every release.
> `H 2 * * *` fires once a day, at a minute in the 02:00 hour that is fixed for the app and action.

## Install action

The install action sets the app up when an operator installs it.
A script declares it with `app.on_install(fn: closure)` or `app.on_install(fn: closure, config: map)`, which return no value.

The `config` map MAY carry `params`, the install action's param schema.
A `config` carrying any other key MUST throw.
A `params` that is not a map MUST throw.

The runtime MUST call `fn` with two arguments: a runtime handle, then the param map.
Every value in the install action's param map is a string.
The runtime MUST run the install action only when an operator [installs the app](rule:i/action.invoke.install).
The install action is not registered under a name, and neither `app.action` nor `invoke` can reach it.

If the script declares no install action, installing the app MUST behave as though the install action's closure called `rt.start(app)`.
That default MUST NOT run the start action's closure, which runs once the install completes.

Install param values MUST be held only for the duration of the install operation, and the runtime MUST discard them when it ends.
