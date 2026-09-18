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

## Open / to confirm

- Whether `Priority` is an enum constant (`Priority.Critical`) as specced, or a plain string —
  specced as an enum to match `OnExit`/`OnUpdate`.
- Whether app priority also wants a Canopy-remote path later (this card is local OI + ctl only).
- Whether the UI should also visualise the *combined* app-major shed order across all apps
  ("what goes first if this host runs out of memory"). Deliberately not specced: useful, but a
  bigger surface than exposing the two levers, and it belongs to a fleet/host view rather than the
  app detail page.
  
