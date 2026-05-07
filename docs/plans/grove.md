# Grove (v0)

## Context

A "grove" is a group of seedling nodes — one leader, N followers — sharing a small set of leader-published settings. Every grove member eventually receives the leader's signed state, gossiped peer-to-peer over a new ALPN; no RAFT, no quorum, eventually consistent.

This v0 ships the smallest useful slice: the leader publishes typed **grove params** (name → value) inside a signed payload; each follower can independently map any local app param to a grove param, so when the grove value changes (or a mapping is created) the app param is updated through the same pathway as an operator `set_param`, firing `on_change`. While a mapping exists, manual local set/unset of that app param is rejected.

Out of scope for v0, but design must not foreclose:
- Replicating whole app definitions/configurations (incl. backup-app pattern from the original ask).
- Secret grove params with envelope encryption to per-member pubkeys.

Explicitly rejected (do not architect extension points for these; the design space is closed, not deferred):
- **Multi-grove membership per node.** The data model is single-row by design.
- **Grove-wide leader election / RAFT / failover.** No quorum, no election. If RAFT enters the picture later, the user's stated direction is that it would live *inside* a leadership group within a grove node, not span the whole grove — which is a different feature, not a generalisation of this one. v0 leader loss is a documented limitation with an out-of-band recovery path (see "Documented v0 limitations" below).

User-confirmed decisions:
- One grove per node.
- Reject local set on grove-mapped app params.
- Reuse the existing OI Ed25519 identity (`data_dir/oi.key`) as the leader's signing key. New nodes can be onboarded via *any* current member; the leader fingerprint is supplied out-of-band so the joiner can verify the signature without having to talk to the leader directly.
- Spec lives at `docs/spec/grove.md` with a new `g[...]` namespace, registered in `.config/tracey/config.styx`.
- Common transport considerations (RPK TLS, fingerprint pinning, ALPN as hard-version-wall, JSON-line framing, abort semantics) are extracted into a new shared `docs/spec/transport.md` with a `t[...]` namespace, referenced from both `docs/spec/interface.md` (`bes.seedling/1`) and `docs/spec/grove.md` (`bes.grove/1`). Avoids duplicating the same prose across protocols and pre-pays the cost for any future ALPN.

## Architecture

### Code structure — extract a `transport` module first

The code structure mirrors the spec split. Today `crates/core/src/oi/` holds both transport primitives (quinn endpoint construction, RPK TLS verifier, `TrustedKeys`, ALPN dispatch, JSON-line framing, per-connection state, fingerprint extraction) and OI-specific behaviour (port-forward, log streaming, event subscription, OI handlers). Grove would otherwise duplicate the transport half.

Refactor (lands before any grove-specific code):
- New `crates/core/src/transport/` module with `endpoint.rs` (quinn endpoint, listen + accept loop, ALPN-keyed handler registry), `auth.rs` (protocol-scoped trust sets — `OperatorTrust` and a registry of additional sets keyed by ALPN — and a verifier that consults the set matching the negotiated ALPN), `connection.rs` (per-connection registry, fingerprint extraction, abort frame), `framing.rs` (JSON-line read/write helpers, currently inline in `oi/server.rs`).
- Stays in `crates/core` rather than promoting to a new `crates/transport` crate for v0; promotion is a follow-up if multiple crates ever need server-side transport. The wire-type half (ALPN constants, dial-side `OiClient`, identity keys) remains in `crates/protocol/`.
- ALPN handlers register with `transport` on startup: OI registers `bes.seedling/1`, grove registers `bes.grove/1`. The existing `oi/server.rs::handle_connection` body splits — generic per-connection logic moves to `transport`, OI-specific stream dispatch stays in `oi/`.

