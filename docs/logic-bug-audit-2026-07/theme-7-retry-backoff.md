# Theme 7: Retry logic that either hammers or gives up forever

> Companion to the [logic bug audit](../logic-bug-audit-2026-07.md), cross-cutting theme 7.

## The failure pattern

Every retry site in the codebase needs two things: a **back-off** (so failure does not
mean hammering) and a **recovery path** (so failure does not mean death). The audited
instances each have at most one of the two:

- `Actuator::ensure_image_available` (`crates/core/src/system/actuator/pull.rs`) retries
  a failed pull immediately on the next 5 s reconciler tick (`Some(state) if !state.in_flight => true`,
  comment: "retry immediately"), then after `MAX_PULL_ATTEMPTS = 5` sets
  `PullState::exhausted = true` — a terminal state nothing ever clears, since `pulling`
  entries are only removed on pull success, which can no longer happen. No back-off,
  then no recovery.
- `Coordinator::run` (`crates/core/src/runtime/tls/issuance.rs:309`) dispatches
  Tailscale-discovered hostnames to `run_tailscale` **before** loading the unified
  `state::compute_state` decision, so `Decision::Blocked` and `Decision::Debounced`
  (the `AUTO_RETRY_DEBOUNCE_SECS = 3600` failure debounce in `tls/state.rs`) are never
  consulted. While tailscaled is down, every 5 s tick opens and finalises a failed
  `tls_cert_attempts` row; the `Snapshot::load` cap of 1000 attempt rows then evicts
  other hostnames' `last_attempt`, dissolving *their* debounce too. No back-off at all —
  and the debounce state that does exist for ACME hostnames is collateral damage.
- `udp_relay_task` (`crates/core/src/oi/forwards/session.rs`) matches
  `Ok(n) if n > 0` on `socket.recv` and hits `_ => break` for both `Ok(0)` (a legal
  zero-length datagram) and `Err(_)` (ICMP port-unreachable surfacing as `ECONNREFUSED`
  on a connected socket). One transient error kills the relay forever; the forward stays
  registered and listed as healthy. No error classification, no recovery, no report.
- The ctl UDP forward loop (`crates/ctl/src/forward.rs:273`) does
  `if client.send_datagram(pkt).is_err() { break; }`, so quinn's recoverable
  `SendDatagramError::TooLarge` (any datagram over path MTU: EDNS, QUIC payloads) is
  treated the same as `ConnectionLost`. The server-side relay in `session.rs` already
  gets this right — it drops-and-reports on oversize and only breaks on
  `ConnectionLost` — so the two ends of the same protocol disagree.
- The web event broker consumer (`crates/web/src/event_broker.rs::run_event_broker`)
  has a correct reconnect loop with capped exponential back-off, but it is defeated by
  the §1 `subscribe_events` bug: an error response leaves `stream_events` blocked
  forever inside `accept_uni()`. A loop that never regains control cannot retry.

Two distinct shapes hide under one theme:

- **(a) Tick-driven retries.** A reconciler calls the site every ~5 s
  (`crates/daemon/src/main.rs:1151`). The retry "loop" is the tick itself; what is
  missing is a per-key *decision* — "should I attempt now?" — with capped exponential
  delay, automatic reset on success, and never a terminal state without an expiry or an
  operator-visible escape hatch. `pull.rs` and the Tailscale issuance path are this shape.
- **(b) Long-lived task loops.** A relay or subscription owns its own `loop`. What is
  missing is *error classification*: transient errors must continue (with delay where
  retrying an operation, without where merely skipping a datagram), only fatal errors
  may exit, and every exit must be reported to whoever still believes the task is alive.
  `udp_relay_task`, `forward.rs`, and the event-broker consumers are this shape.

## Affected findings

