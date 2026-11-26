---
id: LIM
---

# Script engine limits

Applies to the bounds the [runtime](../overview.md#runtime) places on each evaluation of a BSL script.

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
| evaluation time | whole seconds of [evaluation time](#evaluation-time) | 10 | `--script-max-evaluation-time` |

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

## Evaluation time

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

The runtime MUST keep a breaker for each thing of an app it evaluates: the app's definition, each action handler, each shell handler, each gate body, and each parameter's validator.

A thing's inputs are the app's [host constants](../language/constants.md#host-constants), its [app seed](secrets.md#app-seed) and the running Seedling version, together with the inputs below.

| thing | inputs |
| --- | --- |
| definition | the app's parameter values |
| action handler | the app's parameter values, and the params of the invocation once defaults are filled in |
| shell handler | the app's parameter values, and the params of the session once defaults are filled in |
| gate body | the app's parameter values, and the `last` the body is passed |
| parameter validator | the proposed parameter values |

> [!NOTE]
> The running Seedling version is an input because a release can change how long the same script takes, such as through a different regex engine.

When an evaluation fails at the evaluation time limit, the runtime MUST trip the breaker of the thing evaluated, recording the definition and the inputs it was evaluated with.
While a breaker is tripped, an evaluation of its thing with the recorded definition and inputs MUST fail at once, without evaluating any part of the script.
An evaluation of its thing with a different definition or different inputs MUST reset the breaker and evaluate as usual.
A request that would evaluate an action or shell handler whose breaker is tripped for its inputs MUST be refused before any lifecycle operation or shell session starts, as [action invocation](../interface/actions.md#invoking-an-action), [opening an app shell](../interface/shells.md#opening-an-app-shell), [`invoke`](../language/action.md#action-invocation) and [scheduled actions](../language/action.md#scheduled-actions) specify.

The runtime MUST reset a breaker when an operator clears its [`evaluation_runaway`](faults.md#app-faults) fault, and MUST discard every breaker of an app when the app is deregistered.
The runtime MUST persist a tripped breaker, so that a runtime restart does not reset it.

> [!NOTE]
> Operators write the scripts a host evaluates, so the breaker is there to catch an accident, such as a pattern that is slow on a parameter's value, rather than to defend against a hostile script.
> Tripping on the first runaway means its cost is paid once, rather than on every evaluation that follows with the same inputs.