For state, the smallest workable shape is to leave `OiState` alone and add `GroveState` as a sibling struct when grove lands (commit 5). The trust-set and fingerprint fields stay where they are: `OiState` owns the OI trust set, `GroveState` owns the grove trust set, and the `ProtocolTrustRegistry` (constructed at daemon startup and shared with both) is what couples them. Grove handlers capture `Arc<GroveState>` only; OI handlers continue capturing `Arc<OiState>`. The originally-planned `TransportState` / `OiState` carve-out + `Daemon` top-level container is **deferred** — it would touch every OI handler signature for limited cleanup value, and the sibling-struct approach gives the same handler-scoping property (operator handlers don't see grove state and vice versa) without the mechanical churn.

This refactor is pure restructuring — no behavioural change, OI tests stay green.

### Wire protocol — `bes.grove/1`

- New ALPN constant `GROVE_ALPN = b"bes.grove/1"` in `crates/protocol/src/lib.rs`.
- Registered with the `transport` module's ALPN handler registry (see "Code structure" below). The transport endpoint advertises both `bes.seedling/1` and `bes.grove/1`; on accept, the negotiated ALPN selects the registered handler. No second port.
- Trust-set is protocol-scoped (see "Trust-set reconciliation" below). The verifier consults `OperatorTrust` for `bes.seedling/1` and `GroveTrust` for `bes.grove/1`; a key authorised for both must be in both sets.
- Transport tuning is shared; the existing 10s keepalive / 30s idle is acceptable for grove gossip.

JSON-line framed bidi stream, using the framing helpers in `transport/framing.rs` (extracted in commit 0 from the existing `oi/server.rs::handle_bidi_stream` shape).

Messages:
- `hello`: `{type, grove_id, our_seq, our_payload_hash, our_role, our_fingerprint, protocol_version, nonce}`. `protocol_version` is independent of the ALPN (ALPN = hard wall, in-payload version = soft feature flag). `our_payload_hash` is sha-256 of the canonical signed bytes; same seq + different hash → fork → drop + emit `grove.payload-rejected`. Mismatched `grove_id` or unexpected leader → drop.
- `version`: `{type, payload, signature}`. Sender is whoever holds higher seq. Receiver validates signature against its pinned `leader_fingerprint`, requires strict `seq` increase, and bounds the message at 256 KiB on `recv.read_to_end`. Lower bound is "smaller than OI's 4 MiB" — grove payloads stay tiny in v0. Leader publish path **pre-checks** the canonicalised payload size before signing; if `grove invite`, `grove revoke`, or `grove param set` would push the next payload over the cap (e.g. ≥ 240 KiB to leave headroom), the OI handler / CLI / web returns a structured error (`grove.publish-rejected { reason: "payload_too_large", current_bytes, cap_bytes }`) without bumping seq or persisting. Operators get the feedback at the point of mutation, not via a peer's `abort` after-the-fact.
- `peers`: `{type, entries: [{fingerprint, addresses, last_seen}]}`. Address-hint gossip only — *not* membership. On receive: filter to `members` from latest signed payload (drop entries from non-members), cap entries (e.g. 64) and addresses-per-entry (e.g. 4), treat peer-supplied `last_seen` as a tie-breaker, never as authoritative liveness.
- `abort`: `{type, reason}` where `reason ∈ {grove_mismatch, signature_invalid, seq_regression, leader_mismatch, version_too_old, payload_too_large, ...}`. Sent before close on protocol-level rejection so the operator gets a non-network reason via the event surface.

Full-state-only for v0; revisit deltas when payloads might exceed a few MiB (i.e. when app definitions are replicated).

### Signed payload

Ed25519 signature over **JSON canonicalised with `serde_jcs`** (RFC 8785). JSON is idiomatic in this repo (used by OI, events, CLI), and a canonical JSON form keeps the schema flexible for an early-stage feature — adding or reordering fields in v1 doesn't break the wire encoding.

Payload is a `serde`-derived struct serialising to canonical JSON:

```jsonc
{
  "grove_id": "<uuidv4>",          // generated at `grove init`
  "seq": 42,                        // u64, monotonic, leader-managed
  "created_at": "2026-05-07T12:34:56Z",
  "leader_fp": "<sha256-hex>",      // of leader SPKI
  "members": [                      // sorted by fp at serialise time
    { "fp": "<hex>", "label": "..." }
  ],
  "params": [                       // sorted by name at serialise time
    { "name": "...", "kind": "text", "value": "..." }
  ],
  "secrets": []                     // v0: always empty. v1 adds envelope-encrypted entries; no schema break.
}
```

Bytes actually signed are `domain_sep || canonical_json_bytes` where `domain_sep = b"bes.grove/sig/v1\0"`. Domain separation is non-negotiable — Ed25519 signatures without it get confused across protocols.

Encoder, signer, and verifier live in a new `crates/protocol/src/grove.rs` (alongside `keys.rs`) so `seedling-ctl` can verify during `grove join` without depending on `core`. Add `serde_jcs` as a dependency on `crates/protocol`.

`created_at` validation: receive-time skew check of ±5 minutes. Reject outside window with `version_too_old`. Don't include unvalidated fields inside a signed envelope.

### Database — migration v53

All five tables added in one migration block at the bottom of `crates/core/src/runtime/db.rs`. Never edit shipped migrations.

- `grove_membership(id INTEGER PRIMARY KEY CHECK(id=1), grove_id BLOB NOT NULL, role TEXT NOT NULL CHECK(role IN ('leader','follower')), leader_fingerprint TEXT NOT NULL, current_seq INTEGER NOT NULL, current_payload BLOB NOT NULL, current_signature BLOB NOT NULL, joined_at TEXT NOT NULL)` — single-row.
- `grove_peers(fingerprint TEXT PRIMARY KEY, label TEXT, addresses_json TEXT NOT NULL, last_seen_at TEXT, last_connected_at TEXT)`.
- `grove_params(name TEXT PRIMARY KEY, kind TEXT NOT NULL, value TEXT NOT NULL, version_seq INTEGER NOT NULL)` — denormalised current params for query convenience.
- `grove_param_mappings(app_name TEXT NOT NULL, app_param_name TEXT NOT NULL, grove_param_name TEXT NOT NULL, PRIMARY KEY(app_name, app_param_name))` — local-only, never replicated.
- `grove_versions(seq INTEGER PRIMARY KEY, payload BLOB NOT NULL, signature BLOB NOT NULL, received_at TEXT NOT NULL)` — historical payloads for replay/debug. UNIQUE on `seq` makes duplicate-apply idempotent (`INSERT OR IGNORE` then check `changes()`).

All mutations on the leader (publish path) wrap `(load current → mutate → bump seq → sign → persist payload + bump grove_membership)` in one `db.call` closure under a single `parking_lot::Mutex<()>` named `grove_publish_mutex` on `GroveState` (held by `Daemon`, see "Code structure").

### Trust-set reconciliation (load-bearing)

After commit 0 the trust-set machinery lives in `transport/auth.rs` as a protocol-scoped registry, keyed by ALPN. Two strictly-separate sets, neither merged into a global "anyone trusted":
- **Operator-derived** (`OperatorTrust`): from `data_dir/authorized_keys` (loader stays in `oi/auth.rs`). Authorises *only* the OI ALPN (`bes.seedling/1`).
- **Grove-derived** (`GroveTrust`): from the latest signed payload's `members`. Authorises *only* the grove ALPN (`bes.grove/1`). Reconciled on every payload-applied event — additions and revocations both.

A key authorised for both surfaces must appear in *both* sets (operator key on the leader's machine, also a grove member: explicit on both sides). Grove membership grants no operator authority and vice versa.

The verifier in `transport/auth.rs` takes the negotiated ALPN and checks only the corresponding set. Constant-time comparison still applies, but only over the set relevant to the protocol being negotiated.

On revocation, iterate the per-connection registry on `TransportState` and close any grove connection with `client_fp == revoked_fp`. Operator connections from the same fingerprint (if the key is also in `OperatorTrust`) are *not* closed — operator authority is a distinct grant. Without per-protocol close, a revoked member retains an existing grove connection until idle-timeout and can keep gossiping.

### Operations surface

CLI, OI handler, web — all three for parity. CLI subcommand tree under `crates/ctl/src/grove.rs`:

Leader-only mutations (handler rejects on follower with "not the leader; current leader is <fp>", regardless of which surface invoked it):
- `grove init` — generate `grove_id`, persist self-only seq=1 payload.
- `grove invite <fingerprint> <label>` — append to members, seq++, sign, publish.
- `grove revoke <fingerprint>` — remove from members, seq++, sign, publish, close existing connections.
- `grove param set <name> <kind> <value>` and `grove param unset <name>` — modify params, seq++.

Any-node:
- `grove status` — id, role, leader_fp, current_seq, member count, count of currently-connected peers.
- `grove peers` — every known peer (members from latest payload + address-hint entries) with: fingerprint, label, currently-connected (yes/no), last_connected_at, last_seen_at, known addresses. CLI table form + OI structured response + web table. Backed by joining the per-connection registry on `OiState` against `grove_peers` rows.
- `grove members`, `grove params`.
- `grove join <inviter_addr> <inviter_fp> <leader_fp>` — dial inviter (need not be leader), pull latest payload via an inline `version` request, verify against `leader_fp`, persist. After persist, attempt to reach all members listed in the payload.
- `grove map <app> <app_param> <grove_param>` — create local mapping. Validates: app + param exist, no existing mapping for `(app, app_param)`, grove param value passes app param's validator (string-coerce by app param's kind). On success, immediately apply current grove value via the synthetic-grove `set_param` path.
- `grove unmap <app> <app_param>` — remove mapping; app param keeps current value (no automatic revert).

### Reject-local-set + actuation pathway

Refactor `crates/core/src/oi/handler/params.rs::set_param`/`unset_param`:
- Extract internals into `set_param_inner(...args, source: ParamSource)` where `ParamSource = Operator | Grove { seq: u64 }`.
- Public OI handler path: enforce mapping rejection (look up `grove_param_mappings`, return error if mapped). Actor remains the operator's.
- Grove apply path (new helper `apply_grove_param_to_app(state, app, app_param, grove_value)`): synthesises an actor with `kind = Some("grove")`, `id = Some(grove_id_str)`, `display = Some("grove:<seq>")`. Calls `set_param_inner` with `source = Grove { seq }`, bypassing the rejection check.

The barrier/replay system stays grove-unaware; from its point of view a grove-driven `param_set` is identical to an operator one. The actor `kind = "grove"` differentiates in audit/event surfaces.

### Diff application

When a new payload is persisted (idempotent on `grove_versions` UNIQUE):
- Compute `(prev_params, new_params)` diff.
- For each grove param G that changed or was added: look up `grove_param_mappings WHERE grove_param_name = G`; for each `(app, app_param)`, call `apply_grove_param_to_app`.
- For each grove param G removed: same lookup, call `unset_param_inner` with grove source — app param reverts to its declared default through the standard pathway. (`set_param`'s existing "identical-value → not_scheduled" behaviour applies here naturally; `on_change` won't fire if the mapped app already had that value.)

In-flight-op handling: `set_param_inner` already returns `OperationInProgress` when an app is e.g. installing. v0 mirrors operator semantics — the diff application logs the failure as a `grove.param-deferred` event and adds `(app, app_param)` to a `dirty_grove_mappings: HashSet<(AppName, ParamName)>` on `OiState`, drained by an existing reconcile pass when the app phase becomes `Installed`. (Also drained on next `version`-applied for the same grove param.) This handles the case without a heavy queue or pending-state machine.

`grove map` mid-operation: rejects with `OperationInProgress`, mirrors operator semantics. Mapping is *not* persisted; operator retries.

### Connect loop

Single actor on `GroveState`: wakes every ~30s, picks K=4 peers (all stale `last_connected_at` first, then random), dials each over `bes.grove/1`, runs HELLO → optional VERSION (whichever side has lower seq receives) → PEERS → close. Inbound piggybacks on the OI accept loop, dispatched by negotiated ALPN.

### Event surface

New family in `crates/protocol/src/events.rs` (or a sibling `grove_events.rs`):
- `grove.payload-received { peer_fp, seq }`
- `grove.payload-applied { seq, params_changed, members_changed }`
- `grove.payload-rejected { peer_fp, reason }`
- `grove.publish-rejected { reason, current_bytes?, cap_bytes? }` — leader-side, before bumping seq
- `grove.peer-connected { fp }`, `grove.peer-disconnected { fp }`
- `grove.member-revoked { fp }`
- `grove.mapping-created { app, app_param, grove_param }`
- `grove.mapping-removed { app, app_param }`
- `grove.param-applied { app, app_param, grove_param, source: "grove", seq }`
- `grove.param-deferred { app, app_param, reason: "operation_in_progress" }`

Without these the web UI is unusable; plan them up front in the spec commit.

### Documented v0 limitations

In `docs/spec/grove.md` "Known limitations" section:
- **Lost leader key bricks the grove.** No quorum, no election. Recovery path (not implemented in v0): any follower runs `grove fork-leader --force --new-leader-key=<fp>` to write a new self-only payload and break the chain, all other followers re-join. Document the recovery shape so users don't lose data.
- **No secret grove params.** `grove param set --secret` is rejected at v0 definition time. Wire format already reserves a `secrets` field for v1 envelope encryption (Ed25519 → X25519 via `ed25519-dalek` for ECIES per member), so v1 adoption needs no payload-version bump.
- **No multi-grove.** Schema asserts single-row `grove_membership`. Multi-grove would change the schema and most table PKs.

## Critical files

To modify:
- `crates/protocol/src/lib.rs` — add `GROVE_ALPN`.
- `crates/protocol/src/keys.rs` — Ed25519 sign/verify already there; reused.
- `crates/protocol/src/grove.rs` — *new*. Payload + message types (serde-derived), `serde_jcs` canonical encoder, Ed25519 sign/verify with domain separation. Add `serde_jcs` to `crates/protocol/Cargo.toml`.
- `crates/protocol/src/events.rs` — extend with grove events.
- `crates/core/src/runtime/db.rs` — add v53 migration block at the bottom; never edit existing.
- `crates/core/src/transport/` — *new* module (commit 0). `endpoint.rs`, `auth.rs`, `connection.rs`, `framing.rs`. Holds quinn endpoint, ALPN handler registry, protocol-scoped trust registry, JSON-line framing. Replaces transport-level code currently inlined in `oi/server.rs` and `oi/auth.rs`.
- `crates/core/src/oi/server.rs` — slimmed to OI-specific stream dispatch (port-forward, log streaming, event subscription, handler routing). Generic per-connection logic moves to `transport/`.
- `crates/core/src/oi/auth.rs` — bootstrap-file loader stays here; trust-set machinery moves to `transport/auth.rs` as protocol-scoped sets (`OperatorTrust` for OI, `GroveTrust` for grove, registered against their ALPNs).
- `crates/core/src/grove/` — *new* module. `state.rs` (`GroveState`: publish mutex, dial loop, dirty-mappings, signed-payload cache, grove trust set), `dial.rs` (connect loop), `apply.rs` (payload-apply pipeline + diff), `handler.rs` (registers `bes.grove/1` handler with transport, runs HELLO/VERSION/PEERS/abort exchange).
- `crates/daemon/src/main.rs` — at startup, construct the shared `ProtocolTrustRegistry` and `AlpnHandlers`, then call `oi::register` and `grove::register` against them before `transport::endpoint::run`. No new top-level container struct; both states are held as `Arc<OiState>` and `Arc<GroveState>` siblings, captured into their respective handler closures.
- `crates/core/src/oi/handler/params.rs` — extract `set_param_inner(... ParamSource)`, add `apply_grove_param_to_app` helper, rejection check on operator path.
- `crates/core/src/oi/handler/grove.rs` — *new*. OI handlers for all `grove …` operations.
- `crates/ctl/src/grove.rs` — *new*. CLI subcommand.
- `crates/ctl/src/main.rs` — register `grove` subcommand.
- `crates/web/frontend/src/routes/Grove.tsx` and friends — *new*. Status, members, params, mappings UI.
- `crates/web/src/api/grove.rs` (or wherever the web API surface lives) — corresponding endpoints.
- `docs/spec/transport.md` — *new*. `t[...]` items: RPK TLS handshake, fingerprint pinning, ALPN as hard-version-wall, JSON-line stream framing, abort/error semantics, message-size caps. Extracted from interface-spec content that currently describes these for `bes.seedling/1`.
- `docs/spec/interface.md` — replace duplicated transport prose with references to `t[...]` items; keep OI-specific behaviour (handler routing, port-forward, event subscription, log streaming) under `i[...]`.
- `docs/spec/grove.md` — *new*. `g[...]` items, referencing `t[...]` for transport-shared bits.
- `.config/tracey/config.styx` — register `transport/main` and `grove/main`.

To reuse (don't reinvent):
- `quinn::Endpoint` + RPK TLS — after commit 0, owned by the `transport` module; both OI and grove register handlers, never a second port.
- `SeedlingClientVerifier` and the trust sets — extracted in commit 0 to protocol-scoped form; grove registers `GroveTrust` against `bes.grove/1`, doesn't fork the verifier.
- Ed25519 from `crates/protocol/src/keys.rs` — reuse the OI identity for grove signing.
- `ClientAuth::Fingerprint` + `OiClient::connect` (`crates/protocol/src/client.rs`) — model grove dial on the same shape; consider extracting common dial logic to `transport/dial.rs` if both call sites converge enough.
- Generation + barrier/replay (`crates/core/src/runtime/generations.rs`, `runtime/apps/replay.rs`) — grove apply flows through the *unchanged* operator pathway. Barrier doesn't learn about grove.
- `Actor` struct already supports a free-form `kind` string; introduce `"grove"` without a new enum variant.

## Phasing — stacked commits

Each independently shippable, each with its own spec sections + tracey annotations. jj-friendly granularity.

0. **Code restructure: extract `transport` module.** Pure refactor, no behavioural change, OI tests stay green. New `crates/core/src/transport/` (`endpoint`, `auth`); slim `oi/server.rs` and `oi/auth.rs` to OI-specific bits; OI registers itself as the `bes.seedling/1` ALPN handler through the new transport interface. Trust-set machinery becomes protocol-scoped (`ProtocolTrustRegistry` keyed by ALPN) but only OI is registered at this point. `OiState` is left alone — `GroveState` will land as a sibling in commit 5 rather than via a carve-out. *Done in `refactor(transport): extract shared QUIC endpoint, ALPN dispatch, and protocol-scoped trust`.*

1. **Spec + tracey wiring.** Extract shared transport prose from `docs/spec/interface.md` into a new `docs/spec/transport.md` (`t[...]` namespace) covering RPK TLS, fingerprint pinning, ALPN as hard-version-wall, JSON-line framing, abort/error semantics, message-size caps. Update `interface.md` to reference `t[...]` for those items. Re-annotate the (now-moved) transport code from commit 0 with `t[impl ...]` instead of `i[impl ...]`. Add `docs/spec/grove.md` skeleton with all `g[...]` items including event names and "Known limitations", referencing `t[...]` for transport. Register `transport/main` and `grove/main` in `.config/tracey/config.styx`. `tracey query status` clean with stubbed requirements.
2. **DB v53 migration.** All five tables, no consumers yet. Test: migration applies cleanly on a v52 DB.
3. **Signing primitives.** `crates/protocol/src/grove.rs` — payload + message types, `serde_jcs` canonical JSON encoder, Ed25519 sign/verify with `bes.grove/sig/v1` domain separation. Pure functions. Heavy KAT tests, including domain-separation regressions and JCS canonicalisation determinism (round-trip, key ordering, whitespace).
4. **Leader-side state machine, OI surface only.** `grove init`, `grove invite`, `grove revoke`, `grove param set`/`unset` writing the signed payload to v53 tables. Pre-publish payload-size cap check returning structured `publish-rejected` errors before bumping seq. No networking. Tests: seq monotonicity, signature roundtrip, single-writer mutex under concurrent writes, rejection on follower role, size-cap enforcement.
5. **Grove ALPN registration + protocol exchange.** Register `bes.grove/1` against the transport handler registry from commit 0. HELLO/VERSION/PEERS/abort exchange, outbound dial loop, inbound handler. Add `GroveTrust` to the protocol trust registry (alongside `OperatorTrust` from commit 0). Persist payloads, verify signatures, reconcile `GroveTrust` on payload-applied. No param actuation yet.
6. **Onboarding.** `grove join` end-to-end. Test with two ephemeral daemons in-process.
7. **Mapping + actuation.** `grove_param_mappings`, `grove map`/`unmap`, reject-local-set, `set_param_inner` refactor, `apply_grove_param_to_app`, removal-→-unset, `dirty_grove_mappings` drain on app-idle. Tests: mapping creates apply current value, grove updates fire `on_change`, local set is rejected when mapped, grove-removal unsets mapped param, in-flight ops defer correctly.
8. **Web + CLI parity, event surface.** `crates/ctl/src/grove.rs` subcommand, web pages and API endpoints, all event variants emitted. Tracey coverage closing.

## Verification

End-to-end test plan, executed manually after commit 8 lands and re-run on changes:

1. Two daemons (leader L, follower F) in `cargo run -p seedling-cli` on different data dirs, behind `tracey query status` clean before starting.
2. On L: `grove init`. Verify `grove status` shows `role=leader, seq=1, members=1`.
3. Manual fingerprint exchange: capture `oi.key` fingerprints on both nodes (existing OI tooling).
4. On L: `grove invite <F_fp> "node-f"`. Verify `seq=2`, members=2.
5. On F: `grove join <L_addr> <L_fp> <L_fp>`. Verify F's `grove status` shows `role=follower, leader=<L_fp>, seq=2`.
6. On L: `grove param set greeting text "hello"`. Verify `seq=3` on L, then within ~30s on F.
7. On F: register a small BSL app with a param `welcome` that has an `on_change` handler logging via `tracing::info!`. Then `grove map <app> welcome greeting`. Verify F's `apps param get <app> welcome` returns "hello", and the `on_change` log line fired.
8. On F: `apps param set <app> welcome "boom"` — assert error "param is grove-managed".
9. On L: `grove param set greeting text "kia ora"`. Within ~30s, verify F's `apps param get <app> welcome` is "kia ora" and `on_change` re-fired.
10. On L: `grove param unset greeting`. Verify F's app param reverts to its declared default and `on_change` re-fired.
11. On F: `grove unmap <app> welcome`. Then `apps param set <app> welcome "manual"` — should now succeed.
12. On L and F: `grove peers` — verify each shows the other as currently-connected with a recent `last_connected_at`. Stop F's daemon, wait, re-run `grove peers` on L: F appears as not-connected with the previous `last_connected_at`.
13. Restart F. On L: try `grove param set` with a value chosen to push the canonical payload past the cap. Verify the OI/CLI rejects with `payload_too_large` and the seq did not advance.
14. On L: `grove revoke <F_fp>`. Verify L's connection registry shows the F-fp grove connection severed; F's subsequent grove dials get TLS-rejected. If F's fingerprint is *also* in `OperatorTrust`, OI access from F is *not* affected.
15. `tracey query status` still clean, all `g[...]` items covered.

Run: `just test` for the unit/integration tier; `cargo clippy --all-targets`, `cargo fmt --check`, `tracey query status` before each commit.
