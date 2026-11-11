# B5: SAN index for certificate resolution

## Shape

A `tls_cert_sans` table: one row per (certificate, SAN DNS name), lowercased, indexed on the name, `cert_id REFERENCES tls_certificates(id) ON DELETE CASCADE`. Each row also carries the certificate's self-issuance as worked out from its PEM (issuer DN equal to subject DN), so candidates can be ranked without parsing.

Every row with a PEM is indexed, whatever its state. Lookups join back to `tls_certificates` for state and validity, so supersede and revoke never touch the index. A hostname's history rows come back as candidates. Their number grows with how often that one name has been renewed, not with the store.

A lookup computes two keys: the hostname, and `*.` followed by everything after its first label (no wildcard key when it has no dot). An exact match ranks as `exact`, a wildcard match as not. That is RFC 6125's rule as `parse::san_covers` applies it. Keep the key derivation next to `san_covers` in `parse.rs`, with a test that checks the two agree.

## The index is not a source of truth

- Narrow and rank candidates using the index, then confirm the winner by parsing its own PEM and re-ranking it before using it.
- When confirmation contradicts the index (the winner's PEM does not cover the hostname, or ranks differently), rewrite that certificate's SAN rows from its PEM, warn, and move to the next candidate.
- The snapshot is an in-memory view with no database handle, so on a contradiction it warns and skips. The serving lookup ranks the same index the same way, so the next handshake for that hostname reaches the same certificate and rewrites its rows.
- A lookup can't detect a missing SAN row without scanning everything, which is the cost this card removes. Writing SAN rows in the same transaction as the certificate is the first guard. The background guards below repair drift eventually. Say so in the doc comment.

## Drift guards

Both guards rebuild rows with the same derivation the write paths use. They catch rows that have drifted from their PEM, never a bug in the derivation itself.

**Marker, trigger and sweep (prompt repair).**
- The marker is a row in its own table, `tls_cert_index` (one per indexed certificate, also holding self-issuance), not a column on `tls_certificates`. The SAN rows reference it with `ON DELETE CASCADE`. Changed from the first plan: a future migration that rebuilds `tls_certificates` cascades away the index rows and their marker together, so the certificates show up as unindexed, where a marker column would have been carried over and still claimed they were indexed.
- A certificate whose PEM will not parse gets an index row with `self_issued` NULL and no SANs, so it isn't retried forever.
- A trigger `AFTER UPDATE OF cert_pem` (only when the value changes) deletes the certificate's index row. That turns a stale index into a missing one, which the sweep can see. The activate paths update the PEM and then index inside one transaction.
- The unindexed sweep is an anti-join: certificates with a PEM and no index row. It parses nothing when the index is right. It runs at startup and every five minutes.
- `Snapshot::load` already reads every certificate row, so it parses any active certificate missing from the index on the spot (logged as an error) instead of waiting for the sweep.
- If the SAN derivation ever changes, a migration clears every marker and this sweep reindexes everything.

**Full re-parse (backstop).**
- Once a day, re-derive every stored certificate's SAN rows and rewrite any that differ. This catches what the marker can't, such as SAN rows deleted by hand.

**Shared by both.**
- Read PEMs in one database call, parse them off the database thread, then write in a second call. The write is conditional on the PEM being the one that was parsed, so a concurrent certificate write wins.
- Finding anything to repair means a write path has a bug. Log it at error. No fault: an operator can do nothing about it, and the sweep has already repaired it.
- Name the loop's back-off, how errors are classified, and how it reports its exit (repo loop rule). A failed pass is logged and retried at the next interval, never abandoned.

## Write paths

- `insert_certificate`, `activate_pending_csr` and `activate_canopy_chain` write the SAN rows in the same transaction as the certificate. The two activations replace any rows already there.
- `delete_certificate` relies on the cascade.
- A write whose PEM cannot be parsed fails. "Could not work out the SANs" must not be stored as "covers nothing". Callers have already parsed the certificate for its metadata, so this only fires on a real defect.

## Migration

- New `version < N` migration with a `custom_run` that creates the table and backfills it by parsing every stored PEM.
- A row whose PEM will not parse gets no SAN rows and is logged once by the migration. Those rows are never served today either, and failing the migration would block the upgrade. `state::index_sans` goes on warning about active rows like this whenever a snapshot is loaded.

## Control plane (`state::Snapshot`)

The open comment on the card: `Coordinator::run` loads a snapshot per hostname per tick, and building one parses every active certificate.

- `Snapshot::load` reads the SAN table (candidates by key, plus self-issuance) instead of parsing every certificate. `find_active_for_hostname` and `staged_start_for_hostname` in `state.rs` take their candidates from it.
- The certificate the answer rests on (the winner, or the earliest staged cover) is confirmed against its PEM. Each parse is memoised for the life of the snapshot, so one certificate is parsed at most once per load.
- The snapshot ranks with the index's self-issuance, not the stored `self_signed` column, which is stale on older rows (`parse.rs`, `LeafFacts::self_issued`). After this the two matchers agree.
- Not covered: `Snapshot::load` still reads every certificate row, PEMs included. Parsing goes; the read stays. Stopping the read would mean loading the snapshot once per tick, which was considered and not taken.
- `env::runtime_holdings` keeps its cross-tick `LeafFactsCache`. It already parses each certificate once per PEM, not once per tick.

## Steps

- [x] Migration: table, index, backfill
- [x] Key derivation beside `san_covers`, with an agreement test
- [x] Write SAN rows from the three write paths, in their transactions
- [x] Serving lookup: candidates from the index, winner confirmed, reindex on contradiction
- [x] Snapshot: candidates from the index, confirmation memoised, self-issuance from the index
- [x] Marker column, partial index and `cert_pem` trigger (same migration)
- [x] Background task: unmarked-row sweep plus the occasional full re-parse, both with a conditional write-back
- [x] Tests: unknown hostname parses nothing; wildcard-only hostname; history rows ranked out; deleted certificate gone from the index; a contradicting index row is corrected and the next candidate served; backfill skips an unparseable row; a raw `cert_pem` update clears the marker and the sweep reindexes it; hand-deleted SAN rows restored by the full re-parse; a write landing mid-sweep isn't overwritten
- [x] Update the doc comments on `store::find_active_for_hostname` and `Snapshot::sans` to describe the index
