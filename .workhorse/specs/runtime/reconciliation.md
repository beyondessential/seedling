---
id: RCN
---

# Reconciliation

Applies to the loop by which the [runtime](../overview.md#runtime) brings a [host](../overview.md#host) into line with the apps installed on it.

## Terminology

### Steady state

The condition of an app while no [lifecycle operation](../overview.md#lifecycle-operation) is in progress for it.

### Absolute state

Host state that the runtime rebuilds in full on each iteration and applies wholesale, so that applying it removes from the host anything absent from it.

> [!NOTE]
> Proxy configuration, routes and packet-filtering rules are absolute state.

## Desired state

An app's [desired state](../overview.md#desired-state) is derived from its [resource graph](../overview.md#resource-graph) and, while a lifecycle operation is in progress for it, from that operation's progress.

In steady state, an app's desired state MUST be its whole resource graph.
The runtime MUST maintain that desired state without direction from an operator, as [autonomous reconciliation](autonomous.md) specifies.

While a lifecycle operation is in progress for an app, the app's desired state MUST consist of the resources the operation's [action closure](operations.md#operation) has [started](../language/rt.md#starting-resources) or [stopped](../language/rt.md#stopping-resources), each in the state the closure last put it in.
The runtime MUST act only on resources in the desired state.
A resource outside the desired state MUST be left as it is until the operation names it or completes.
Its running instances MUST keep the specification they were last started with, and MUST NOT be rotated.

An install operation is a lifecycle operation, and the runtime MUST actuate the resources its closure places in the desired state as for any other.

From the moment the runtime initiates an app's [start action](../language/action.md#start-action) until that start action ends, including while it is queued, the app's desired state MUST be as for an app with a lifecycle operation in progress.

> [!NOTE]
> Holding the whole resource graph back until the start action has run lets the start action bring an app up in its own order after the host restarts.

## Reconciliation loop

The runtime MUST run a reconciliation loop for as long as it runs.
Each iteration MUST perform these steps, in order:

1. Observe the host, as [observation](observation.md#observations) specifies.
2. Record the observations in the [world observation history](history.md#world-observation-history).
3. Derive each resource instance's [lifecycle state](../overview.md#lifecycle-state) from its observation history.
4. Where a lifecycle operation is suspended on a [barrier](suspension.md#barriers) whose condition is now satisfied, resume the operation's action closure.
5. Compute the operations needed to bring the derived lifecycle states to the desired state.
6. For each such operation, decide from the [autonomous operations log](history.md#autonomous-operations-log) whether to perform it, back off, or file a [fault](../overview.md#fault).
7. Perform the operations decided on in step 6.
8. Record each operation performed, with its [operation provenance](history.md#operation-provenance), in the autonomous operations log.

Given a desired state that does not change and a host that functions, successive iterations MUST converge on the desired state.

Every operation the runtime performs in an iteration MUST be idempotent.
Performing an operation whose effect is already in place MUST NOT produce an error or repeat a side effect.

An operation MUST NOT block the loop for an unbounded or long time.
An operation MUST NOT block the loop to wait on an external condition, such as a process exiting.
It MUST instead return control to the loop, and a later iteration MUST re-evaluate the condition.

## Applying absolute state

An app contributes to absolute state while it is `Installing`, `Installed` or `Uninstalling`.
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

The runtime MUST tear down infrastructure shared between apps only when no app is `Installing`, `Installed` or `Uninstalling`.
