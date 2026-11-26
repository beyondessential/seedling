---
id: LIM
---

# Script engine limits

Applies to the bounds the [runtime](../overview.md#runtime) places on each evaluation of a BSL script, and to the breakers that stop it repeating an evaluation that ran out of time.

## Terminology

### Breaker

A record, kept for an app's definition or one of its action closures, of the definition and inputs with which an evaluation of it last exceeded the evaluation time limit, under which the runtime does not evaluate it again with them.

### Tracked evaluation

An evaluation whose outcome trips or resets a [breaker](#breaker), as [breakers](#breakers) specifies.

## Limits

The runtime MUST bound the resources a single evaluation of a BSL script can consume, with each of the following limits.

| limit | unit | default | daemon option |
| --- | --- | --- | --- |
| operations per evaluation | operations | 100 000 | `--script-max-operations` |
| function call nesting depth | calls | 64 | `--script-max-call-depth` |
| expression nesting depth | levels | 64 | `--script-max-expr-depth` |
| string length | bytes | 1 048 576 (1 MiB) | `--script-max-string-size` |
| array length | elements, or bytes of a Blob | 10 000 | `--script-max-array-size` |
| object map size | entries | 10 000 | `--script-max-map-size` |
| derived values | calls to a [derivation helper](../language/random.md#derivation-helper), and helper results [withheld as derived values](secrets.md#redaction) | 1 000 | `--script-max-derived-values` |
| regex compiled size | bytes of [patterns compiled](../language/regex.md#compiled-size) | 33 554 432 (32 MiB) | `--script-max-regex-compiled-size` |
| password hashes | calls to a [password hash helper](../language/random.md#password-hash-helper) | 16 | `--script-max-password-hashes` |
| evaluation time | whole seconds of [evaluation time](#measuring-evaluation-time) | 10 | `--script-max-evaluation-time` |

An evaluation that exceeds any limit MUST fail with an error.
A pattern that would exceed the regex compiled size limit MUST instead raise a script error, as [compiled size](../language/regex.md#compiled-size) specifies.

> [!NOTE]
> A password hash helper is a derivation helper, so each call to one counts toward both the derived values and the password hashes limits.

The expression nesting limit MUST apply both at the top level of a script and within function bodies.

Functions in a script's scope MAY NOT pause evaluation for any span of time, other than using the methods of a [runtime handle](../language/rt.md#runtime-handle).

An operator MAY override each limit with its daemon option when the daemon starts.
Setting the operations, expression nesting, string, array, object map, derived values, regex compiled size, password hashes or evaluation time limit to 0 MUST remove that limit.
The daemon MUST refuse to start with a function call nesting depth of 0.

> [!NOTE]
> A depth of 0 would allow no function calls at all, and an unbounded depth would let a script overflow the runtime's own stack.

> [!NOTE]
> The runtime keeps what an evaluation derived, and the results it withholds as derived values, for as long as it withholds them from operators, so the derived values limit bounds what one definition can make it hold.

## Measuring evaluation time

An evaluation's evaluation time is the processor time the runtime spends evaluating its script.
Time the evaluation spends waiting, such as in a method of a [runtime handle](../language/rt.md#runtime-handle), at a [barrier](../language/rt.md#barrier), or while a [shell](../language/action.md#shells) is attached, is not part of it.

Each of these is one evaluation:

- an evaluation of an app's definition;
- a pass of a lifecycle operation over its action closure, together with the evaluation of the definition the pass begins with, as [replay](suspension.md#replay) specifies;
- a run of a shell's closure in a shell session;
- a call of a parameter's [validator](../language/parameter.md#validators).

The runtime MUST stop an evaluation whose evaluation time exceeds the evaluation time limit, at the next of the operations the operations per evaluation limit counts.
An evaluation that returns having exceeded the limit MUST fail all the same.
An evaluation stopped at the limit fails to evaluate, and a request that reports such failures in its result rather than refusing, such as [change planning](../interface/params.md#change-planning) or a [template preview](../interface/templates.md#preview), MUST report it there.

> [!NOTE]
> A single call into a function the runtime provides, such as one regex search, runs to its end before the evaluation can be stopped.

> [!NOTE]
> Processor time leaves out the time a script waits on the host or an operator, such as for a command to finish, and the time other work on a busy host takes from it, neither of which measures the script itself.

## Breakers

The runtime MUST keep a breaker for an app's definition and for each [action closure](../overview.md#action-closure) the definition declares: the closure of each action and of each shell, and each gate's body.

A tracked evaluation is one of these, at the app's current [generation](../overview.md#generation):

- an evaluation of the app's definition with its stored parameter values;
- a pass of a lifecycle operation, a shell session's run of its shell's closure, or a gate run's run of the gate's body.

Every other evaluation, such as of a proposed definition or proposed values, a change plan, a template preview, an image discovery probe, a validator, or a pass of an operation at an earlier generation, MUST neither trip nor reset a breaker, and is stopped at the evaluation time limit all the same.

> [!NOTE]
> An evaluation that is not tracked either answers a request whose caller sees the failure at once, or fails the operation it belongs to, so its cost is not paid again until someone acts.

The definition of an evaluation is the definition bundle of the generation it evaluates.
Its inputs are the app's [host constants](../language/constants.md#host-constants), its [app seed](secrets.md#app-seed), the running Seedling version and the evaluation time limit in force, together with the inputs below.

| thing | inputs |
| --- | --- |
| definition | the app's stored parameter values |
| an action's closure | the app's stored parameter values, and the params of the invocation once validated and filled in from defaults, each in its [canonical form](../language/action.md#kind-parsers), leaving out [volume params](../language/action.md#volume-params) and [reserved keys](../language/action.md#param-delivery) |
| a shell's closure | the app's stored parameter values, and the params of the session, taken as for an action |
| a gate's body | the app's stored parameter values, and the definition and parameter values of the gate's last success that the body's `last` reflects, if any |

> [!NOTE]
> The running Seedling version is an input because a release can change how long the same script takes, such as through a different regex engine, and the limit in force is one so that an operator who raises or removes it can run again what it stopped.

> [!NOTE]
> The world an action closure observes through its runtime handle, and its [operation seed](secrets.md#operation-seed), are not inputs, so a change on the host does not reset a breaker; an operator who has fixed the host clears the fault to try again.

The runtime MUST record a breaker's definition and inputs as a digest computed under the host's [secret key](secrets.md#secret-key), and MUST NOT store them, nor anything from which they could be recovered without the secret key.

When a tracked evaluation fails at the evaluation time limit, the runtime MUST trip the breaker of the thing it evaluated, recording the definition and the inputs it was evaluated with.
A pass of a lifecycle operation evaluates the action the operation runs, including any closure the pass reaches through [`invoke`](../language/action.md#action-invocation).

While a breaker is tripped, a tracked evaluation of its thing with the recorded definition and inputs MUST fail at once, without evaluating any part of the script.
A tracked evaluation of its thing with a different definition or different inputs MUST evaluate as usual.
If it finishes within the limit, the runtime MUST reset the breaker.
If it fails at the limit, the runtime MUST record its definition and inputs on the breaker in place of the earlier ones, and the breaker stays tripped.
Where an evaluation the runtime then adopts as the app's own, as an [update](../interface/apps.md#update) does, finishes within the limit, the runtime MUST reset the definition's breaker whatever it was tripped for.

Runs of a tripped action closure are refused or held back before they start, as [action invocation](../interface/actions.md#invoking-an-action), [opening an app shell](../interface/shells.md#opening-an-app-shell), [`invoke`](../language/action.md#action-invocation), [scheduled actions](../language/action.md#scheduled-actions) and [gate preparation](gates.md#preparation) specify.

A breaker is tripped exactly while its [`evaluation_runaway`](faults.md#app-faults) fault is active.
The runtime MUST trip a breaker and file its fault as one durable change, and MUST reset a breaker and clear its fault as one durable change, so that neither outlives the other and a runtime restart resets neither.

When an operator clears an `evaluation_runaway` fault, the runtime MUST reset its breaker, and where the breaker is the definition's, MUST then evaluate the definition again.
When the app's definition no longer declares an action closure, the runtime MUST reset its breaker.
When an app is deregistered, the runtime MUST discard every breaker of the app.

> [!NOTE]
> Clearing an app's faults clears every `evaluation_runaway` fault of the app with them, and so resets every breaker of the app.

If the runtime cannot read whether a breaker is tripped, it MUST NOT make a tracked evaluation of the breaker's thing, MUST hold a [`breaker_state_unreadable`](faults.md#app-faults) fault against the thing, and MUST keep trying to read it.

> [!NOTE]
> A breaker bounds the repeated cost of an accident, such as a pattern that is slow on a parameter's value; it is no defence against a script written to exhaust the host.
> Tripping on the first runaway means its cost is paid once, rather than on every evaluation that follows with the same inputs.
