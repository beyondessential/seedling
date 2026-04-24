# Readiness, liveness, and healthchecks

## Context

Seedling tracks a `Ready` lifecycle state for container resources and the runtime spec already says a container is Ready when "running and any configured health checks are passing" (`r[lifecycle.container]` in `docs/spec/runtime.md`). The barrier oracle already maps a `health_check_pass` observation to `LifecycleState::Ready`, and the system observer already ingests podman's `ContainerHealth::{Starting,Healthy,Unhealthy}` (`crates/core/src/system/observer.rs:211`).

What is missing:

1. **No BSL syntax** for declaring a healthcheck. Today a container is Ready purely based on whichever `HEALTHCHECK` the image author baked in — or, if none, "Running implies healthy" (`observer.rs:218`). App authors have no way to override or define a check.
2. **Unhealthy observations are dropped on the floor.** `ObservationFact::ContainerUnhealthy` emits no observation kind (`system/types.rs:492`) so the oracle never transitions a Ready container back to Running, and no fault is filed on persistent unhealth.
3. **No liveness-style response to failure.** Even once an unhealthy signal propagates, there's no declarative way to say "kill the container and let the on_terminate policy recreate it".

This plan closes all three gaps with a single `.healthcheck(...)` builder on `Deployment` (and `Job`) that is translated to podman's native `--health-*` flags. Readiness semantics fall out automatically (Ready = Running + Healthy). Liveness-style behaviour is opt-in via `on_failure: "restart" | "kill" | "stop"` which maps to podman's `--health-on-failure`. Sustained health failure files a fault that auto-clears on recovery.

Application alerts (non-faulting, operator-visible notices) are a separate concern and will be planned in their own file. This plan intentionally omits them.

## Non-goals

- No k8s-style separate liveness / readiness / startup probes. If the single-probe model proves insufficient, split it later.
- No healthchecks for services, ingresses, volumes, or external resources — only containers.
- No healthcheck for the seedling daemon itself. `/server/ping` already covers that case.
- No BSL API for firing alerts (covered in the separate alerts plan).

## Design

### BSL surface

A new method on `Deployment` and `Job`:

```rhai
app.deployment("web")
    .image("myapp:latest")
    .healthcheck(#{
        kind: "command",                                         // required discriminator
        cmd: ["curl", "-fsS", "http://localhost:8080/health"],
        interval: "10s",       // jiff-parseable duration string
        timeout: "3s",
        retries: 3,
        start_period: "30s",   // grace window before failures count
        on_failure: "none",    // "none" | "kill" | "restart" | "stop"
    });
```

`kind` is **required** and is the forward-compat hinge. In v1 the only accepted value is `"command"`, but the field shape reserves space for future networked probes that seedling will poke from the host (not delegated to podman):

- `kind: "http"`, `url: "/health"`, `expect_status: 200..400`, `host_header: ...` — probed over pod-address from the daemon.
- `kind: "tcp"`, `port: 8080` — open a TCP connection and close it.
- `kind: "grpc"`, `port: 9000`, `service: "..."` — call the gRPC health RPC.

Because the networked variants cannot be delegated to podman's `--health-cmd`, they would require seedling to schedule its own probe loop and emit `ContainerHealthy` / `ContainerUnhealthy` observations itself. That work is explicitly out of scope here; the spec entry for `healthcheck.kind` enumerates the reserved values as "future", and the implementation rejects any value other than `"command"` with a clear error.

Common fields across all kinds (interval / timeout / retries / start_period / on_failure) live at the top level of the map, not inside a nested struct per kind, so the flat map shape is stable across kinds. All fields except `kind` and the kind-specific payload (`cmd` for `"command"`) are optional with documented defaults (mirroring podman: 30s/30s/3/0s/none). `cmd` may also be provided as a single string (treated as `["CMD-SHELL", s]`).

### Spec changes (first)

Per repo rule: spec first, then implement, then test.

**`docs/spec/language.md`** — new section under Resources/Deployment:

