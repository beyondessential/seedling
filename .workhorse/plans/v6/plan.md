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

- **Seed frame.** `std` helpers are free functions with no receiver, so they read their seeds from a thread-local frame, the way `is_in_action_closure()` and `ValidatorFrame` already carry context. The frame holds an app seed slot and an operation seed slot. A helper picks the operation slot when `is_in_action_closure()` holds, and the app slot otherwise (static and validator).
- **A slot is three-state:** a seed, an unreadable seed carrying its error, or no seed for this evaluation. A helper throws on the last two with distinct messages. An unreadable seed must never read as an absent one, and absent must never silently become a fresh one.
- **Seeds are required by type.** `apps::evaluate`, `evaluate_validated`, `run_definition` and `reconstruct_app_def` take the app seed as a parameter, not an `Option`, so the compiler finds every evaluation site. The sites are registration, update, `set_params`, template preview, image discovery, `load_from_db`, `reload`, and generation reconstruction for operations.
- **Unregistered apps.** Registration draws the seed before it evaluates, evaluates against it, and writes it in the registration transaction. Template preview and image discovery for a proposed script evaluate against a throwaway seed. Neither persists anything derived.
- **In-memory copy.** `AppEntry` holds the decrypted app seed, or its unreadable state, loaded in `load_from_db` and set in `insert_registered`. `reload`, `reconstruct_app_def` and rotation read it from there rather than going back to the database for each evaluation.
- **Operation seed.** It is drawn in `run_operation_loop` and gate runs (`gates/run.rs`) before the first persist, carried in `OperationContext`, and installed by `ActionClosureGuard`. Replay reads it back through `read_operation_state`, which already fails as a whole when any part is unreadable. Shell sessions draw one in `run_app_shell` and never persist it. Probes draw a throwaway one.
- **Storage.**
  - New table `app_seeds (app_name PK, ciphertext BLOB NOT NULL)`, plus a new `seed_ciphertext` column on `current_operation`.
  - Both column names end in `ciphertext`, so `protected_values_exist` sees them and a missing key file still stops the daemon.
  - Seeds are encrypted with the existing `Cipher` (orion AEAD) via `SecretString`, or a byte-oriented equivalent added beside it.
- **Backfill for already-registered apps.** Migrations run before the cipher is loaded, so they cannot draw or encrypt seeds. The migration creates the table and records that a backfill is pending. After `Cipher::load_or_create`, the daemon gives a seed to every registered app without an `app_seeds` row, in one transaction, then clears the marker. An existing but unreadable row is never replaced.
- **Size limits.** `random_bytes` and `random_string` check `n` and `length` against the engine's array and string size limits before allocating. A large `n` fails as an engine limit rather than allocating first.
- **Dependencies.**
  - `hmac` must be the version built on the same `digest` as the workspace `sha2` 0.11.1.
  - `chacha20` is the RFC 8439 variant (`ChaCha20`, 12-byte nonce).
  - Add both with `cargo add`. Confirm with the RFC 8439 §2.4.2 keystream vector that the crate's counter and nonce layout matches the spec.
- **Recording derived values.** Every value a helper returns is recorded in the frame:
  - Values from the static evaluation are kept on `AppEntry` beside the seed, for `describe_app`'s substring redaction.
  - Values from an operation are kept for the operation's own logs. Redacting them in `rt.exec` breadcrumbs is #X6's, filed there.
- **`script_error` fault sync.** It has three copies: `sync_fault_state`, `sync_script_error_fault`, and an inlined copy in `re_evaluate_all_apps`. Rotation adds a fourth caller, so fold the three into one first, per the rule on shared contracts.
- **Pre-existing bug.** Deregistration never deletes `secret_params` rows: `delete_app_params` only clears `params`, and `delete_app_secret_params` has no caller. An app registered again under the same name would load the old app's secret values. It is fixed here because the app seed's deletion goes into the same block.

## Checklist

### Derivation

