---
id: RCK
---

# Definition re-checks

Applies to the [runtime](../overview.md#runtime)'s periodic check of whether the registry tag an [app](../overview.md#app)'s definition was fetched from now selects a different definition.

## Resolution

For every app whose definition was [fetched](../interface/definitions.md#fetching) from a reference naming a tag, the runtime MUST periodically re-check that reference.
A re-check MUST resolve the reference and select a definition manifest in the same way as a [fetch](../interface/definitions.md#artefact-selection), and MUST compare the selected digest with the digest of the definition the app is running.
A re-check MUST read only manifests.
It MUST NOT download the bundle, change the app, or install anything.

The runtime MUST NOT re-check a definition fetched by digest, or a pushed definition.

A re-check MUST honour the [registry allowlist](../interface/definitions.md#registry-access).
A re-check the allowlist does not permit MUST count as a failed re-check.

The outcome of a re-check is reported by the [`definition_source_moved`](faults.md#app-faults) fault.

## Schedule

The runtime MUST NOT re-check an app within an hour of that app's last successful re-check.
The runtime SHOULD re-check each app about every six hours.
The runtime MUST spread the timing of its re-checks so that hosts do not re-resolve against a registry at the same moment.

> [!NOTE]
> A tag moves rarely, and acting on a move is the operator's decision.
> One such schedule waits six hours plus a random delay of up to 36 minutes before each app's first re-check and after each successful one.

## Back-off

The runtime MUST space an app's consecutive failed re-checks by a delay that grows with each failure, up to a bounded maximum.
The runtime SHOULD begin that delay at 15 minutes, double it with each consecutive failure, and cap it at 24 hours.
However long an app's re-checks have been failing, the runtime MUST make another attempt once the maximum delay has passed since the last failure.
A successful re-check MUST reset the delay.

After 5 consecutive failed re-checks of an app, the runtime MUST file a [`definition_recheck_failed`](faults.md#app-faults) fault, and MUST keep re-checking at the delay above.

One app's failed or slow re-checks MUST NOT delay another app's re-checks.
