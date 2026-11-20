---
id: RCN
---

# Reconciliation

Applies to the loop by which the [runtime](../overview.md#runtime) brings a [host](../overview.md#host) into line with the apps installed on it.

## Terminology

### Steady state

The condition of an app while no [lifecycle operation](../overview.md#lifecycle-operation) other than a [gate run](gates.md) is in progress for it.

### Autonomous operation

An action the runtime performs on the host, without direction from an [action closure](operations.md#operation), to bring a resource instance towards its desired state.

### Absolute state

Host state that the runtime rebuilds in full on each iteration and applies wholesale, so that applying it removes from the host anything absent from it.

> [!NOTE]
> Proxy configuration, routes and packet-filtering rules are absolute state.

## Desired state

An app's [desired state](../overview.md#desired-state) is derived from its [resource graph](../overview.md#resource-graph) and its [gates](../language/gate.md), and, while a lifecycle operation is in progress for it, from that operation's progress.

In steady state, the desired state of an app that is `Installed` MUST be its whole resource graph, as [start order](../language/gate.md#start-order) and [gate holds](#gate-holds) qualify it.
In steady state, the desired state of an app that is `Uninstalling` MUST hold each of the app's recorded instances `Unscheduled`.
An app that is `NotInstalled` MUST have no desired state.
The runtime MUST maintain an app's steady-state desired state without direction from an operator, as [autonomous reconciliation](autonomous.md) specifies.

While a lifecycle operation other than a [gate run](gates.md) is in progress for an app, the app's desired state MUST consist of the resources the operation's [action closure](operations.md#operation) has [started](../language/rt.md#starting-resources) or [stopped](../language/rt.md#stopping-resources), each in the state the closure last put it in.
The runtime MUST act only on resources in the desired state, and on the Jobs of the [gate checks](gates.md#checks) it runs.
A resource outside the desired state MUST be left as it is until the operation names it or completes.
Its running instances MUST keep the specification they were last started with, and MUST NOT be rotated.

While a gate run is in progress for an app, the app's desired state MUST be as in steady state, with each resource the gate's body has started or stopped in the state the body last put it in, including the gate's own exclusive selection and the workloads ordered after it.

> [!NOTE]
> A gate run holds only what the gate holds, so the rest of the app goes on being maintained while a migration runs, including a supervisor that answers requests meanwhile.

While a [shell session](../interface/shells.md#shell-session) on an app is open, the app's desired state MUST also hold the [dynamic resources](../language/app.md#dynamic-resource-lifetime) the session's closure has started, each in the state the closure last put it in, whatever else the desired state consists of.
The runtime MUST keep each session's resources apart from those of every other session and of any lifecycle operation.

### Gate holds

Before a [gate run](gates.md#preparation) stops the gate's exclusive selection, the runtime MUST record durably that the gate holds the selection, and MUST NOT stop it if it cannot record that.
From then until the gate is next satisfied, the runtime MUST hold every resource of the selection `Unscheduled`, through failed and cancelled runs and across restarts of the runtime, whatever the resource's effective scale and except as the gate's own body directs it.
A closure other than the gate's body cannot start a held resource, as [resources a gate holds](../language/rt.md#resources-a-gate-holds) specifies.
The runtime MUST clear the record once the gate is satisfied, and MUST NOT start the selection before it has cleared it.

The hold covers the gate's exclusive selection as the app's current definition gives it, and the runtime MUST keep the resources it covers in its record.
A resource that a change adds to the selection while the gate holds it MUST be stopped and held with the rest.
A resource that a change removes from the selection is released once the runtime has removed it from the record.
When the definition no longer declares the gate, the runtime MUST clear the record, and every resource the hold covered is released once it has.
While the app's definition fails to evaluate, the runtime MUST go on holding the resources the record names.

If the runtime cannot read whether a gate holds its exclusive selection, it MUST NOT start any resource of the selection.

The Job of a gate's [check](../language/gate.md#check) MUST be held out of the steady-state desired state, except while the runtime runs it to [check the gate](gates.md#checks).

> [!NOTE]
> A key change makes the gate's dependants stale, but their replacements wait for the gate, so the instances already running keep running until the run stops what it holds.

## Reconciliation loop

The runtime MUST run a reconciliation loop for as long as it runs.
Each iteration MUST perform these steps, in order:

1. Observe the host, as [observation](observation.md#observations) specifies.
2. Record the observations in the [world observation history](history.md#world-observation-history).
3. Derive each resource instance's [lifecycle state](../overview.md#lifecycle-state) from its observation history.
4. Where a lifecycle operation is suspended on a [barrier](suspension.md#barriers) whose condition is now satisfied, schedule the operation's action closure to resume, without waiting for it to run.
5. Compute the [autonomous operations](#autonomous-operation) needed to bring the derived lifecycle states to the desired state.
6. For each such autonomous operation, decide from the [autonomous operations log](history.md#autonomous-operations-log) whether to perform it, back off, or file a [fault](../overview.md#fault), and record each autonomous operation it decides to perform, with its [operation provenance](history.md#operation-provenance), in the autonomous operations log.
7. Perform the autonomous operations recorded in step 6.
8. Record the outcome of each autonomous operation performed on its entry in the autonomous operations log.

The runtime MUST NOT perform an autonomous operation whose entry in the autonomous operations log it could not record.

Given a desired state that does not change and a host that functions, successive iterations MUST converge on the desired state.

Every autonomous operation MUST be idempotent.
Performing an autonomous operation whose effect is already in place MUST NOT produce an error or repeat a side effect.

An autonomous operation MUST NOT block the loop for an unbounded or long time.
An autonomous operation MUST NOT block the loop to wait on an external condition, such as a process exiting.
It MUST instead return control to the loop, and a later iteration MUST re-evaluate the condition.

## Applying absolute state

An app contributes to absolute state while it is `Installed` or `Uninstalling`.
The runtime MUST withdraw a contribution only when the resources making it leave the app's desired state, or when the app moves to `NotInstalled`.
The runtime MUST NOT withdraw a contribution because it failed to compute it.

If an iteration cannot compute the contribution of every contributing app to an item of absolute state, the runtime MUST NOT apply that item in that iteration.
The item last applied then stays in force until an iteration computes every contribution.

An iteration that withholds an apply MUST NOT file or clear the fault for that item.
It MUST NOT record any resource as ready on the strength of the withheld apply.

An iteration that computes every contribution MUST apply the result, including a result that is empty.

> [!NOTE]
> An absent contribution and a deleted one look the same once applied.
> Removing the last ingress of the last app leaves the proxy serving nothing, while failing to compute that app's ingresses leaves the proxy as it was.

The runtime MUST tear down infrastructure shared between apps only when no app is `Installed` or `Uninstalling`.
