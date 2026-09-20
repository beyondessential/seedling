# H1 — Deployment priority and app priority

Working notes from the spec interview. The acceptance criteria live in the tracey specs:
`l[deployment.priority]` and `l[const.priority.enum]` (language), `r[priority.*]` (runtime),
`i[app.priority.*]` (interface).

## Design decisions (settled)

- **Two levers, not a raw OOM knob.** A per-Deployment `priority(Priority)` declared in BSL
  (`critical`/`elevated`/`normal`/`low`, default `normal`), and a per-app app priority
  (`high`/`normal`/`low`, default `normal`) set operationally, not in the definition. No literal
  `OOMScoreAdjust` on the BSL surface — the level is intent, the runtime owns the mapping.
- **App-major composition.** Kill order and CPU/IO share are decided app-first, then Deployment
  tier. A `Critical` deployment in a `low` app is shed before a `Normal` deployment in a `high` app.
- **Seedling owns its slices.** It does not expect or join the host's `critical.slice` /
  `elevated.slice`; it creates its own grouping (`r[priority.groups-owned]`).
- **Relative only.** Weights and kill-preference, no hard aggregate caps. Per-container hard caps
  stay with `container.memory` / `container.cpus`.
- **Deployments only.** Services and jobs run at `Normal`; `priority` is not registered on Job.

## Build shape (to expand via plan-implementation)

- **BSL surface.** Add `Priority` enum in `defs/enums.rs` (mirror `OnExit`: `rhai_constant()`,
  register in the constants scope). Add `priority` field + builder on the Deployment def (it is a
  `deployment.*` method, so it goes on the deployment builder, not the shared container mixin in
  `defs/container.rs`). Reject on Job by not registering there.
- **Plumbing.** Thread the resolved (app-priority, deployment-priority) standing through
  `start_pod_instance` into new fields on `TransientUnitSpec` (`system/types.rs`), then emit the
  systemd properties in `system/systemd.rs` alongside `KillSignal`/`RestartUSec`: `Slice=`,
  slice-level `CPUWeight`/`IOWeight`, per-unit `OOMScoreAdjust`. Infra units (caddy, resolver)
  rank above all app workloads.
- **Slice tree.** `seedling-<app>.slice` → `seedling-<app>-<tier>.slice` → units. Slices are
  transient units too (or written unit files) — need a lifecycle: create before the unit joins,
  reweight live when app priority changes, GC when empty. Reserve the slice-name scheme in
  `crates/core/src/reserved.rs` and reject colliding workload names at creation only.
- **App priority storage.** New durable per-app operator setting (like scale decisions):
  new `version < N` migration block at the bottom of `runtime/db.rs`, discard on uninstall.
  Live reweight without redeploy (see `autonomous.restart.rate.settings` for the pattern of an
  operator setting that takes effect without a runtime restart).
- **Interface + ctl.** `/apps/priority` OI endpoint (`i[app.priority.set]`), surfaced in
  `/apps/show` and `/apps/list` (`i[app.priority.describe]`). Add the matching `ctl` subcommand —
  per AGENTS.md, anything in the OI needs a CLI command.
- **Web UI.** In scope (`w[routes.apps.priority]`, `w[routes.apps.priority-indicator]`).
  App priority is a control on `frontend/src/routes/AppDetail.tsx` plus a chip in the apps table
  (`frontend/src/routes/Apps.tsx` — follow the `w[impl routes.apps.fault-count]` chip already there,
  around Apps.tsx:181). Deployment priority is a read-only indicator per Deployment resource on the
  app detail page, next to the healthcheck indicator in the resources table. Both colour scales come
  from the existing status palette. Live update on change without a reload, consistent with the
  held-volumes badge.

## Implementation checklist

### 1. BSL surface (language)
- [ ] `Priority` enum in `defs/enums.rs`: `Critical`/`Elevated`/`Normal`(default)/`Low`, totally ordered (`Critical > Elevated > Normal > Low`), with `rhai_constant()`. Annotate `l[impl const.priority.enum]`.
- [ ] Register `Priority` constant in `defs.rs` scope (after `OnExit`, ~line 237).
- [ ] `DeploymentDef.priority: Priority` field (default Normal) in `defs/deployment.rs`; `deployment.priority(level)` builder. Annotate `l[impl deployment.priority]`. Not registered on Job → calling on Job is a BSL error (covered by absence).
- [ ] `DeploymentSummary` gains a `priority` field (`defs/summary.rs`) so `/apps/plan` diffs it and `/apps/show` static JSON carries the declared level.

