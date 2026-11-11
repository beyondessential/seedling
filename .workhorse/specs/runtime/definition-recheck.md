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

The outcome of a re-check is reported by the [definition source moved fault](faults.md#app-faults).

## Schedule

The runtime SHOULD re-check each app every six hours.
The runtime SHOULD delay each app's first re-check, and each re-check after a success, by a further random interval of up to a tenth of the six hours, so that hosts started together do not re-resolve against a registry at the same moment.

## Back-off

The runtime MUST space an app's consecutive failed re-checks by a delay that grows with each failure, up to a bounded maximum.
The runtime SHOULD begin that delay at 15 minutes, double it with each consecutive failure, and cap it at 24 hours.
However long an app's re-checks have been failing, the runtime MUST make another attempt once the maximum delay has passed since the last failure.
A successful re-check MUST reset the delay.

One app's failed or slow re-checks MUST NOT delay another app's re-checks.
