# Replicable randomness

Password hashing formats are a later card; this card provides the derivation they will take salts from.

## Seed lifetimes

Two seeds, each scoped to the BSL object whose lifetime it shares:

- `app.random_*` derives from a per-registration seed, available in the static and action contexts.
- `rt.random_*` derives from a per-operation seed, available only where `rt` is.

Lifetimes considered:

| lifetime | outcome |
| --- | --- |
| per evaluation | Unusable: static values churn on every evaluation, restarting every consuming container; action values differ between passes, breaking the closure determinism replay relies on (SUSP#replay), and the runtime may replay at any time, not only after a restart. |
| per operation | Kept, as `rt.random_*`. Persisted with the operation and encrypted like its parameters (OPS#persistence), cleared when the operation ends, so every pass and restart replay sees the same values. No static context. |
| per generation | Rejected: every definition or parameter change restarts every consumer, and nothing needs it that per-registration does not cover. |
| per registration | Kept, as `app.random_*`. Covers the pairing case: a static deployment's env and an install action's `rt.exec` deriving the same database password. |

The app seed is created at registration and discarded at deregistration with the rest of the app's state.
It has no import or export: an exported seed would disclose every value the app will ever derive, more than any secret parameter does.
A host restored from its database and key file keeps its seeds; an app moved to another host gets a new seed, and its operator re-keys any data store holding app-seeded values.

Shell sessions are never persisted or replayed, so a session's seed can live in memory only.
Validators get no randomness: they judge a proposed value, and randomness has no meaning there.

## Docs

A guide in `docs/` covers the pitfalls:

- An operation-seeded value differs in the next operation; anything that must outlive the operation goes into a volume or comes from the app seed.
- App-seeded values change on re-registration, or when an app moves to another host, while data written to volumes keeps the old ones.
- The function, label and every argument enter the derivation: changing a length or alphabet changes the value, and two uses wanting distinct secrets need distinct labels.
