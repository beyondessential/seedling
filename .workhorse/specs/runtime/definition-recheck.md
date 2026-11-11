---
id: RCK
---

# Definition re-checks

## Definition Sources

<!-- from runtime: Definition Sources -->

> r[definition.recheck]
> For every app whose definition was fetched from a reference naming a tag, the runtime periodically re-resolves that reference, selecting the definition manifest in the same way as a [fetch](rule:i/definition.fetch.select), and compares the selected digest with the one the app is running.
> A re-check reads only manifests: it does not download the bundle, change the app, or install anything.
> A definition fetched by digest, and a pushed definition, are never re-checked.
> A re-check honours the [registry allowlist](rule:i/definition.fetch.access), and one it may not make counts as a failed re-check.

> r[definition.recheck.cadence]
> Re-checks are infrequent, since a tag moves rarely and acting on a move is the operator's decision.
> Their timing is spread across hosts, so a fleet does not re-resolve against a registry at the same moment.
> The exact schedule is not prescribed by this spec.

> r[definition.recheck.backoff]
> Consecutive failed re-checks for an app are spaced by an increasing delay up to a bounded maximum, so that an unreachable registry is not contacted on every cycle.
> However long an app's re-checks have been failing, waiting the maximum delay is sufficient for another attempt, and a successful re-check resets the delay.
> One app's failing re-checks do not delay another's.
