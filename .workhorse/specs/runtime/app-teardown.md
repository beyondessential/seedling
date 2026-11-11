---
id: TRD
---

# App teardown

Applies to the removal of an [app](../overview.md#app)'s resources from a [host](../overview.md#host) on uninstall, and of the app itself from the [runtime](../overview.md#runtime) on deregistration.

## Deregistration

When it deregisters an app, the runtime MUST discard all state it holds under the app's name.
That state includes the operator-configured [external service mappings](site-resources.md#external-mappings) and [external volume mappings](site-resources.md#external-mappings) made for the app.
If the runtime cannot discard any of that state, it MUST fail the deregistration and leave the app registered.

> [!NOTE]
> An app name is the runtime's identity for an app, so state left under a name is inherited by the next app registered under it.
> An external volume mapping carries a target volume and a read-only flag, so an inherited mapping would mount data an operator granted to a different app.

## Uninstall scope

When it uninstalls an app, the runtime MUST act only on the [resources](../overview.md#resource) belonging to that app.

The runtime MUST identify an app's resources by the [identities](identity.md) it recorded for them, and MUST NOT identify them by the shape of their names.
The runtime MAY use a name prefix to enumerate candidates, and MUST then match each candidate exactly against a recorded identity before acting on it.

> [!NOTE]
> Names are not prefix-free: one app's name may be a prefix of another's.

If the runtime cannot read an app's recorded identities, it MUST NOT advance the app's teardown until it can.

The runtime MUST NOT take an app's lack of recorded identities, on its own, as evidence that its teardown has finished.
Where the runtime has no recorded identities for an app, it MUST enumerate the resources on the host that may belong to the app, and MUST rule out each one that exactly matches a recorded identity of another app.
While any enumerated resource is not ruled out, the runtime MUST NOT declare the app uninstalled, and MUST hold an [`uninstall_unidentifiable_units`](faults.md#app-faults) fault.
If an operator clears that fault, the runtime MUST complete the teardown, leave the resources that were not ruled out untouched, and not file the fault again for that uninstall.

## Teardown

An uninstall MUST bring every resource instance recorded for the app to [`Unscheduled`](lifecycle.md#lifecycle-states).
The runtime MUST identify each instance it tears down by its recorded identity, as [uninstall scope](#uninstall-scope) specifies.

Teardown completes when every resource instance recorded for the app is `Unscheduled`.
When teardown completes, the runtime MUST move the app to `NotInstalled`, emit an `AppPhaseChanged` [event](../interface/events.md#event-types), and forget the app's recorded instances.

## Uninstall admission

The runtime MUST reject an uninstall while a [lifecycle operation](../overview.md#lifecycle-operation) for the app is in progress or queued, or while a parameter or definition change to the app is being committed.
The rejection MUST name the action, or the change, that holds the app.

The runtime MUST decide whether an operation holds the app from its record of admitted operations, and not from the app's [install phase](../overview.md#install-phase).
If the runtime cannot determine whether an operation holds the app, it MUST reject the uninstall.

> [!NOTE]
> An action closure runs while its app is in the installed phase, so the phase does not distinguish an idle app from one running an action.

The check and the app's departure from the installed phase MUST be indivisible with respect to admitting a lifecycle operation, so that no operation is admitted between them.
Once an app has left the installed phase to be torn down, the runtime MUST NOT admit a lifecycle operation for it, whether an operator action, a parameter change's handler, a scheduled firing or an install requested it.

> [!NOTE]
> A rule that each requester of an operation applies for itself holds only for the requesters that know it.
> Admission is one decision, made where operations are admitted.

> [!NOTE]
> An operator who wants the uninstall to proceed [cancels](operations.md#cancellation) the operation and retries.
> Cancelling on the operator's behalf would need a bound on how long the uninstall waits for the closure to unwind, and an uninstall that stops waiting leaves the app half torn down.