- `l[deployment.healthcheck]` — the builder exists and accepts the shape above.
- `l[healthcheck.kind]` — `kind` is required; enumerate accepted values. v1 accepts only `"command"`; document `"http"`, `"tcp"`, `"grpc"` as reserved for future use. Unknown kinds must be a BSL-time error.
- `l[healthcheck.command]` — when `kind: "command"`, `cmd` is the probe command; exit code zero means healthy.
- `l[healthcheck.timings]` — interval, timeout, retries, start_period semantics. Common across all kinds.
- `l[healthcheck.on_failure]` — enumerate the four values and what each means in terms of lifecycle state transitions. Common across all kinds.

Add an equivalent mixin for `Job`.

**`docs/spec/runtime.md`** — augment existing lifecycle and fault sections:

- Update `r[lifecycle.container]` prose to reference declared healthchecks explicitly (today it says "any configured health checks" abstractly).
- Add `r[lifecycle.container.unhealthy-transition]` — a Ready container that observes sustained unhealth transitions back to Running (not Ready) until it observes health again.
- Add `r[fault.healthcheck]` — when a container is unhealthy for longer than `start_period + (interval × retries)` a fault of kind `health_check_failed` is filed, scoped to the instance. It auto-clears when the container observes `health_check_pass` or is removed.
- Add `r[healthcheck.on-failure]` — the runtime response to `on_failure` values:
  - `none`: no automatic action (default).
  - `kill`: send SIGKILL once, then let `on_terminate` policy decide recreation.
  - `restart`: stop the container cleanly and recreate according to `on_terminate`.
  - `stop`: stop the container cleanly and leave it stopped (do not recreate in this reconciliation tick).
  Note that `kill`/`restart`/`stop` are passed to podman via `--health-on-failure`; the runtime's role is to observe the resulting state transitions, not to perform the kill itself.

### Data model

No new DB tables. The healthcheck config lives in the existing app definition (serialised in `registered_apps.current_generation`). Fault rows reuse the existing `faults` table with kind `health_check_failed`.

One new column may be needed if we want to track "first observed unhealthy" timestamps per instance to implement the `start_period + retries × interval` grace window — but the autonomous_ops log or a derived query over `world_observation_history` is likely sufficient. Implementer should try the derivation path first; fall back to a new migration only if required.

### Implementation layering

Spec changes land first in a single commit. Then the code changes, which naturally split across the def → translate → system → observe → fault layers:

1. **Defs** — `crates/core/src/defs/pod.rs` (or `container.rs`): add a kind-discriminated healthcheck type. Shape:

   ```rust
   pub struct HealthcheckDef {
       pub kind: HealthcheckKind,
       pub interval: Duration,
       pub timeout: Duration,
       pub retries: u32,
       pub start_period: Duration,
       pub on_failure: HealthcheckOnFailure,
   }

   pub enum HealthcheckKind {
       Command { cmd: Vec<String> },
       // reserved: Http { url, expect_status, host_header }, Tcp { port }, Grpc { port, service }
   }

   pub enum HealthcheckOnFailure { None, Kill, Restart, Stop }
   ```

   Field lives on the container config because a pod could in principle have multiple containers (verify first whether `PodDef` holds one container inline or a list; this determines whether healthcheck is a `PodDef` field or per-container).

