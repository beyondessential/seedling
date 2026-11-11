---
id: TRD
---

# App teardown

Applies to the removal of an [app](../overview.md#app)'s resources from a [host](../overview.md#host) on uninstall, and of the app itself from the [runtime](../overview.md#runtime) on deregistration.

## Deregistration

Deregistering an app MUST discard all state the runtime holds under the app's name.
That state includes the operator-configured [external service mappings](site-resources.md#external-mappings) and [external volume mappings](site-resources.md#external-mappings) made for the app.
If any of that state cannot be discarded, the deregistration MUST fail and leave the app registered.

> [!NOTE]
> An app name is the runtime's identity for an app, so state left under a name is inherited by the next app registered under it.
> An external volume mapping carries a target volume and a read-only flag, so an inherited mapping would mount data an operator granted to a different app.

## Uninstall scope

Uninstalling an app MUST affect only the [resources](../overview.md#resource) belonging to that app.

The runtime MUST identify an app's resources by the [identities](identity.md) it recorded for them, and MUST NOT identify them by the shape of their names.
The runtime MAY use a name prefix to enumerate candidates, and MUST then match each candidate exactly against a recorded identity before acting on it.

> [!NOTE]
> Names are not prefix-free: one app's name may be a prefix of another's.

If the runtime cannot read an app's recorded identities, it MUST NOT advance the app's teardown until it can.

An app having no recorded identities MUST NOT on its own be taken as evidence that its teardown has finished.
Where the runtime has no recorded identities for an app and cannot rule out that resources of the app remain on the host, it MUST NOT declare the app uninstalled.
In that case the runtime MUST hold a [fault](../overview.md#fault) of kind `uninstall_unidentifiable_units` against the app, and MUST clear it when the app's teardown completes or the app is deregistered.

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
