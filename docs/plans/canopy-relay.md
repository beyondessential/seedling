# Canopy relay

## Context

Seedling has no way to reach Canopy. It has no Canopy device identity, and
giving it one would mean a second enrolment per host and a second set of
credentials to rotate.

bestool already has that identity, and on a Seedling host bestool is already
connected: the Debian package makes bestool a hard dependency, generates
`/etc/bestool/seedling.key` on install, and authorises its fingerprint in
`/var/lib/seedling/authorized_keys`. bestool dials the OI as a fully-authorised
client. Separately, `bestool-canopy` now has a pluggable transport
(`CanopyTransport`, one method: `call(http::Request<Bytes>) ->
Result<http::Response<Bytes>>`), so the typed OpenAPI-generated client can sit
on top of any way of reaching Canopy.

Those two facts compose. bestool tells Seedling "Canopy is reachable through
me"; Seedling relays whole HTTP requests over the existing OI connection;
bestool issues them with its device identity and relays the responses back.
Seedling gets the full typed Canopy surface without an identity of its own.

This is the first step of the direction sketched at the top of `TODO.txt` —
reaching Canopy over a single connection, with an S3 proxy for Kopia and a
registry proxy for images as later candidates on the same transport.

## Locked-in decisions

These were settled in the design conversation that opened this plan. The rest
of the document treats them as constraints; to redirect any of them, edit this
section first.

1. **Generic HTTP proxy, no path allowlist.** The wire carries method, path,
   headers, and body; bestool executes it verbatim. Seedling therefore wields
   the host's full Canopy authority. This is not an escalation: bestool's OI
   key already holds full operator authority over Seedling, so the two are
   mutually trusting by construction.
2. **Seedling depends on `bestool-canopy`** and uses the generated typed
   methods, with an OI-backed `CanopyTransport` underneath.
3. **One stream per request.** Each relayed call rides its own
   Seedling-initiated bidirectional QUIC stream. No hand-rolled multiplexing
   over a single long-lived stream, and no pre-opened stream pool: QUIC already
   provides per-stream flow control, cancellation via `RESET_STREAM`, and
   framing by stream lifetime.
4. **Fail fast when no offer is live.** A relayed call with no registered
   provider is an immediate `Err`. Nothing is queued or replayed.
5. **Gate at offer acceptance, not at the consumer.** One setting turns the
   whole facility off, covering reporting today and the TLS and
   backup-credential consumers later.
6. **Absence is never a fault.** Seedling never dials out — bestool initiates
   both the connection and the offer — so a host without bestool has nothing
   running, nothing retrying, and nothing to report as broken. A fault is filed
   only when a heartbeat fails *while an offer is live*, which is a real
   malfunction rather than an absence.
7. **No `required` mode.** Detecting a host that ought to be reporting and
   isn't belongs in Canopy, which already tracks heartbeat staleness and can
   see the case where Seedling is down entirely — something a fault filed by
   Seedling structurally cannot.
8. **Seedling reports as its own Canopy source.** Canopy's `StatusPayload` has
   a `source` field and first-class support for `"seedling"` (per-source check
   policies, per-source issue scoping, per-source silencing). A source's push
   only opens and recovers its own checks, so Seedling and alertd do not
   contend.
9. **Fixed check names, not per-app.** Every check name Canopy sees enters an
   operator-facing catalog; per-app names would pollute it fleet-wide with
   every app anyone installs.
10. **Seedling does not send `tamanuVersion`.** That field sets the server's
    tracked version and alertd already sends it.

## Wire protocol

### Offer

Registration is an ordinary control request per `i[stream.control]` — no new
stream kind:

```json
→ {"method":"/canopy/offer","actor":{…},"params":{
     "agent":"bestool 0.7.7","endpoint":"https://meta.tamanu.app","via":"mtls"}}
← {"result":{"offer_id":"c1"}}
```

`agent` names the offering program, `endpoint` the Canopy base URL it will
reach, `via` is a free-form human-readable note on how it authenticates. All
three are for operator display only; Seedling makes no decisions from them.

The offer's lifetime is the connection's. `/canopy/withdraw { offer_id }` ends
it early.

Disabling Canopy revokes every live offer as well as refusing new ones, so the
setting takes effect immediately rather than at the next reconnect. Each
revoked offer emits `CanopyWithdrawn` with a reason distinguishing it from a
voluntary withdrawal, and in-flight relay streams for it are reset.

Re-enabling has to recover without a reconnect, since a healthy OI connection
may not reconnect for weeks. bestool therefore re-attempts the offer on a slow
timer whenever it is connected but unoffered, so an enable heals within one
retry interval. The attempt is a single cheap control request, which is why a
timer is preferred to having bestool watch the event feed for a setting change.