2. **BSL mixin** — add `.healthcheck(#{...})` to `Deployment` in `crates/core/src/defs/app/deployment.rs` and mirror on `Job` in `crates/core/src/defs/app/job.rs`. Use `rhai::Map` → `HealthcheckDef` parsing that (a) requires `kind` and errors clearly if missing or unknown, (b) validates kind-specific fields (`cmd` required when `kind: "command"`). Validate durations against jiff's parser. Annotate with `l[impl deployment.healthcheck]` / `l[impl healthcheck.kind]` / `l[impl healthcheck.command]` placed directly before the handler bodies, not at the top of `CustomType::build`.
3. **Translate** — extend `ContainerSpec` in `crates/core/src/system/types.rs` with `healthcheck: Option<ContainerHealthcheck>` (mirror shape of `HealthcheckDef` but in the system layer's vocabulary). Wire def → spec in whichever translate function currently builds `ContainerSpec` from `PodDef`.
4. **Podman args** — in `crates/core/src/system/podman.rs`, extend the argv builder to emit `--health-cmd`, `--health-interval=10s`, `--health-timeout=3s`, `--health-retries=3`, `--health-start-period=30s`, `--health-on-failure=none|kill|restart|stop`. Unit test the argv construction against a known `ContainerSpec` example.
5. **Observation mapping** — in `crates/core/src/system/types.rs:492`, give `ObservationFact::ContainerUnhealthy` a real mapping: `("health_check_fail", json!({}))`. Ensure the oracle in `crates/core/src/runtime/barrier/oracle.rs` treats `health_check_fail` as a demotion from Ready back to Running (the exact state machine edit depends on current `match` arms — read `oracle.rs:386` and neighbouring arms before editing).
6. **Fault filing** — in the reconcile loop (near `crates/core/src/runtime/lifecycle.rs` or wherever per-instance faults are derived today, e.g. the pattern in `apps/registry_faults.rs:83`): when an instance has been continuously unhealthy past its grace window, file a `health_check_failed` fault scoped to `(app, resource_type, resource_name, instance_id)`. Auto-clear on the next `health_check_pass` or when the instance is unscheduled. Use the existing `faults::clear_faults_for_instance` hook on tear-down.
7. **`on_failure` wiring** — for `kill`/`restart`/`stop`, podman handles the actuation. The runtime only needs to not fight it: confirm the reconciler does not immediately try to "correct" a container that podman just stopped on unhealth. The `on_terminate` policy (`OnTerminate::Recreate` vs `Keep`) already governs restart behaviour after a container exit, so `on_failure: restart` combined with `on_terminate: Recreate` yields "heal by restart". Document this interaction.

### Operator UX

`crates/web/frontend/` changes are minor. Existing surface already shows most of what we need:

- Per-resource lifecycle in `AppDetail.tsx` — once unhealthy demotes a Ready container to Running, the chip flips automatically.
- Faults inline per-app and on the global `/faults` page — `health_check_failed` faults show up with no UI change.
- The app-level `AppStatus::Degraded` is derived from presence of resources not in Ready + no faults (`crates/core/src/runtime/apps.rs:40`); `Faulted` wins when a fault is active. Both are correct for the new fault kind without code changes.

In addition, surface the declared healthcheck on the resource row so operators can see what's being probed without reading the rhai source:

- Extend `AppResource` (or the per-instance shape) in `crates/web/frontend/src/lib/types.ts` and the matching `/apps/show` handler in `crates/core/src/oi/handler/` with a `healthcheck: { kind, on_failure, interval_secs } | null` summary field. Don't ship the full probe payload — just enough for the indicator.
- In `AppDetail.tsx`, add a small chip or icon next to the lifecycle chip on containers with a declared healthcheck. Tooltip shows `kind`, `on_failure`, and the current state (passing / failing / in start-period). Colour follows the existing status palette (`crates/web/frontend/src/lib/status.ts`): success when healthy, warning when in grace, error when failing.
- A one-line entry in the resource detail expando listing the declared healthcheck fields (cmd truncated to a sensible width).

Spec this in `docs/spec/web.md` as `w[app.detail.healthcheck-indicator]` before implementing.

## Files to modify

Primary:

- `docs/spec/language.md` — healthcheck BSL spec with `kind` discriminator
- `docs/spec/runtime.md` — lifecycle unhealthy transition + fault kind
- `docs/spec/web.md` — healthcheck indicator in app detail
- `crates/core/src/defs/pod.rs` (or `container.rs` — verify) — `HealthcheckDef`, `HealthcheckKind`, `HealthcheckOnFailure`
- `crates/core/src/defs/app/deployment.rs` — `.healthcheck()` builder
- `crates/core/src/defs/app/job.rs` — `.healthcheck()` builder
- `crates/core/src/system/types.rs` — `ContainerSpec` healthcheck field; fix `ContainerUnhealthy` observation mapping
- `crates/core/src/system/podman.rs` — emit `--health-*` argv (only for `kind: "command"`)
- `crates/core/src/runtime/barrier/oracle.rs` — handle `health_check_fail` demotion
- `crates/core/src/runtime/lifecycle.rs` (or the fault-filing site) — file and auto-clear `health_check_failed`
- `crates/core/src/oi/handler/` (apps show path) — expose healthcheck summary on resources
- `crates/web/frontend/src/lib/types.ts` — `Healthcheck` summary type on `AppResource`
- `crates/web/frontend/src/routes/AppDetail.tsx` — indicator chip + resource expando row

Translate wiring (pod/container def → `ContainerSpec`): depends on where the current translation lives — identify before editing. Likely `crates/core/src/system/reconcile.rs` or a neighbour.

Examples and tests:

- `test.seed.rhai` — extend one deployment to declare a simple healthcheck as a living example.
- `crates/core/src/defs/app/deployment.rs` tests — builder accepts healthcheck, rejects malformed inputs.
- `crates/core/src/system/podman.rs` tests — argv construction against a `ContainerSpec` with and without healthcheck.
- `crates/core/src/runtime/barrier/oracle/tests.rs` — a `health_check_fail` observation after `health_check_pass` demotes Ready → Running.
- Integration test (fault filing): a deployment with a deliberately failing healthcheck files `health_check_failed` after the grace window, and clears the fault once the check recovers.

## Reused utilities

- `faults::file_fault`, `faults::clear_faults_by_kind`, `faults::clear_faults_for_instance` — `crates/core/src/runtime/faults.rs:54, 172, 207`. Same pattern as `registry_faults.rs` for dedupe-by-condition before filing.
- `ObservationFact` and observer pipeline already emit `ContainerUnhealthy` — we only need to give it a DB mapping, not re-wire podman inspection.
- `SessionProvider` + `useEventRefresh` already surface fault events to the UI; `FaultFiled`/`FaultCleared` will fire for the new kind with no protocol changes.
- Jiff duration parsing already used elsewhere in BSL for `deadline` arguments — reuse the same parsing helper for healthcheck timings (find via `grep "Duration::from" crates/core/src/defs/`).

## Verification

Static:

- `cargo fmt` and `cargo clippy` clean.
- `tracey query status` clean, and `tracey query uncovered --spec-impl runtime/main` plus the `language/main` variant show no new uncovered items introduced by this change.

Unit / integration:

- `cargo test` green.
- New tests listed above pass.

End-to-end in a local run:

1. `just run` (or repo's equivalent) to boot the daemon.
2. Install an app with `.healthcheck(#{ cmd: ["true"] })` — verify the instance reaches Ready and the app status chip is `Running`.
3. Flip to `.healthcheck(#{ cmd: ["false"] })` — verify:
   - Instance transitions Ready → Running after the grace window.
   - A `health_check_failed` fault appears in `/faults` and inline on the app detail page.
   - App status chip flips to `Faulted`.
4. Flip back to `cmd: ["true"]` — verify fault auto-clears and Ready returns.
5. With `on_failure: "restart"`, verify podman restarts the container on sustained failure and the `on_terminate` policy governs the subsequent recreate.

## Follow-up work (not in this plan)

- **Application alerts**: non-faulting operator-visible notices with list/dismiss. Separate plan. The user deferred design questions (who can file, how they clear) to that plan.
- Separate readiness vs liveness probes, if the `on_failure` single-probe model proves insufficient.
- Per-service readiness gating (service Ready when N backends healthy rather than the current ≥1); today's spec (`r[lifecycle.service]`) already uses ≥1 and that's probably fine indefinitely.