- [ ] `cargo add` `hmac` and `chacha20` to `crates/core` (workspace dependencies, full versions)
- [ ] `crates/core/src/defs/std.rs` (replacing the empty `Module::new()` in `defs.rs`) with `defs/std/random.rs`: call encoding, call key, byte stream, and the three value constructions, cited `// spec: RND#call-encoding` / `#byte-stream` / `#values`
- [ ] Known-answer tests for each helper, against vectors computed outside this code (Python `hmac` plus a ChaCha20 checked against RFC 8439 §2.4.2), committed with the script that produced them as a comment
- [ ] Argument checks through `defs::take`: empty label, `n`/`length` below 1, alphabet under two characters or with a repeat (counted as Unicode scalar values), engine size limits; one test per rule
- [ ] `random_string` rejection sampling: a test with an alphabet size that does not divide 2^32, asserting the discard boundary
- [ ] `uuid_v4`: version and variant bits, lowercase hyphenated form

### Seed frame and contexts

- [ ] Thread-local seed frame with app and operation slots, its guard, and the three-state slot type, in `runtime/seeds.rs` (or beside `secrets.rs`)
- [ ] Helpers pick the slot by context; tests in `tests/stdlib.rs` for each context: static and validator calls agree; an action closure call differs from the same call at top level; a closure capturing a top-level value sees it unchanged
- [ ] `run_definition` and `capture_closures` install the app seed; `run_validators` runs inside the same frame
- [ ] `ActionClosureGuard` installs the operation seed from `OperationContext`; probes get throwaway seeds
- [ ] Thread the app seed through `evaluate`, `evaluate_validated`, `reconstruct_app_def`, `reload`, and every call site listed above

### Storage

- [ ] Migration v83: `app_seeds` table, `current_operation.seed_ciphertext`, backfill marker
- [ ] `runtime/apps/seeds.rs`: draw (CSPRNG, 32 bytes), encrypt, upsert with `ON CONFLICT DO UPDATE`, load to the three-state type, delete; cited `SEC#seeds`, `SEC#app-seeds`
- [ ] Daemon start: backfill after the cipher loads, then `load_from_db` fills `AppEntry` seeds; test that an unreadable row stays unreadable and the app's helper calls fail with `script_error`
- [ ] Registration draws and persists the seed in its transaction; test that two registrations of one script derive different values
- [ ] Deregistration deletes the app's seed and its `secret_params` (the pre-existing bug, own commit with its own test)
- [ ] `save_current_operation` stores the operation seed; `read_operation_state` and `start_replay` restore it; a test severs in-memory state between persist and replay and checks a closure's derived values match
- [ ] Invoked actions derive from the caller's operation seed (test through `Action.invoke`)
- [ ] Shell sessions draw an in-memory seed per session

### Rotation

- [ ] Consolidate the three `script_error` sync copies into one function (own commit)
- [ ] `/apps/seed/rotate` in `oi/handler/apps.rs` and the `handler.rs` route table: `not_found`, `blocker_for` mapped through `app_blocked`, `AppHold` for the duration, persist the new seed, then reload without a generation change, sync the `script_error` fault, wake the tick, reply `{}`; cited `CTL#seed-rotation`
- [ ] `AppSeedRotated` event in `crates/protocol/src/events.rs` with emitters on both senders and the shape test
- [ ] Handler tests: `not_found`; `operation_in_progress` for running, queued and held; generation unchanged; a failing evaluation keeps the new seed and files `script_error`; the event is emitted
- [ ] CLI `apps seed rotate <app> --yes` in `crates/ctl/src/apps.rs` (nested like `apps gate run`); without `--yes`, print the consequences, send nothing, exit non-zero
- [ ] Web console: a Rotate seed control on the app detail page, `safety="dangerous"`, with a confirm dialog through `useOiAction`; `AppSeedRotated` in `EventsSidebar`, `actor_activity.rs`, `APP_DETAIL_EVENTS` and the app list refresh set; frontend test asserting the OI call

### Redaction

- [ ] Record derived values in the frame and keep the static evaluation's on `AppEntry`
- [ ] `describe_app` adds them to the substring set passed to `redact_secret_env`; test with a derived value inside an env value
- [ ] Seeds never reach a log line, event or OI response: grep-level review of every place the new types are formatted, and `Debug` on the seed type that prints nothing

### Docs

- [ ] `docs/bsl-scripting.md`: Replicability lists the app seed; Standard helpers lists the three helpers and names derivation as the one exception to the replicability convention; a Derived random values section with the pitfalls above
- [ ] `docs/deploying.md` operator commands: `apps seed rotate`
- [ ] `docs/threat-model.md`: seeds under the secret key, rotation, and what a leaked key file now exposes
