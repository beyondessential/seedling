# Replicable randomness

Password hashing formats are a later card; this card provides the derivation they will take salts from.

## Seed lifetimes

The script sees one set of `std::` helpers; the runtime picks the seed by where the call runs:

- In the static and validator contexts, a per-registration app seed. Validators run only on parameter changes to a registered app, so the seed always exists.
- In an action closure (action, shell, gate body), a per-operation seed. Gate checks run a Job, not a closure, so every action closure has an operation or shell session behind it.

Lifetimes considered:

| lifetime | outcome |
| --- | --- |
| per evaluation | Unusable: static values churn on every evaluation, restarting every consuming container; action values differ between passes, breaking the closure determinism replay relies on (SUSP#replay), and the runtime may replay at any time, not only after a restart. |
| per operation | Kept, for action closures. Persisted with the operation and encrypted like its parameters (OPS#persistence), cleared when the operation ends, so every pass and restart replay sees the same values. |
| per generation | Rejected: every definition or parameter change restarts every consumer, and nothing needs it that per-registration does not cover. |
| per registration | Kept, for the static context. A closure that needs the same value as a workload's config, such as the database password an install action sets, captures the top-level value. |

The app seed is created at registration and discarded at deregistration with the rest of the app's state.
It has no import or export: an exported seed would disclose every value the app will ever derive, more than any secret parameter does.
A host restored from its database and key file keeps its seeds; an app moved to another host gets a new seed, and its operator re-keys any data store holding app-seeded values.

An operator can rotate the app seed (`/apps/seed/rotate`, `apps seed rotate <app> --yes`, a `dangerous`-mode console control). Rotation re-evaluates without bumping the generation, like a host constant change; every consumer restarts, and data stores are re-keyed by hand. Operation seeds need no rotation.

Shell sessions are never persisted or replayed, so a session's seed can live in memory only.

## Functions

In `std`, as every BSL helper is (STD):

- `random_bytes(label, n)` returns a Blob. Encodings to text arrive with the hashing card.
- `random_string(label, length, alphabet)` draws `length` characters from `alphabet`, without bias. The alphabet is required, so a script always states which characters a consumer accepts.
- `uuid_v4(label)` returns a version 4 UUID, hyphenated and lowercase. Named by version so other UUID versions can be added beside it.

The derivation is pinned in RND (HMAC-SHA-256 over a length-prefixed call encoding, keying a ChaCha20 stream), since changing it would rotate every derived value on upgrade.

STD#replicability names derivation helpers as the one exception to reading only what the arguments carry, and STD#results admits a Blob result.
The app seed counts among the inputs of replicable evaluation (BSL#replicability), beside the bundle, parameter values and host constants.

A label is any non-empty string.
The function name, the label and every argument enter the derivation, so no call's value is a prefix of, or otherwise related to, another's.

## Redaction

Every value an evaluation derives, from either seed, is redacted wherever secret parameter values are: the substring rule for environment values in describing an app, and the runtime's logs and events.
A derived Blob counts through each encoding BSL offers, worded generically so the hashing card's encodings fall under it without editing the spec; with no encodings yet, that clause has nothing to match.
Generation history records parameters, not derived values, so it needs no change.
The redaction gaps secret parameters already have (volume file writes, sub-action parameter maps in the action log) are card X6.

## Docs

A guide in `docs/` covers the pitfalls:

- The same call derives a different value inside an action closure than at the top level: derive a value the workloads also hold at the top level and capture it.
- An operation-seeded value differs in the next operation; anything that must outlive the operation goes into a volume or is derived at the top level.
- App-seeded values change on rotation, on re-registration, or when an app moves to another host, while data written to volumes keeps the old ones.
- The Replicability paragraph and the Standard helpers section of `docs/bsl-scripting.md` cover the app seed as an evaluation input, the derivation helpers, and their one exception to the helper conventions.
- The function, label and every argument enter the derivation: changing a length or alphabet changes the value, and two uses wanting distinct secrets need distinct labels.
