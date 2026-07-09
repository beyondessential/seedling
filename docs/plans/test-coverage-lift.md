# Test coverage lift

Baselines (2026-07-09): Rust 37.9% lines (16,591/43,828 via cargo-llvm-cov);
frontend 3.4% lines (104/3,053 via vitest v8). Goal: significant lift in both,
prioritising bang-for-buck; no testability refactors; keep test wall-time
reasonable (favour fewer, broader tests over many tiny ones).

## Output

Two PRs:

1. `test: lift Rust coverage` — OI dispatch harness + handler tests, protocol
   event serialisation tests, runtime CRUD tests, pure-translation tests, ctl
   and web unit tests.
2. `test: lift frontend coverage` — renderWithSession harness + fixture
   factories, component and route tests.

Plus a report (in the PR bodies / final message) on areas deliberately
excluded that would benefit from future spikes.

## Rust plan

### Harness (build first, in main workspace)

`#[cfg(test)]` test-support module in `crates/core` providing
`test_oi_state()`: in-memory `Db`, `Cipher::for_tests()`,
`System::setup_stubbed(tempdir, false)`, `Coordinator::new`, empty
registries, `new_event_channel()`, `tailscale_provider: None`,
`site_resolver: None`. Prove it with a smoke test driving
`oi::handler::dispatch` for `/server/ping` and `/server/status`.

### Targets (one jj workspace + agent each)

- R1 `protocol/events.rs` (~1,010 uncov): serialisation shape tests for the
  event enums + emit helpers; also `error.rs`, `actor.rs` gaps.
- R2 OI handlers: apps read/lifecycle-adjacent (`handler/apps.rs`, 1,591
  uncov) — list/show/script/generations/plan/scale/stop/unstop via dispatch;
  skip paths needing live containers.
- R3 OI handlers: volumes + backups (1,506 uncov).
- R4 OI handlers: tls + ingresses + services (~1,772 uncov).
- R5 OI handlers: images + params + templates + registries + keys + dispatch
  routing/error paths (~1,300 uncov).
- R6 runtime DB CRUD: `runtime/apps.rs`, `faults.rs`,
  `external_volume_mappings.rs`, `site_volumes.rs`, `stopped.rs`,
  `restart_gens.rs`, `apps/params.rs`, `apps/secret_params.rs`,
  `apps/registry_faults.rs` (~1,300 uncov) via `Db::open_in_memory()`.
- R7 pure translations: `system/caddy/config.rs`, `system/translate/*` gaps,
  `defs/summary.rs` gaps, `runtime/desired.rs` gaps (~1,000 uncov).
- R8 ctl pure parse/format helpers (`apps.rs`, `op.rs`, `backups.rs`,
  `tls.rs`, `templates.rs`, `forward.rs`) + web Rust (`config.rs`,
  `proxy.rs` PeekedRequest, `event_broker.rs`) (~900 uncov).

### Conventions

- Tracey `// <l|r|i|w>[verify <id>]` annotations citing existing spec IDs
  only; no new spec items (we test existing behaviour).
- No new dependencies without flagging; dev-deps added centrally in the
  harness commit if needed.
- Tests must not shell out, hit the network, or need podman/systemd.
- Each agent runs scoped `cargo nextest run -p <crate>`, `cargo clippy`,
  `cargo fmt` before finishing.

## Frontend plan

### Harness (build first)

`src/test/harness.tsx`: `renderWithSession(ui, {fixtures, route, path,
events, safetyMode})` — MemoryRouter + SafetyModeProvider (seeding
`sessionStorage["seedling.safetyMode"]` for write mode) +
`SessionContext.Provider` with fake client whose `request` resolves
`{ok:true, value: fixtures[method]}` (or an injected error), plus
`vi.fn` capture for assertions. `src/test/factories.ts`: minimal valid
instances of the types in `src/lib/types.ts`. localStorage/sessionStorage
reset in the harness. Prove with a `Faults.tsx` smoke test.

### Targets (one jj workspace + agent each)

- F1 pure components: ActionButton, SafetyModeSwitcher, PlanDiff,
  ScriptInventory, ImageReferences, OiErrorAlert, ErrorPage, Offline,
  SafetyModeProvider (fake timers).
- F2 list routes: Apps, Faults, Keys, Registries, Images.
- F3 routes: Services, Ingresses, Templates.
- F4 routes: Volumes, Backups + MapVolumeDialog, Snapshot/Promote dialogs.
- F5 AppDetail (3,020 lines; largest single file).
- F6 Certificates + TlsHostnamesTable (clipboard/createObjectURL stubs).
- F7 Navbar, EventsSidebar, Logs/InfraLogs (fake `streamLogs`), Login error
  paths, hooks (useOiQuery cache/error paths, useEventRefresh).

### Conventions

- No new npm deps; use @testing-library/react + fireEvent.
- Mock `@uiw/react-codemirror` to a textarea and xterm to a no-op ONLY where
  a target route imports them; do not test editor/terminal internals.
- Favour one broad render+interaction test per view state over many tiny
  assertions; keep vitest wall-time additions modest.

## Orchestration

jj workspaces (NOT git worktrees): harness commits land in the default
workspace first; then `jj workspace add` one workspace per agent batch,
each based on the harness tip. Agents commit with jj in their own
workspace. Merge: rebase each batch's commits onto the accumulating tip in
ID order (disjoint files; conflicts unlikely), run the full suite +
coverage, fix fallout, then bookmark and open PRs.

## Deliberate exclusions (report these; candidates for future spikes)

- `daemon/main.rs` (912 uncov): startup/wiring glue; would need a
  process-level harness spike.
- `system/` actuation: podman.rs, systemd.rs, volume_store.rs, actuator*,
  data_plane/nft.rs, jool.rs, caddy/startup.rs, resolver/* (~5,400 LOC):
  real side effects; future spike = fake-command-runner seam.
- `oi/server.rs`, `oi/shells/*`, `oi/forwards/*` (~2,800 LOC): QUIC/PTY
  sessions; future spike = in-process QUIC loopback harness.
- `runtime/tls/{issuance,acme,dns/route53,serve}.rs`, `tailscale.rs`:
  network/CA-bound; future spike = pebble-based ACME integration test.
- `protocol/client.rs`: QUIC client; covered indirectly by any future
  loopback harness.
- `system/reconcile/*` deep paths (faults.rs 974 uncov, pods, phases):
  reconciler needs richer fixtures than stub fleet exposes cheaply —
  partial coverage may fall out of OI handler tests; assess after.
- Frontend: Shell.tsx/ShellsSidebar (xterm), CodeMirror editor internals,
  SessionProvider connect/reconnect loops, Login success path (needs
  WebTransport + fetch mocks): low value per effort in jsdom; future spike
  = Playwright e2e expansion (infra already exists in frontend/e2e).