### Relay

One Seedling-initiated bidirectional stream per call, framed like
`i[stream.forward]` — a newline-terminated JSON header, then raw bytes:

```
→ {"canopy":"c1","method":"POST","path":"/status/abc","headers":{…}}\n<body>  finish()
← {"status":200,"headers":{…}}\n<body>  finish()
```

When bestool obtained no response from Canopy at all, the reply is an error
frame instead, and the stream is finished with no body:

```
← {"error":{"code":"unreachable","message":"…"}}\n  finish()
```

The split follows the `CanopyTransport` contract. A Canopy `403` is
`{"status":403}` carrying its body, because non-2xx statuses are the client's
to interpret — a `/backup-target` `412` means a dormant device. The `error`
frame is reserved for "no response exists" and is what becomes `Err`. Its codes
are `unknown_offer`, `unreachable`, and `invalid_request`.

Including `offer_id` in the request header lets a stream that races a withdrawal
be rejected with `unknown_offer` rather than silently executed against a
provider the operator has just revoked.

### Server-initiated stream dispatch

Seedling has never opened a bidirectional stream before, so bestool has no
accept-side convention to match. This work establishes one, symmetric to
`i[stream.dispatch]`: every server-initiated bidirectional stream begins with a
newline-terminated JSON object, dispatched on its key. `canopy` is the only key
today. A future push from Seedling to a connected client then needs no new
stream type.

### Bounds

Three, because an unbounded relay lets either side exhaust the other.

- **Response cap.** Seedling stops reading past a fixed ceiling of 16 MiB and
  resets the stream. `request()` uses 4 MiB today and status payloads are far
  under both. The wire stays streaming-capable — only `CanopyTransport`'s
  buffered signature forces a ceiling here, so raising it for a future
  streaming consumer is a Seedling-side change, not a wire change.
- **In-flight cap.** Relay streams are server-initiated, so they never reach
  `i[stream.concurrency-limit]`, which bounds client-initiated request streams.
  They get their own bound of 8 concurrent relayed calls; beyond that, a call
  waits rather than opening a stream. Seedling's own use is a single heartbeat
  on a timer, so the bound exists to contain a future consumer rather than to
  shape today's traffic.
- **Timeout.** Seedling resets a relay stream after 60 seconds, comfortably past
  `ReqwestTransport`'s own 30-second timeout, so bestool's timeout fires first
  and reports a real error rather than Seedling guessing at one.

## Spec changes

Write these before implementing, per the repo's spec-first rule.

`docs/spec/interface.md`:

- `i[stream.dispatch.server]` — server-initiated bidi streams open with a
  newline-terminated JSON object, dispatched on its key.
- `i[stream.canopy]` — relay stream framing, both directions.
- New `# Canopy Relay` section: `i[canopy.offer]`, `i[canopy.offer.lifetime]`,
  `i[canopy.offer.selection]` (most recently registered live offer wins),
  `i[canopy.offer.disabled]` (refuses new offers and revokes live ones),
  `i[canopy.withdraw]`, `i[canopy.relay]`,
  `i[canopy.relay.error]`, `i[canopy.relay.limits]`, `i[canopy.settings]`,
  `i[canopy.status]`, `i[canopy.request]`, `i[canopy.report.invoke]`.
- `i[wire.error-codes]` gains `canopy_disabled` and `canopy_unavailable`.
- `i[event.types]` gains `CanopyOffered` (`offer_id`, `agent`, `endpoint`) and
  `CanopyWithdrawn` (`offer_id`, `reason`).

`docs/spec/runtime.md`, new `# Canopy Reporting` section:

- `r[canopy.settings.enabled]` — durable, default enabled.
- `r[canopy.report.schedule]` — 60-second cadence, matching the cadence Canopy
  already receives from alertd.
- `r[canopy.report.identity]` — `server_id` resolved via `GET /servers/self`,
  cached durably, re-resolved on failure.
- `r[canopy.report.checks]` — the fixed check set.
- `r[canopy.report.extra]` — the free-form payload fields.
- `r[canopy.report.fault]` — `canopy_report_failed`, derived, cleared on the
  next success.

`docs/spec/web.md`: `w[canopy.page]`.

## Seedling implementation

**`crates/protocol`** — `canopy.rs` with the frame types (offer params, relay
request header, relay response header, error frame) and `OiClient::accept_bi()`.
This crate is published, so bestool consumes the same definitions rather than
hand-syncing a second copy.

**`crates/core/src/oi/canopy/`**

- `registry.rs` — offers keyed by id and indexed by connection, mirroring
  `oi/forwards/registry.rs`. `current()` returns the most recently registered
  live offer. `remove_by_conn()` for teardown.
