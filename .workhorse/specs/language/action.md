---
id: ACN
---

# Actions

Applies to the actions and shells a BSL script declares on an [app](../overview.md#app), the closures they run, and the params those closures receive.

## Terminology

### Action

A named task an app declares for an [operator](../overview.md#operator) or the [runtime](../overview.md#runtime) to run, carried out by an action closure.

### Action closure

A closure a script hands to the runtime to run with a [runtime handle](rt.md#runtime-handle): the closure of an action or a shell, or a [gate's body](gate.md#body).

### Shell

A named interactive terminal session an app offers to operators, set up by an action closure.

### Param map

The object map from param names to values that an action closure receives from its invoker.

### Param schema

The definitions of the params an action or shell accepts, keyed by param name.

## Action declaration

A script declares an action with `app.on_action(name: string, fn: closure)` or `app.on_action(name: string, fn: closure, options: map)`.
Either method returns an `Action` handle for the declared action.
A `name` that is not a valid [name](overview.md#names) MUST throw.
Declaring an action under a name an action of the app is already declared under MUST throw.
`app.on_action` MUST throw when called in the [action context](app.md#action-context).

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

An action is not a [resource](../overview.md#resource), and a runtime handle method given one as a [selection](rt.md#selections) throws.

## Action handles

`app.action(name: string)` returns the `Action` handle of the action declared under `name`.
The handle MUST be equivalent to the one the declaring `app.on_action` call returned.
`app.action` MUST throw when called outside the action context.
A `name` naming no declared action MUST throw.

### Action invocation

`action.invoke()` and `action.invoke(params: map)` run the action's closure inline, within the [lifecycle operation](../overview.md#lifecycle-operation) of the calling action closure, and return no value.
`action.invoke()` MUST behave as `action.invoke(#{})`.
The runtime MUST call the invoked closure with the caller's runtime handle.
How the invoked closure shares the caller's operation is specified by [action composition](../runtime/operations.md#action-composition).

Every value in `params` MUST be a unit, a boolean, an integer, a finite floating-point number, a string, a `Version`, a `SemVer`, an `IpAddress`, an `IpSubnet`, an `IpRange`, or an array or map of such values.
Any other value MUST make `invoke` throw.
A `Version` or `SemVer` MUST be taken as its [text](version.md#text-of-a-version).
An `IpAddress`, `IpSubnet` or `IpRange` MUST be taken as its [canonical form](ip.md#text-of-a-value).

> [!NOTE]
> A closure can pass the param map it received to `invoke` unchanged.

Before running the closure, the runtime MUST validate `params` against the action's param schema exactly as it validates an operator's invocation of the action.
If validation fails, `invoke` MUST throw without running the closure.

An exception the invoked closure throws MUST propagate to the caller of `invoke`.

`invoke` MUST throw when called outside the action context.
`invoke` on an action already on the operation's [call stack](../runtime/operations.md#cycles), directly or through other invocations, MUST throw before the closure runs.
The error MUST name each action on the stack, in call order, followed by the action being invoked.

## Param delivery

The runtime MUST pass an action's or shell's closure the params its invoker supplied, after validation against its param schema, as the param map.
If the invoker supplies no params, the param map MUST be empty.
In the param map, a declared param that has a value after validation MUST hold its kind's [parser result](#param-kinds) for that value, except as [volume params](#volume-params) specify.
A declared param that has no value after validation MUST be absent from the param map.

Param keys ending in `_volume` or `_filename` are reserved for the runtime.
The runtime MUST reject any invocation, by an operator, by a shell request or by `invoke`, whose supplied params include a reserved key.
The runtime MUST insert reserved keys only as [operation volume params](../runtime/operations.md#operation-scoped-volumes) specify.

> [!NOTE]
> A declared param's name is a valid name, which cannot contain `_`, so no declared param collides with a reserved key.

## Param schemas

A param schema is an object map from param name to a param definition, which is a map with these keys:

| key | type | default | meaning |
| --- | --- | --- | --- |
| `kind` | string or map | `"text"` | how the value is presented and validated, as a [kind](#kind-arguments) |
| `required` | boolean | `true` | whether the param needs a value |
| `default_value` | string, boolean or integer | none | the value taken when none is supplied |
| `description` | string | none | free-form text telling operators the value's meaning or purpose |
| `secret` | boolean | by kind | whether the value is sensitive |

Declaring a param schema MUST throw if any param name is not a valid name, any definition is not a map, a definition carries any other key, a key's value is not of the type in the table, `kind` is not a valid [kind](#kind-arguments), or a non-empty `default_value` is one its kind's [parser](#kind-parsers) rejects.
An empty `default_value` MUST be treated as no `default_value`.
Declaring a param schema MUST throw if a `default_value` is a boolean and the kind is not `boolean`, or is an integer and the kind is neither `integer` nor `quantity`.
A boolean or integer `default_value` MUST be taken as its [canonical form](#kind-parsers).

`secret` defaults to `true` for the `password` and `weak-password` kinds and to `false` for every other kind.
Declaring a param schema MUST throw if a param whose kind is `boolean` or `choice` has `secret` set to `true`, as such a param cannot be [secret](parameter.md#secret-parameters).
The runtime MUST handle a param whose `secret` is `true` as it handles a [secret parameter](parameter.md#secret-parameters).

The runtime MUST validate an invocation's params against the schema, in this order, before the closure runs:

1. A boolean supplied for a `boolean` param, or an integer supplied for an `integer` or `quantity` param, MUST be taken as its canonical form.
2. Any other declared param supplied as anything other than a string MUST be rejected.
3. A declared param that is absent or supplied as the empty string MUST take its `default_value`, if it has one.
4. A required param that is still absent or empty MUST be rejected.
5. A declared param that is still absent or empty MUST NOT be validated against its kind.
6. Every other declared param's value MUST then be validated as its kind specifies.

A param the schema does not declare MUST be passed to the closure as supplied.

A presentation layer MUST pre-fill a param's input with its `default_value`.
A presentation layer MUST NOT let an operator submit a required param empty.

### Param kinds

| kind | arguments | value | parser result |
| --- | --- | --- | --- |
| `text` | none | a single line of text | the value |
| `multiline` | none | text that may span several lines | the value |
| `email` | none | an email address | a map of `local`, the part before the first `@`, and `domain`, the part after it |
| `password` | none | a strong password | the value |
| `weak-password` | none | a password of any strength | the value |
| `random` | none | free-form text, usually a generated random value | the value |
| `version` | none | a version of any scheme | a `Version`, as [generic versions](version.md#generic-versions) specify |
| `version-semver` | none | a [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html) version | a `SemVer`, as [SemVer versions](version.md#semver-versions) specify |
| `purl` | none | a [Package URL](https://ecma-tc54.github.io/ECMA-427/) | a map of its components, as [Package URLs](purl.md#components) specify |
| `url` | none | a URL | a map of `scheme`, `host`, `port`, `path`, `query`, `fragment`, `username` and `password`, as [URLs](url.md#urls) specify |
| `url-origin` | none | an `http` or `https` URL naming a host and optionally a port | a map of `scheme`, `host` and `port`, as [URL origins](url.md#url-origins) specify |
| `hostname` | none | a hostname an ingress can be declared on | the hostname in lower case, as [hostnames](url.md#hostnames) specify |
| `ip-address` | `family`, optional | an IPv4 or IPv6 address, of `family` where one is given | an `IpAddress`, as [address values](ip.md#address-values) specify |
| `ip-subnet` | `family`, optional | an IPv4 or IPv6 subnet, of `family` where one is given | an `IpSubnet`, as [subnet values](ip.md#subnet-values) specify |
| `ip-range` | `family`, optional | a range of IPv4 or IPv6 addresses, of `family` where one is given | an `IpRange`, as [range values](ip.md#range-values) specify |
| `boolean` | none | `true` or `false` | the value as a boolean |
| `integer` | `range`, optional | a whole number, within `range` where one is given | the value as an integer |
| `quantity` | `range`, optional | a [quantity](#quantities), within `range` where one is given | its amount as an integer |
| `choice` | `options`, required | one of `options` | the value |
| `volume` | none | the name of a site volume, as [volume params](#volume-params) specify | the value |

### Kind arguments

A kind is given either as a kind name from the table above, or as a kind map whose `name` key holds a kind name and whose other keys are that kind's arguments.
A kind name MUST be equivalent to a kind map holding only `name`.

A kind is valid only if it names a kind, carries no key that is neither `name` nor one of that kind's arguments, carries every argument the table marks required, and satisfies the rules below for each argument it carries.

An `integer` or `quantity` kind's `range` is valid only if it is an exclusive or inclusive integer range containing at least one integer.

> [!NOTE]
> BSL reads a range with no start, such as `..10`, as one starting at 0, so a kind given only an upper bound accepts no negative value.
> A range with no end runs to the largest integer where it is inclusive, such as `1..=`, and to one short of it where it is exclusive, such as `1..`.

The `family` of an `ip-address`, `ip-subnet` or `ip-range` kind is valid only as the [family argument](ip.md#family-argument) specifies.

A `choice` kind's `options` is valid only if it is a non-empty array of non-empty strings, no two of which are equal, each of which `std::parse_text` accepts.

### Kind parsers

Every kind MUST have a parser, the [helper](std.md#helper) `std::parse_<kind>`, where `<kind>` is the kind's name with each `-` written as `_`.
A parser takes the value as its first argument, followed by the kind's argument where the kind has one, returns the parser result the kind table gives, and raises a script error for a value the kind does not accept.
`std::parse_integer` and `std::parse_quantity` MUST each accept being called with or without `range`.
`std::parse_choice` MUST throw when called without `options`.
A parser MUST throw when given an argument that a kind map carrying it would make invalid.
The runtime MUST reject a value that its kind's parser rejects, with the parser's error as the reason.
The runtime MUST accept a value that its kind's parser accepts, except where [volume params](#volume-params) add a check.

> [!NOTE]
> A script calls a parser to apply the runtime's own check to a value the runtime has not checked, such as one it computed or one it read with `param.value()`.

Every parser but `std::parse_multiline` MUST reject a value containing any character whose Unicode general category is Control (Cc), Line Separator (Zl) or Paragraph Separator (Zp).
`std::parse_text`, `std::parse_weak_password` and `std::parse_random` MUST accept every other value.
`std::parse_multiline` MUST accept any string.

> [!NOTE]
> Every kind but `multiline` is entered as a single line, so a line break or other control character in one is invisible where it is shown and is most often a paste or injection the operator did not mean.

`std::parse_email` MUST also reject a value that contains no `@`, that has nothing before its first `@`, or whose part after the first `@` contains no `.` or begins or ends with `.`.
`std::parse_email` SHOULD NOT reject any other value.

`std::parse_password` MUST also reject a value it judges weak.
It SHOULD judge a value weak when [zxcvbn](https://www.usenix.org/conference/usenixsecurity16/technical-sessions/presentation/wheeler) scores it below 3.

`std::parse_volume` MUST reject a value that breaks the [name rules](overview.md#names).

`std::parse_boolean` MUST accept exactly `true` and `false`.

`std::parse_integer` MUST accept exactly the decimal forms of signed 64-bit integers within `range`, or of any signed 64-bit integer where it is given no `range`.
The decimal form of an integer is its digits with no leading zero, preceded by `-` where the integer is negative, with zero written `0`.

The canonical form of a boolean is `true` or `false`, and the canonical form of an integer is its decimal form.
Where `std::parse_integer` rejects an integer for falling outside `range`, its error SHOULD state the range.

`std::parse_choice` MUST accept exactly the strings equal to one of `options`, compared case-sensitively.

> [!NOTE]
> Each `boolean` and `integer` value has a single accepted spelling, so a value round-trips through an operator interface's control unchanged.
> A client or script that takes looser input normalises it before supplying it.

### Quantities

A quantity is an optional sign, `+` or `-`, followed by a number and an optional suffix, with nothing between, before or after them.
A number is one or more decimal digits, with at most one `.` before, between or after them.
Suffixes are case-sensitive, and each multiplies the number as the table gives.

| suffix | multiplier |
| --- | --- |
| none | 1 |
| `k`, `M`, `G`, `T`, `P`, `E` | 10^3, 10^6, 10^9, 10^12, 10^15 and 10^18 |
| `Ki`, `Mi`, `Gi`, `Ti`, `Pi`, `Ei` | 2^10, 2^20, 2^30, 2^40, 2^50 and 2^60 |
| `e` or `E`, followed by an optional sign and one or more decimal digits | 10 to the power of the signed digits |

An `E` followed by digits is an exponent, and an `E` on its own multiplies by 10^18.
The amount of a quantity is its number multiplied by its suffix's multiplier, negated where its sign is `-`.

`std::parse_quantity` MUST accept exactly the quantities whose amount is a whole number within the signed 64-bit range, and within `range` where it is given one, and MUST return that amount.
It MUST compute the amount exactly, and MUST reject a quantity whose amount is not whole or falls outside the signed 64-bit range, rather than round or cap the amount.
Where it rejects a quantity for falling outside `range`, its error SHOULD state the range's bounds as quantities.
Where it rejects a value that would be a quantity but for its suffix, its error SHOULD suggest the suffix most likely meant, such as `Mi` for `m`, `mi` or `MiB`, and `M` for `MB`.

> [!NOTE]
> The syntax is the [Kubernetes resource quantity](https://kubernetes.io/docs/reference/kubernetes-api/common-definitions/quantity/), without the suffixes `n`, `u` and `m` for fractions of a unit.
> Rejecting a fractional amount, rather than rounding it, makes a value such as `512m`, written where `m` conventionally means mebibytes, an error instead of a single byte.

> [!NOTE]
> A quantity carries no unit, so a parameter's description tells the operator whether it counts bytes, bits or something else.

### Presentation of kinds

A presentation layer SHOULD offer a single-line input for a `text`, `version`, `version-semver`, `purl`, `url`, `url-origin`, `hostname`, `ip-address`, `ip-subnet` or `ip-range` param, and a multi-line input for a `multiline` param.
A presentation layer SHOULD offer a toggle for a `boolean` param, a numeric input limited to `range` for an `integer` param, and a selection among `options`, in the order the script declared them, for a `choice` param.
A presentation layer SHOULD offer a single-line input for a `quantity` param, and SHOULD show the amount the value resolves to while it is entered.
A presentation layer MUST let an operator leave a param that is not required without a value, whatever its kind.
A presentation layer MAY warn about an `email` value that is valid but unlikely to be meant, such as `" "@example.org`.
A presentation layer SHOULD indicate the strength of a `weak-password` value as it is entered, and MUST NOT refuse a `weak-password` value for its strength.
A presentation layer SHOULD offer a generator of strong passwords for `password` and `weak-password` params.

A presentation layer SHOULD offer a one-click generator for a `random` param, beside manual entry.
That generator's default output MUST be 32 random bytes written as 64 lowercase hexadecimal digits.

> [!NOTE]
> The `random` kind does not imply `secret`, so a script keeping sensitive material in a `random` param sets `secret: true`.

### Volume params

The `volume` kind MUST be accepted in the param schemas of actions and shells.
A [parameter's kind](parameter.md#schema) does not accept it.

The runtime MUST reject an invocation whose `volume` param names a site volume that does not exist.
When it dispatches the invocation, the runtime MUST bind the named site volume for the operation, as [operation volume params](../runtime/operations.md#operation-scoped-volumes) specify.
In the param map the closure receives, the runtime MUST replace the param's own key with the key `<name>_volume`, whose value names that binding.
If the binding cannot be made at dispatch, the runtime MUST fail the invocation without running the closure.
A closure obtains the bound volume with `app.external_volume(param["<name>_volume"])`, as [operation-scoped external volumes](volume.md#external-volumes) specify.

> [!NOTE]
> A volume binding lasts for one operation, while a parameter's value outlives it.
> A script needing a permanent volume declares an external volume for the operator to map.

## Shells

A script declares a shell with `app.on_shell(name: string, fn: closure)` or `app.on_shell(name: string, fn: closure, options: map)`, which return no value.
A `name` that is not a valid name MUST throw.
Declaring a shell under a name a shell of the app is already declared under MUST throw.
`app.on_shell` MUST throw when called in the action context.
The `options` map takes the same keys as for `app.on_action`, and MUST throw on the same conditions.

Shells and actions MUST have separate namespaces.
A shell and an action MAY share a name, and each MUST remain reachable under it as its own kind.

The runtime MUST run a shell's closure only when an operator opens a session of the shell.
The runtime MUST call `fn` with three arguments: a runtime handle, a [shell control](#shell-control), then the param map.
A closure declaring any other number of parameters MUST fail when the runtime calls it.
The runtime MUST validate a shell's params as it validates an action's.
How a shell session relates to lifecycle operations is specified by [shell sessions](../runtime/operations.md#shell-sessions).

### Shell control

A shell control is a `ShellControl` value with two methods.

`shell.attach(job: Job)` starts the [job](job.md#instances), connects the operator's input and output to it, and returns a [`ShellExit`](#shellexit) once the attachment ends.
`shell.attach` given anything other than a Job MUST throw.
A second `shell.attach` within one run of the closure MUST throw.

The attachment MUST end when the job's process exits, when the operator closes the session, or when the connection is lost.
When it ends other than by the job's process exiting, the runtime MUST stop the job.

The closure MAY go on using the runtime handle after `shell.attach` returns, without the operator's input and output.
The session MUST last until the closure returns.
If the closure returns without having called `shell.attach`, or fails, the runtime MUST end the session with an error, reported to the operator while the connection holds.

`shell.error(message: string)` ends the session with an error carrying `message`, and MUST throw to end the closure.

> [!NOTE]
> A shell is a scripted procedure with an interactive step.
> The closure can prepare for the session before attaching, and act on the session's outcome once it has ended.

### ShellExit

A `ShellExit` is an opaque value standing for how a shell's attachment ended.

| method | returns |
| --- | --- |
| `exited()` | `true` if the job's process exited, `false` if the attachment ended first |
| `exit_code()` | the job's exit status, as [`Executed`](rt.md#executed) defines it, or the unit value `()` if the process did not exit |
| `success()` | `true` if the process exited with status 0, `false` otherwise |
| `ensure_success()` | no value |

`ensure_success()` MUST throw unless the process exited with status 0.
Its error MUST carry the exit status, or state that the process did not exit.

## Scheduled actions

`action.on_schedule(expr: string)` attaches a schedule to the action, and returns the same `Action` handle.
An action MAY carry several schedules.
An `expr` that is not a valid [schedule expression](#schedule-expressions) MUST throw.
`on_schedule` MUST throw when called in the action context.

Each time a schedule comes due, the runtime MUST run its action as [scheduled actions](../runtime/scheduled-actions.md) specify.
The runtime MUST validate a scheduled run against the action's [param schema](#param-schemas) as an invocation that supplies no params, so that each declared param takes its `default_value`.
If that validation fails, the runtime MUST NOT run the closure, and MUST file a [`schedule_params_invalid`](../runtime/faults.md#app-faults) fault.

### Schedule expressions

A schedule expression is five fields separated by whitespace, optionally followed by a time zone.
The fields are, in order, minute, hour, day of month, month and day of week.
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
