---
id: TRD
---

# App teardown

## Generation

<!-- from runtime: Generation -->

> r[deregister.purge]
> Deregistering an app must discard every piece of state the runtime holds under that app's name, including the operator-configured [external service](rule:r/service.external.mapping.events) and [external volume](rule:r/volume.external.mapping.events) mappings made for it. An app name is the runtime's identity for an app, so state surviving the app it names is inherited by the next app registered under that name rather than belonging to nobody.
>
> External volume mappings make that inheritance a question of access rather than tidiness: a mapping carries a target volume and a read-only flag, so an app coming up under a reused name would mount another app's data on the strength of a decision an operator made about a different app, having declared only that it wants an external volume by that name.

## Fault Handling

<!-- from runtime: Fault Handling -->

> r[app.uninstall.scope]
> Uninstalling an app affects only resources belonging to that app.
> Resources are identified by the identities the runtime recorded for them, not by the shape of their names: names are not required to be prefix-free, so one app's name may be a prefix of another's.
> A scan by name prefix is permitted only to enumerate candidates that are then matched exactly against recorded identity.
> Having no recorded identities is not on its own evidence that teardown has finished: where the runtime cannot rule out that resources of the app are still present, it must not declare the app uninstalled, and must raise a fault that clears when teardown does complete.

> r[app.uninstall.exclusive]
> An uninstall must be rejected while a [lifecycle operation](rule:r/operation.lifecycle) for that app is in progress or queued, and the rejection must name the action that holds the app so the operator knows what to cancel.
> Uninstall must determine this from the scheduler's record of operations rather than from the app's phase: an action closure runs with its app in the `Installed` phase, so the phase does not distinguish an idle app from one mid-action.
> Where the runtime cannot determine whether an operation holds the app, it must reject.
> The check and the app's departure from the installed phase must be indivisible with respect to admitting an action, so an action cannot be admitted between the two and then run its closure under the teardown.
> Once the app has left the installed phase to be torn down, no lifecycle operation may be admitted for it, whatever asked for one: an operator action, a param change's handler, a scheduled firing, or an install.
>
> Admission is one decision and belongs wherever operations are admitted, not with each caller that asks for one.
> A rule each caller is expected to apply for itself holds only for the callers that know it, and the next one added will not.
>
> Rejecting rather than cancelling the operation keeps uninstall consistent with [operation.lifecycle.single.intra-app](rule:r/operation.lifecycle.single.intra-app), which rejects a competing request rather than pre-empting the operation in flight.
> Cancelling on the operator's behalf would need a bound on how long uninstall waits for the closure to unwind, and an uninstall that abandons the wait leaves the app in exactly the half-torn-down state the rejection exists to prevent.
> [Cancel](rule:r/operation.cancel) is already an operator-reachable action, so an operator who wants the uninstall to win cancels and retries.