- `stream.rs` — writes the request frame, reads the response frame, enforces the
  cap and timeout.
- `transport.rs` — `OiCanopyTransport`, implementing
  `bestool_canopy::CanopyTransport`. No live offer, or Canopy disabled, is
  `Err`.

**`crates/core/src/oi/handler/canopy.rs`** — `/canopy/offer`, `/canopy/withdraw`,
`/canopy/status`, `/canopy/settings/set`, `/canopy/request`, `/canopy/report`.

**`crates/core/src/oi/server.rs`** — drop the connection's offers in the
teardown path, next to the existing `forwards.remove_by_conn(conn_id)`.

**`crates/core/src/runtime/db.rs`** — a new `version < 53` block at the bottom
adding a `canopy_settings` singleton table and a cached `server_id`, following
the `tls_settings` shape. Never edit a shipped migration block.

**`crates/core/src/runtime/canopy.rs`** — the heartbeat task. Skips silently
when disabled or when no offer is live. Otherwise resolves `server_id`, builds
the payload, posts it, and files or clears `canopy_report_failed`.

Checks, all reported under `source: "seedling"`:

| Check | Passed | Warning | Failed |
|---|---|---|---|
| `health/apps` | all apps Running | any Degraded | any Faulted |
| `health/faults` | no active faults | — | any active fault |
| `health/proxy` | proxy running | — | stopped |
| `health/resolver` | resolver running | — | stopped |

Each entry carries the offending app or fault names as extra per-check fields,
which Canopy passes through verbatim to its status UI.

Free-form payload fields: `seedlingVersion`, `seedlingUptimeSecs` (the daemon's
own uptime, so an operator can see a restart), `appsTotal`, `appsByStatus`,
`activeOperations`, `activeFaults`. Not `hostname` or host uptime — bestool
already reports both. Not `tamanuVersion`.

**`crates/ctl/src/canopy.rs`** — `seedling-ctl canopy status | enable | disable
| report | request <METHOD> <PATH>`. `request` is the raw relayed call and
doubles as the end-to-end smoke test. `/canopy/offer` and `/canopy/withdraw` are
bestool-facing and get no CLI.

## Web UI

A `Canopy.tsx` route at `/canopy`, following `Registries.tsx` for shape: current
offer (agent, endpoint, via, since) or "no provider connected", last heartbeat
outcome, and an enable/disable control. Nothing more.

## bestool implementation

Work in a jj workspace to avoid colliding with other work in that repo.

**`crates/alertd/src/seedling.rs`** (new)

- A persistent OI connection with reconnect and backoff, using
  `/etc/bestool/seedling.key` rather than the per-user default path that
  `bestool-tamanu`'s `Oi::open` uses. `ClientIdentity::load_or_generate` already
  takes an explicit path.
- On connect, send `/canopy/offer`. A `canopy_disabled` rejection is logged once
  at info, not repeatedly — the operator has said no. Whenever connected but
  unoffered, re-attempt on a slow timer (5 minutes) so that re-enabling recovers
  without waiting for a reconnect.
- An `accept_bi` loop dispatching relay streams: parse the header, rebuild an
  `http::Request<Bytes>`, hand it to the `CanopyClient` the daemon already built
  at startup (`daemon.rs:90`), and write the response frame back. Bound the
  number handled concurrently.
- Withdraw cleanly on shutdown.

**`crates/tamanu/Cargo.toml`** — bump `seedling-protocol` to the release
carrying the frame types.

`bestool-canopy` itself needs no changes.

## Testing

- **protocol**: frame round-trips; malformed headers rejected. The crate has a
  fuzz target directory already.
- **core**: relay against an in-process offering client built on
  `oi/test_support.rs` — success, non-2xx passed through as a status rather than
  an error, each error-frame code, no offer, disabled, most-recent-offer-wins,
  teardown on connection close, response cap, timeout.
- **heartbeat**: payload shape and check derivation against a stub
  `CanopyTransport`, including fault filing and clearing.
- **bestool**: relay execution against a stub Canopy client; reconnect
  behaviour; offer rejection handled without a retry loop.

Annotate implementations and tests with tracey references throughout, placing
each `i[impl …]` / `r[impl …]` immediately before the code it describes rather
than at the top of the enclosing block.

## Deferred

- **`backup_now`.** The heartbeat response carries a list of backup types the
  server should run immediately. Seedling ignores it for now; wiring it to the
  backup app is separate work.
- **Other consumers.** TLS and DNS issuance through Canopy, and backup
  credentials, are the motivating cases from `TODO.txt` but are not in this
  work. The transport is built so they need no wire changes.
- **Streaming relay bodies.** The wire supports it; `CanopyTransport`'s buffered
  signature means nothing uses it yet. An S3 or registry proxy would want it.
