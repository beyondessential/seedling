# Restart accounting

Seedling has no record of individual container restarts. `r[autonomous.restart.start-limit-hit]` reacts only to the terminal state — systemd has refused to retry — so the visible states are "fine" and "systemd gave up". A container that crashes twice a day forever, never exhausting `StartLimitBurst` inside its window, is silent: no fault, no history, nothing an operator can query. Sub-threshold flapping is the common real-world shape and it is invisible.

The fix is to record restart attempts in the database, and to derive `crash_loop` from the recorded rate rather than from systemd's internal accounting. systemd keeps actioning restarts on Linux; seedling keeps the books.

## Why Linux first

The Windows container runtime has no systemd equivalent — containerd has no restart policy — so seedlingd will own restart, pacing, and the start limit there, and will record attempts firsthand. That makes recording a portable requirement, and this is the runtime where it can be built and tested today. Settling the portable shape here means the Windows spec conforms to a proven rule instead of inventing one alongside an unbuilt runtime.

It also reframes the portable rule usefully. Extracting systemd's *parameters* (`RestartSec`, `StartLimitBurst`) portably is awkward — they are not the knobs a Windows reconciler would have. Extracting the *observable* is not: the runtime records restart attempts, and `crash_loop` is a function of the recorded rate. Who actions the restart becomes platform detail.

## Spec changes (first, per the tracey workflow)

In `docs/spec/runtime.md`:

- **New**: the runtime records each restart of a container instance — when it happened, the exit status where known, and whether the restart was actioned by the supervisor or initiated by the runtime itself.
- **New**: `crash_loop` is filed when the recorded restart rate for an instance exceeds a threshold over a window, and cleared when the instance is later observed healthy. This replaces the systemd-specific trigger as the primary path.
- **Amend `r[autonomous.restart.start-limit-hit]`**: demote to a secondary trigger. A unit that has given up must still produce `crash_loop` even where the recorded rate has not crossed the threshold, and the existing stop-auto-recovering behaviour is unchanged.
- **Amend `r[autonomous.restart.backoff]`**: keep the systemd pacing requirements as the Linux mechanism, but stop making them the definition of crash-loop detection.
- **New**: retention for restart records, alongside the existing `r[gc.*]` rules.

Runtime-initiated restarts (deploys, `r[autonomous.healthcheck-replace]`) are recorded but excluded from the crash-loop rate — otherwise every rolling update reads as a crash burst.

## Data model

New migration `v54.sql` plus its `Migration` entry at the bottom of `crates/core/src/runtime/db.rs` (never edit a shipped block). One row per observed restart: instance identity, generation, timestamp, exit code and exit kind where known, and the initiator.

Growth is bounded per instance rather than globally: a hard crash loop produces rows fastest exactly when the detail is most wanted, so keep the last N attempts per instance and let age-based GC handle the rest. A per-instance cap bounds the worst case deterministically, where rate-limiting (the `r[history.operations.rate-limiting]` precedent) would drop precisely the samples being diagnosed.

## Observing restarts on Linux

Restarts cannot be counted by polling container state: if systemd restarts within `RestartSec` and that is shorter than the observe interval, the observer sees `active` before and after and never learns anything happened. `r[autonomous.job-terminal]` already concedes this hazard for short-lived jobs.

Read systemd's own counter instead. `NRestarts` is monotonic per unit, so a per-poll delta catches restarts that were never observed as a state transition. It lives on `org.freedesktop.systemd1.Service`, not the `Unit` interface `Systemd1UnitProxy` covers today, so this adds a proxy trait in `crates/core/src/system/systemd.rs` alongside the existing one. `ExecMainStatus` and `ExecMainCode` on the same interface give the last exit, which is what makes a record diagnostic rather than a tally.

Two wrinkles to get right in v1:

- `NRestarts` resets on `reset-failed` and on a deliberate stop/start, so a *decrease* is a reset, not a negative delta. Treat the counter as monotonic-with-resets and re-baseline rather than recording a negative.
- The delta includes restarts seedling itself caused. The reconciler knows when it initiated one; those are recorded with the runtime initiator and excluded from the rate.

Reading two extra properties per pod instance per tick is one additional D-Bus round trip on a path already fetching `ActiveState` and `SubState` per instance (`unit_state_impl`). If that shows up in tick latency on a large host, batch through `ListUnits` rather than per-unit property reads.

## Touch points

| Area | File |
|---|---|
| Migration | `crates/core/src/runtime/db.rs`, `db/migrations/v54.sql` |
| Service proxy, unit properties | `crates/core/src/system/systemd.rs`, `system/types.rs` (`UnitState`) |
| Observation | `crates/core/src/system/observer.rs` (`observe_pod_instance`) |
| Crash-loop detection | `crates/core/src/system/reconcile/pods.rs`, `reconcile/faults.rs` |
| Operator interface | `docs/spec/interface.md`, `crates/protocol`, `crates/core/src/oi/` |
| CLI and web | `crates/ctl`, `crates/web` — the restart history needs a CLI command, not only a UI panel |

## Tests

- Counter delta across a simulated restart, including the reset-to-zero case and a re-baseline after `reset-failed`.
- Runtime-initiated restarts recorded but excluded from the rate; a rolling update must not file `crash_loop`.
- Rate threshold crossing files the fault; observed-healthy clears it.
- A unit reaching `start-limit-hit` below the rate threshold still files the fault.
- Per-instance cap holds under a sustained crash loop.

## What Windows inherits

`wcr[shim.ownership]` drops its restart clause; the reconciler owns restart, pacing, and the start limit, and records each attempt at the point it actions one — no counter inference needed. Two Windows-specific requirements come with it: the exit observation must be folded into history before the exited task is reaped (containerd requires deletion before the container ID is reusable), and the daemon-down gap — a workload that crashes while seedlingd is down stays down until it returns, bounded by SCM restart — is stated as a property rather than left to be discovered.

## Open

- The rate threshold and window. Wants to be loose enough that a slow-failing container gets several chances and tight enough to catch flapping on a human timescale, which is the same judgement `r[autonomous.restart.backoff]` already makes for systemd's parameters — but it is now seedling's number, and it is operator-visible.
- Whether restart history is its own operator-interface surface or an extension of an existing one.
