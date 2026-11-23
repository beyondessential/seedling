# Replicable randomness: test cases

## Derivation

- [x] Each helper's value under a fixed seed matches an independent implementation of the pinned derivation (verifies spec: RND)
- [x] The ChaCha20 keystream matches RFC 8439 §2.4.2 (verifies spec: RND)
- [x] A word in the biased top of the range is discarded, and none is for a power-of-two alphabet (verifies spec: RND)
- [x] `uuid_v4` carries version 4 and the RFC variant, hyphenated and lowercase (verifies spec: RND)
- [x] An empty label, `n` or `length` below 1, an alphabet of one character or with a repeat, and a non-string label each throw (verifies spec: RND)
- [x] A multibyte alphabet is read as characters (verifies spec: RND)
- [x] A count beyond the engine's limits fails without allocating (verifies spec: LIM)

## Seeds and contexts

- [x] The same script under the same seed derives the same values, and under another seed different ones (verifies spec: RND)
- [x] Adding a call leaves the other calls' values unchanged (verifies spec: RND)
- [x] A validator derives what the top level derives (verifies spec: RND, STD)
- [x] An action closure derives a different value from the same call at the top level, and a captured top-level value is the one the app declared (verifies spec: RND)
- [x] An invoked action derives what its caller would (verifies spec: SEC)
- [x] An unreadable app seed fails only evaluations that derive (verifies spec: SEC)
- [x] Two registrations of one script derive apart; registering again after deregistering draws a new seed, and deregistration removes the old one (verifies spec: SEC)
- [x] A replay derives from the operation seed on record (verifies spec: SEC)
- [x] Saving an operation again keeps its first seed; the next operation records its own (verifies spec: SEC)
- [x] An operation persisted before seeds existed is given one that lasts; an unreadable one is never replaced (verifies spec: SEC)
- [x] The one-time backfill seeds unseeded apps and leaves existing rows, readable or not (verifies spec: SEC)
- [x] Seeds are encrypted at rest and count as protected values for the missing-key check (verifies spec: SEC)
- [ ] Upgrade a host with registered apps: every app gets a seed at start, and its workloads keep running
- [ ] Restart the daemon mid-operation on a closure that derives a value: the replay derives the same value

## Rotation

- [x] Rotation changes derived values at the same generation and emits `AppSeedRotated` (verifies spec: CTL, SEC, EVT)
- [x] Rotation of an unregistered app is `not_found` (verifies spec: CTL)
- [x] Rotation is `operation_in_progress` while an operation runs or is queued or a change holds the app, and leaves the seed (verifies spec: CTL)
- [x] A definition failing against the new seed keeps it and files `script_error` (verifies spec: CTL, SEC)
- [x] The CLI parses `apps seed rotate <app>` with and without `--yes` (verifies spec: CLI)
- [x] `apps seed rotate <app>` without `--yes` prints the consequences, exits non-zero and never connects (verifies spec: CLI); run by hand against no server
- [x] The console's Rotate seed control is disabled below the dangerous mode, and confirms before calling `/apps/seed/rotate` (verifies spec: UI)
- [ ] On a running app whose env holds a derived value, rotation restarts that deployment and nothing else

## Redaction

- [x] An env value containing a derived string, or a Blob's string form, is `null` in `/apps/show` (verifies spec: APPS, SEC)
- [x] A seed never prints through `Debug` (verifies spec: SEC)
- [x] Deregistering an app discards its secret parameter values, so the next app of the name does not inherit them (verifies spec: TRD)
