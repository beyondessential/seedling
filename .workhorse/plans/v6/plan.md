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

## Implementation notes

- **Seed frame.** `std` helpers are free functions with no receiver, so they read their seeds from a thread-local frame (`runtime/seeds.rs`), the way the action-closure and validator flags already work. A helper reads the operation slot when `is_in_action_closure()` holds, and the app slot otherwise (static and validator).
- **Missing seeds.** An app seed is `Ready` or `Unreadable(reason)`, and a frame with no seed installed is a third case. A helper throws on the last two with distinct messages. A registered app without a recorded seed loads as unreadable, never as one to create.
- **Seeds are required by type** in `apps::evaluate` and `evaluate_validated`. The evaluated `App` carries the seed it was evaluated with, and `capture_closures` re-enters it. So every operation pass and probe re-evaluates the static part against the seed the running definition was declared with, and a closure that captures a top-level value captures the one the workloads hold. `reconstruct_app_def` loads the seed from the database, one seed serving every generation of a registration.
- **Who evaluates against which seed.**
  - Registration draws the seed before evaluating and commits it in the registration transaction.
  - Template preview evaluates against a throwaway seed.
  - Image discovery, updates and parameter changes use the registered app's seed.
- **Operation seed.**
  - The seed is drawn before the first persist and stored, encrypted, in `current_operation.seed_ciphertext`. Saving again for the same operation keeps the seed on record, which a gate run's replay relies on.
  - Every operation loop reads the seed back from the database before its first pass (`history::load_operation_seed`).
  - A row from before the column existed derived nothing, so it is given a seed. An unreadable seed fails the operation.
  - Shell sessions draw one in memory, and probes draw a throwaway one.
- **Backfill.** Migration v83 creates `app_seeds` and a one-row `app_seed_backfill` marker. After the cipher loads, the daemon seeds every registered app without a row, then deletes the marker.
- **Derived values** are recorded only from the app seed, on `AppDef::derived`, and `describe_app` withholds any environment value they are found in. A Blob is recorded in both text forms Rhai offers: its UTF-8 reading, and the hex digits of its string form. Values derived from an operation seed are not recorded, because their redaction in `rt.exec` breadcrumbs is #X6's.
- **Recorded as keyed hashes.** They serve only redaction, so each form is held as its length and a SipHash-1-3 under a key drawn once per process. Finding them hashes each window of a recorded length in the text. A collision only withholds more.
- **Derived values limit.** `--script-max-derived-values` (1 000 by default, 0 for none) caps derivation calls per evaluation, bounding what a definition can make the runtime hold. Engines carry the limit as their default tag for the helpers to read; the count resets with each seed frame, so an operation pass's static run and its closure each have their own budget, as the operations limit does.
- **Size limits.** `n` and `length` are checked against the engine's array and string limits before allocating, and the allocation itself uses `try_reserve`.
- **Rotation** takes the app's change hold only while nothing runs or is queued, checked and taken under one scheduler lock (`AppHold::take_idle`). It persists the new seed, updates the entry, announces `AppSeedRotated`, then reloads at the same generation and syncs faults. The CLI refuses an unconfirmed rotation before it connects.
- **`script_error` fault sync** is one function now. Re-evaluation after a host change also records a changed Canopy type, which the copy it had did not.
- **Deregistration** deletes the app's seed and its `secret_params` rows. The latter was a pre-existing bug, fixed in its own commit.

## Checklist

### Derivation

- [x] `cargo add` `hmac` and `chacha20` to `crates/core`
- [x] `crates/core/src/defs/stdlib.rs` with `defs/stdlib/random.rs`: call encoding, call key, byte stream, the three value constructions
- [x] Known-answer tests against vectors from an independent Python implementation, plus the RFC 8439 §2.4.2 keystream
- [x] Argument checks through `defs::take`, with one test per rule
- [x] `random_string` rejection boundary test
- [x] `uuid_v4` version and variant bits

### Seed frame and contexts

- [x] Thread-local seed frame, guards and the app seed type in `runtime/seeds.rs`
- [x] Helpers pick the seed by context: static, validator and action tests, including a captured top-level value
- [x] `run_definition` and `capture_closures` install the app seed; validators run inside the same frame
- [x] `ActionClosureGuard::with_operation_seed`; probes get a throwaway operation seed
- [x] App seed threaded through every evaluation site

### Storage

- [x] Migration v83: `app_seeds`, `current_operation.seed_ciphertext`, backfill marker
- [x] `runtime/apps/seeds.rs`: store, load, delete, backfill
- [x] Daemon start: backfill, then `load_from_db` fills `AppEntry` seeds; unreadable rows stay unreadable
- [x] Registration draws and persists the seed in its transaction; two registrations derive apart
- [x] Deregistration deletes the app's seed and its `secret_params` (own commit and test)
- [x] Operation seed persisted and read back on replay; a test replays from a persisted row
- [x] Invoked actions derive from the caller's operation seed
- [x] Shell sessions draw an in-memory seed

### Rotation

- [x] One `script_error` sync function (own commit)
- [x] `/apps/seed/rotate` handler and route
- [x] `AppSeedRotated` event, emitters and shape test
- [x] Handler tests: `not_found`; `operation_in_progress` for running, queued and held; same generation; failing evaluation keeps the seed and files `script_error`; event emitted
- [x] CLI `apps seed rotate <app> --yes`, refusing before connecting without it
- [x] Console Rotate seed control (dangerous), confirm dialog, event handling, frontend tests

### Redaction

- [x] Derived values recorded on the definition
- [x] `describe_app` withholds env values holding them, including a Blob's string form
- [x] Seeds print as `Seed(..)` and are never formatted into logs, events or responses

### Docs

- [x] `docs/bsl-scripting.md`: replicability inputs, the helpers' exception to the conventions, a Derived random values section with the pitfalls (its example checked by running it)
- [x] `docs/deploying.md`: `apps seed rotate`
- [x] `docs/threat-model.md`: seeds and derived values
