---
id: GATE
---

# Gates and start order

Applies to the gates a BSL script declares to bring its [app](../overview.md#app) to a state its [resource graph](../overview.md#resource-graph) cannot express, and to the order in which the app's workloads and gates start.

## Terminology

### Gate

A named step an app declares, which the [runtime](../overview.md#runtime) keeps satisfied by running the gate's body whenever the gate's key moves from the key of its last success.

### Key

The value a gate's definition gives it at a [generation](../overview.md#generation).

### Last success

The most recent run of a gate that the runtime recorded as successful, together with the key and the generation it ran for.

### Satisfied

A gate is satisfied when its last success was for its current key and, where it declares a check, the check passes.

### Target

A gate or workload that another gate or workload is ordered after with `after`.

### Dependant

A gate or workload ordered after a target.

## Declaration

`app.gate(name: string)` MUST declare the gate named `name` and return a `Gate` [builder](overview.md#builders).
A `name` that is not a valid [name](overview.md#names) MUST throw.
Calls with the same name MUST return the same gate.
`app.gate` MUST throw when called outside the [static context](app.md#static-context).

A gate is not a [resource](../overview.md#resource).
A [runtime handle](rt.md#runtime-handle) method given a gate as a [selection](rt.md#selections) MUST throw.

Every builder method of a `Gate` MUST throw when called outside the static context.

## Keys

`gate.key(value)` MUST set the gate's key to `value`, and returns the `Gate`.
Calling it again MUST replace the key it set before.
A gate on which `key` is never called has the unit value `()` as its key.

`value` MUST be representable in JSON: a unit, a boolean, an integer, a finite floating-point number, a string, or an array or map of such values.
Any other value MUST make `key` throw.

Two keys are the same when they are the same JSON value, with the members of a map compared regardless of their order.

> [!NOTE]
> A key is evaluated with the app's definition and parameter values, so a gate keyed on `version.value()` moves whether the version moved through a stored value, a default, a value derived from another parameter, or a new definition.
> A gate whose key never changes runs once, and again only when its check fails.

## Body

`gate.run(fn: closure)` MUST set `fn` as the gate's body, and returns the `Gate`.
Calling `run` a second time for the same gate MUST throw.

A script MAY pass a closure taking up to two parameters.
The runtime MUST call `fn` with as many of these arguments as it takes, in this order: a runtime handle, then `last`.
A closure declaring more than two parameters MUST fail when the runtime calls it.

`last` MUST be an `App` reflecting the generation of the gate's last success: that generation's definition, evaluated with that generation's parameter values.
Where the gate has no last success, `last` MUST be the unit value `()`.
The static resources and actions reachable through `last` MUST reflect their definitions at that generation, and `last.file` and `last.dir` read that generation's bundle, as [bundle files](bundle.md#bundle-files) specify.

`last` is a read-only view.
Calling a resource method on `last` MUST NOT affect the app.

> [!NOTE]
> `last` is the state the gate last brought the app to, not the generation before the current one.
> A version that moved twice while a migration kept failing is still upgraded from the release the database was last migrated to.

A gate without a body succeeds at once each time it runs, so that it is satisfied whenever its check passes.

How and when the runtime runs a gate is specified by [gate runs](../runtime/gates.md).

## Check

`gate.check(job: Job)` and `gate.check(job: Job, options: map)` MUST set `job` as the gate's check, and return the `Gate`.
Calling `check` again MUST replace the check it set before.
`job` MUST be a named [Job](job.md) of the app, and anything else MUST make `check` throw.

The `options` map MAY carry `interval`, a number of seconds.
An `options` map carrying any other key, or an `interval` below 1, MUST throw.

A check passes when an instance of its Job runs to completion and exits with status 0.
It fails when the instance exits with any other status, or is stopped at its [deadline](job.md#deadline).

A Job that is a gate's check MUST NOT be part of the app's steady-state [desired state](../runtime/reconciliation.md#desired-state), and the runtime MUST start it only to check the gate, as [gate checks](../runtime/gates.md#checks) specify.
A Job MUST NOT be the check of more than one gate, and declaring it as a second gate's check MUST throw.

> [!NOTE]
> A check lets a gate see a change made outside Seedling, such as a database restored from a backup taken before a migration.
> A gate with a check and no body waits on a condition alone, such as a database the operator provides becoming reachable.

## Exclusive selection

`gate.exclusive(selection)` MUST set the gate's exclusive selection to the Deployments and Jobs of `selection`, and returns the `Gate`.
`selection` is a selection as runtime handle methods take one, holding only the app's static resources.
Calling `exclusive` again MUST replace the selection it set before.

The runtime stops a gate's exclusive selection before running the gate, and holds it stopped until the gate is satisfied, as [gate holds](../runtime/reconciliation.md#gate-holds) specify.

## Start order

`after(target)` MUST order the gate or workload it is called on after `target`, and returns the builder it was called on.
`after` is a builder method of `Gate` and of every type that implements [`Pod`](pod.md).
`target` MUST be a gate, a named Deployment, a named Job, or an array of these, and anything else MUST make `after` throw.
Each call MUST add its targets to those already given.

A target MUST be of the same app.
A script MUST NOT order a gate or workload after itself, whether directly or through other targets, and the evaluation of a script whose `after` calls form a cycle MUST fail.

A target is ready when:

- for a gate, it is satisfied;
- for a Deployment, at least one of its instances is [Ready](../overview.md#lifecycle-state);
- for a Job, its instance at the app's current generation has run to completion and exited with status 0.

The runtime MUST NOT start an instance of a dependant workload, whether its first start, a replacement or a recovery restart, until each of its targets is ready.
The runtime MUST NOT run a dependant gate until each of its targets is ready.
The runtime MUST NOT stop a running instance because a target is not ready.

A Deployment that is [stopped](../interface/app-control.md#resource-stop), or held at an effective scale of zero, is not ready, and its dependants wait on it.

A dependant of a gate takes the gate's key as part of its configuration, as [stale instances](../runtime/update-strategies.md#stale-instances) specify.

> [!NOTE]
> A running Deployment ordered after a gate is replaced by its update strategy once the gate is satisfied for a new key.
> A rolling Deployment keeps serving on its previous instances until then, while one the gate's exclusive selection holds is down for the run.