| Finding | Section | Severity |
|---|---|---|
| Image pull retries have no back-off and exhaustion is permanent (H3) | [§13](../logic-bug-audit-2026-07.md#13-system-actuation-actuator-observer-breadcrumb-journal-stub-types) | high |
| Tailscale issuance path bypasses retry blocks and failure debounce, retrying every reconciler tick (H10) | [§9](../logic-bug-audit-2026-07.md#9-runtime-tls-identity-secrets) | high |
| UDP relay task dies permanently on transient socket error or zero-length datagram (H21) | [§3](../logic-bug-audit-2026-07.md#3-oi-server-auth-forwards-shells) | high |
| UDP port forward terminates permanently on a single oversized datagram (H15) | [§16](../logic-bug-audit-2026-07.md#16-daemon-and-ctl-crates) | high |

Adjacent instances of the same discipline failure, catalogued under other themes:
`subscribe_events` swallows errors then blocks forever, defeating the broker's back-off
loop (H16, [§1](../logic-bug-audit-2026-07.md#1-protocol-crate-cratesprotocol), high),
and lagged event subscribers silently lose events with no gap signal or resync
([§17](../logic-bug-audit-2026-07.md#17-web-crate), medium).

## Would a high-level change help?

**Shape (a): yes, decisively.** The codebase already contains the correct pattern,
written twice, and both bugs are sites that failed to use it:

- `should_back_off` in `crates/core/src/runtime/scheduler.rs` (spec
  `r[impl history.operations.rate-limiting]`): capped exponential
  (`BASE 5 s × 2^(n-1)`, cap 300 s), automatic reset when the gap since the last
  matching operation exceeds the cap, thoroughly unit-tested in
  `scheduler/tests.rs` including the overflow case. No terminal state exists at all.
- The TLS `Decision` machine in `tls/state.rs` is a DB-persisted version of the same
  idea (fixed 1 h debounce rather than exponential, plus operator blocks and
  force-retry). The Tailscale bug is precisely a code path that routes around the
  single decision function.

So the high-level fix for shape (a) is not new machinery: (1) extract the
`should_back_off` shape into a reusable per-key gate so `pull.rs` stops hand-rolling
`in_flight`/`exhausted` booleans, and (2) adopt the rule that a subsystem with a
central decision function (`compute_state`) admits **no dispatch before the decision**
— `dispatch_to_tailscale` must run after, or `run_tailscale` must receive the
`Decision`.

**Shape (b): yes, but as a discipline, not a type.** A back-off value alone would not
have prevented H21 or H15 — those bugs are misclassification (`Ok(0)` and `TooLarge`
lumped in with `ConnectionLost`), plus silent exits (the relay's `status_tx` is simply
dropped). The good in-repo exemplar is `crates/ctl/src/subscribe.rs`
(`i[impl ctl.subscribe.reconnect]`): classified outcomes
(`SessionOutcome::GracefulClose / Error / Interrupted`), capped back-off, reset on
success, and a bounded overall deadline. Shape (b) needs that structure imposed on
every relay/consumer loop, with the gate type merely supplying the delay when the loop
retries an operation (reconnect) rather than skipping a unit of work (one datagram).

Verdict: **one decision type plus one loop discipline.** A single "retry helper" that
wraps a future (the `backoff`-crate model) fits neither shape well: tick-driven sites
have no future to wrap (the reconciler is the loop), and relay loops need per-error
classification mid-loop, not whole-operation retry.

## Proposed pattern

**1. A `RetryGate` type** (~50 lines plus tests), generalising `should_back_off`:

```rust
pub struct RetryGate {
    base: Duration,          // e.g. 5 s
    cap: Duration,           // e.g. 300 s; also the staleness-reset horizon
    failures: u32,
    last_failure: Option<Instant>,
}

impl RetryGate {
    pub fn should_attempt(&self, now: Instant) -> bool;
    pub fn record_failure(&mut self, now: Instant);  // failures saturating += 1
    pub fn record_success(&mut self);                // full reset
    pub fn reset(&mut self);                         // operator force-retry path
}
```

Semantics: `should_attempt` is true when there is no failure history, when
`now - last_failure >= min(base × 2^(failures-1), cap)`, or when the gap exceeds `cap`
(staleness auto-reset, exactly as `should_back_off` does). There is **no `exhausted`
variant**: past a threshold (`failures >= N`) the caller files an operator-visible
fault (`image_pull_failed`) and keeps retrying at the cap interval. Permanent stop is
only permissible behind either an expiry or an explicit operator action with a manual
reset path (the TLS retry-block + `store::set_force_retry` pair is the model).
Per-key use is a `HashMap<String, RetryGate>` — for `pull.rs`, replacing the
`in_flight`/`exhausted` fields of `PullState` while keeping `in_flight` and the
`PULL_STALE_TIMEOUT` stuck-task recovery.

**2. A loop discipline for shape (b)**, anchored by a classification enum:

```rust
enum StepOutcome { Continue, Retry(reason), Fatal(reason) }
```

Rules: `Continue` for per-unit skips (oversize datagram: drop, count, report via
`status_tx` / stats — the server relay's existing oversize arm is the template);
`Retry` for transient operation errors (`ECONNREFUSED` on connected-UDP `recv`,
stream loss), delayed by a `RetryGate`; `Fatal` only for unrecoverable conditions
(`SendDatagramError::ConnectionLost`, channel closed) — and every `Fatal` exit must
tear down or report state so nothing keeps advertising the dead task (`udp_relay_task`
must emit a `forward.status` and deregister; today it silently drops `status_tx`).
Corollary: no unbounded await inside the loop body — H16 shows a hang defeats even a
correct loop.

**Dependency question.** The obvious crates do not fit: `backoff` is unmaintained;
`exponential-backoff` and `backon` model "retry this future/closure", which matches
neither the tick-driven gate (no future to wrap) nor mid-loop classification. Given the
repo already owns a tested implementation of the exact semantics (`should_back_off`),
the recommendation is **in-house `RetryGate`**, extracted rather than written fresh.
This runs against the stated preference for small dependencies over reimplementation,
so per AGENTS.md: **user to confirm**, particularly the crate placement — the gate is
needed from `core` (actuator, forwards), `ctl`, and `web`, so it lands either in
`seedling-protocol` (already a shared dependency) or a new tiny workspace crate.

## What it prevents — and what it does not

Prevents: retry storms against tick cadence (pull hammering, tailscaled hammering and
the attempt-row flood that evicts other hostnames' debounce); permanent-death states
with no recovery (`exhausted`, dead relays that still appear in `/forwards/list`);
divergent hand-rolled retry state per site; and the silent-exit variant, since the
discipline makes exit reporting a named obligation.

Does not prevent: misclassification itself — someone must still decide that `Ok(0)` is
a legal datagram and `TooLarge` is per-packet, and a wrong call puts a transient error
in the `Fatal` arm regardless of machinery. Nor does it fix bypass-the-decision bugs
(H10 is a dispatch-ordering fault; the gate existed and was skipped), hangs upstream of
the loop (H16), or policy questions like whether the 1000-row `Snapshot::load` attempt
cap should become per-hostname-latest (it should, independently).

## Migration path

1. Extract `RetryGate` from `should_back_off`'s logic; port `scheduler.rs` call sites
   or leave `should_back_off` as a thin adaptor over persisted op history.
2. `pull.rs`: replace `exhausted` with a gate; after 5 failures file
   `image_pull_failed` (already spec'd to clear on subsequent success,
   `runtime.md:1074-1075`) and keep attempting at the cap. Spec first: `interface.md:786`
   already promises "retries and back-off".
3. `issuance.rs`: move `dispatch_to_tailscale` below the `compute_state` load so
   Tailscale hostnames honour `Blocked`/`Debounced`; fix the snapshot attempt cap.
4. `session.rs::udp_relay_task`: classify `recv` outcomes; report and deregister on exit.
5. `forward.rs`: honour `max_udp_payload` / drop-and-report on `TooLarge` (spec
   `i[forward.mtu]`); align with the server relay's arms.
6. Sweep remaining loops (`web/main.rs:151`, `event_broker.rs`,
   `site_services/resolver.rs`) — mostly already conformant; align them on the shared
   type where it simplifies.

## Enforcement

- **Gate unit tests** with `tokio::time::pause`: delay doubles per failure, caps,
  auto-resets after the staleness horizon, `record_success` resets fully — mirroring
  the existing `scheduler/tests.rs` suite (including the `2^99` overflow case).
- **Stub-driver tests**: a `pull_image` stub failing N times then succeeding must end
  with the image pulled and the fault cleared (kills the `exhausted` regression);
  `udp_relay_task` against a closed local UDP port must survive `ECONNREFUSED` and emit
  status.
- **Tracey spec items** (docs/spec) stating requirements, not mechanisms: "a transient
  pull failure must not permanently disable actuation of the workload"; "background
  issuance must respect operator retry blocks and the failure debounce regardless of
  provider"; "a forward whose relay has terminated must not be reported as active";
  annotated `r[impl ...]` at the gate call sites per the annotation guide.
- **Review checklist**: every `loop` containing a fallible await names (i) its back-off
  source, (ii) its transient/fatal classification, and (iii) its exit-reporting path;
  any terminal give-up state names its expiry or its operator reset path. "Retry
  immediately" and bare `_ => break` are the tells this audit found four times.
