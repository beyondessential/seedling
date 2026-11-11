---
id: RCN
---

# Reconciliation

## Reconciliation

<!-- from runtime: Reconciliation -->

> r[reconciliation.loop]
> The runtime must run a continuous reconciliation loop.
> Each iteration of the loop must:
>
> 1. Observe the current state of the world.
> 2. Record observations to the [world observation history](rule:r/history.world).
> 3. Derive the [lifecycle state](rule:r/lifecycle.states) of each resource from its observation history.
> 4. If a [lifecycle operation](rule:r/operation.lifecycle) is in progress and suspended on a [barrier](rule:r/barrier.suspension), check whether the barrier condition is satisfied. If so, resume the action closure.
> 5. Compute the difference between the [desired state](rule:r/desired-state.definition) and the derived state.
> 6. For each intended operation, consult the [autonomous operations log](rule:r/history.operations) to decide whether to proceed, back off, or file a [fault](rule:r/fault.definition).
> 7. Execute the operations that pass evaluation.
> 8. Record each executed operation (with [provenance](rule:r/history.operations.provenance)) to the autonomous operations log.

> r[reconciliation.convergence]
> The reconciler must be convergent: given a stable desired state and a functioning system, repeated iterations must bring the world closer to the desired state, and eventually reach it.

> r[reconciliation.idempotency]
> All operations performed by the reconciler must be idempotent.
> Performing the same operation twice must not cause errors or duplicate side effects.

> r[reconciliation.liveness]
> Individual reconciliation operations must not block the loop for an unbounded or long duration.
> When an operation requires waiting for an external condition (e.g. a process to terminate), the reconciler must release control and re-evaluate the condition on a subsequent iteration rather than polling inline.

> r[reconciliation.absolute-state]
> Some state the reconciler maintains is *absolute*: it is rebuilt in full each iteration and applied wholesale, so anything absent from what was built is removed from the host.
> An installed app's contribution to absolute state may be withdrawn only by an explicit [lifecycle](rule:r/lifecycle.states) transition — never because computing that contribution failed.
> When an iteration cannot compute the contribution of every installed app, the affected absolute state must not be applied for that iteration; the previously applied state stands until an iteration succeeds.
> An iteration that withholds an apply must draw no conclusion from having done so: the fault for that state is neither filed nor cleared, and no resource is recorded as ready on the strength of an apply that did not run.
> State that comes out empty from an iteration that computed every contribution is a result like any other, and is applied: removing the last ingress from the last app must leave the proxy serving nothing, not whatever it was last given.
> Likewise, a full teardown of shared infrastructure occurs only when no app is installed or installing — never when apps are installed but their state could not be computed.
> This does not constrain per-resource incremental actuation, which acts only on the resources it names.

## Desired State

<!-- from runtime: Desired State -->

> r[desired-state.definition]
> The desired state is the set of resources that should exist and their intended [lifecycle states](rule:r/lifecycle.states).
>
> The desired state is derived from two inputs:
>
> 1. The AppDef: the resource graph produced by evaluating the BSL script.
> 2. The progress of the current [lifecycle operation](rule:r/operation.lifecycle), if any: which resources have been started or stopped by the action closure so far.

> r[desired-state.steady]
> When no lifecycle operation is active, the desired state is the full AppDef.
> The reconciler maintains it autonomously: restarting terminated containers according to their restart policy, maintaining scale, and keeping networking and ingress configuration consistent.

> r[desired-state.during-operation]
> When a lifecycle operation is in progress, the desired state is built incrementally by the action closure's `rt.start()` and `rt.stop()` calls.
> The reconciler must only act on resources that the operation has placed into the desired state so far.
> Resources that are not in the desired state during an operation are left untouched: any running instances continue to run with whatever spec they were last started with, and no rotation is performed against them until the operation either schedules them explicitly or completes (returning the app to steady state).

> r[desired-state.during-install]
> An install operation obeys the same rules as any other lifecycle operation under [desired-state.during-operation](rule:r/desired-state.during-operation).
> While an app is in the `Installing` status, the reconciler must actuate resources the install closure has placed into the desired state, exactly as it would for any other in-progress operation.
> The fact that the app has never previously been installed does not exempt the reconciler from actuating these resources.