### 2. App priority storage (operator setting)
- [ ] New `runtime/priority.rs` mirroring `scaling.rs`: `AppPriority` (High/Normal/Low, default Normal), `load_app_priority`, `save_app_priority`, `delete_app_priority_for_app`, `effective_app_priority`. `pub mod priority;` in `runtime.rs`.
- [ ] `v57.sql` migration: `app_priorities (app TEXT PRIMARY KEY, priority TEXT NOT NULL, updated_at TEXT NOT NULL)`. Register `SQL_V57` + `Migration{version:57}` in `db.rs`. Annotate `r[impl priority.settings]`.
- [ ] Discard on uninstall + deregister (`apps.rs` ~1427 and ~1373). Annotate `i[impl app.priority.reset-on-uninstall]`.

### 3. Interface (OI + ctl)
- [ ] `/apps/priority` handler `set_priority` in `oi/handler/apps.rs` (mirror `scale_app`): `PriorityParams{app, priority}`, validate level (bad → `invalid_request`/RequirementsInvalid), app exists (→ not_found), save, `tick_notify`, emit `PriorityChanged`. Route in `handler.rs`. Annotate `i[impl app.priority.set]`.
- [ ] `/apps/show`: top-level `priority` (operator app priority) + per-Deployment `priority` (declared). `/apps/list`: per-app `priority`. Annotate `i[impl app.priority.describe]`.
- [ ] `OiEvent::PriorityChanged` + builder in `protocol/src/events.rs`.
- [ ] `ctl apps priority <app> <level>` subcommand + dispatch.

### 4. Actuation (runtime → systemd) — DEPENDS ON OOM-LIVE DECISION (see below)
- [ ] Combined-standing model: map `(AppPriority, Priority)` → CPU/IO slice weights + per-unit `OOMScoreAdjust`, app-major. Infra (caddy, resolver) ranks above all app workloads. Values are implementation concern; app term dominates OOM so app-major holds.
- [ ] Slice naming: `seedling.slice` → `seedling-<esc_app>.slice` (app weight) → `seedling-<esc_app>-<tier>.slice` (tier weight) → unit. Escape `-` in app component as `\x2d` (systemd escaping) so the hierarchy is clean; extend `validate_unit_name` to accept the escape. Infra: `seedling-infra.slice`.
- [ ] `reserved.rs`: reserve the slice-name scheme; reject a resource whose realised unit collides (creation only). Annotate `r[impl priority.groups-owned]`.
- [ ] `TransientUnitSpec`: add `slice: Option<String>`, `oom_score_adjust: Option<i32>`; emit `Slice=` + `OOMScoreAdjust=` in `systemd.rs`. Annotate `r[impl priority.actuation]`.
- [ ] `ProcessManager`: `ensure_slice(name, cpu_weight, io_weight)` (StartTransientUnit create / SetUnitProperties reweight), `remove_slice`. Impl in `systemd.rs`, `stub.rs`, `unavailable.rs`.
- [ ] Reconcile pass: each tick, derive desired slices for running workloads, create/reweight (live CPU/IO), GC empty. Thread `(app_prio, dep_prio)` into `start_pod_instance` for `Slice=`/`OOMScoreAdjust=`. App-priority read fresh per tick like `compute_effective_scales`.
- [ ] Infra: caddy + resolver startup join `seedling-infra.slice` with protective OOM.

### 5. Web UI (in scope)
- [ ] `AppDetail`: settable app-priority control in header (mirror scale handler) → `/apps/priority`. Read-only per-Deployment priority indicator in the State cell (mirror `HealthcheckIndicator`), shown when declared level ≠ normal. `w[impl routes.apps.priority]` / `w[impl routes.apps.priority-indicator]`.
- [ ] `Apps` table: priority chip in status column, shown only when app priority ≠ normal; colour distinguishes raised vs lowered.
- [ ] `priorityColor`/`priorityLabel` helpers in `lib/status.ts`; types in `lib/types.ts`; `AppPriorityChanged` added to `APP_LIST_EVENTS` + `APP_DETAIL_EVENTS` + `SeedlingEvent`.

### 6. Tests + docs
- [ ] BSL tests (enum constant, deployment.priority, Job rejection), storage tests, OI handler tests (set/validate/not_found/describe), systemd emission tests, reserved-name tests, weight/OOM mapping tests.
- [ ] `tracey query status` clean for new `l/r/i/w[impl ...]`; add `r[verify ...]`/etc where practical.

## Open / to confirm

- Whether `Priority` is an enum constant (`Priority.Critical`) as specced, or a plain string —
  specced as an enum to match `OnExit`/`OnUpdate`.
  - Answer: Probably an enum
- Whether app priority also wants a Canopy-remote path later (this card is local OI + ctl only).
  -  Answer: not in scope
- Whether the UI should also visualise the *combined* app-major shed order across all apps
  ("what goes first if this host runs out of memory"). Deliberately not specced: useful, but a
  bigger surface than exposing the two levers, and it belongs to a fleet/host view rather than the
  app detail page.
  - Answer: let's make a low-prio card for this.
  
