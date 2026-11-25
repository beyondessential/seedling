# Split seedling-core into a portable core and a Linux platform crate

## Why

T6 (`definitions check` in `seedling-ctl`) needs the evaluator on every platform `seedling-ctl` ships for: Linux, macOS and Windows.
Making `seedling-core` portable unblocks it. A runtime for another platform is a long-run possibility, not a goal of this card.

## Tech notes

### Where the line falls

Core keeps every decision; `seedling-linux` holds only the adapters that touch the host.

- Stays in core: `defs/`, `runtime/`, `oi/`, the reconciler (`system/reconcile*`), `system/translate/`, Caddy and CoreDNS configuration (they run as infrastructure containers through the container-runtime trait), `stub.rs` and `unavailable.rs`.
- Moves to `seedling-linux`: the Podman, systemd and journal clients, the nftables data plane, rtnetlink address and route reading, Jool, the btrfs volume store, confinement, host probes, and host facts read from `/proc`.
- Core gains traits for the facilities not yet behind one: log reading, the volume store, routes and addresses, the NAT64 translator, host probes and host facts. They join the existing `ContainerRuntime`, `ProcessManager`, `NetworkProxy` and `DataPlane`.

### Unix code that stays in core

File modes on the database, secrets, audit log and TLS files, `getuid` in definition auth, signal names in the barrier, and process handling in shells stay in core under `cfg(unix)`, each behind one small helper.
On Windows, a path only the daemon reaches is `todo!()`; a path `seedling-ctl` reaches through T6, such as `getuid` in credential lookup, gets a real Windows implementation.

### Keeping core portable

CI runs clippy on `seedling-core` for macOS and Windows on every PR.
Core's test suite runs on Linux only; T6's check tests run on all three platforms.

`seedling-linux` and the daemon are Linux-only crates.
CI and release builds on macOS and Windows name their packages with `-p`; `cargo build --workspace` is a Linux command.

### Host facts

Host facts are a trait with no default implementation in core; `seedling-linux` implements it.
Off Linux, `seedling-ctl` takes memory as undetermined unless `--const` sets it, which is filed on T6.

`ProbeSource` in `system/host_probes.rs` is already that trait; `HostSource`, its host implementation, is what moves.
Of what it calls, the NAT64 detection (`nat64::detect_external_nat64`, a DNS lookup) and the thread, time-zone and node-name detection in `sysconst.rs` are portable and stay in core; the egress detection (rtnetlink) and the `/proc/meminfo` read move.
`SystemFacts::default()` stops reading memory and reports it undetermined; on Linux, `seedling-ctl` can reach the Linux host source through a Linux-only dependency on `seedling-linux`, which is T6's to decide.

### Linux-settled values

The process manager takes what a workload is, and `seedling-linux` turns that into what the Linux platform spec settles: slice names and weights, OOM score adjustments and restart pacing.
Unit names stay in core, because the reconciler matches units against recorded identity.

### Specs follow the boundary

The generic specs name host facilities, the snapshotting volume store and the packet filter, rather than BTRFS and nftables; the Linux platform spec settles them with the detail the generic specs dropped.
The `nftables_failed` fault is `packet_filter_failed`.

## Build steps

### Scaffolding

- [ ] Create `crates/linux` as the `seedling-linux` package, depending on `seedling-core` and `seedling-protocol`, and add it to the workspace members
- [ ] Make the `seedling` daemon depend on `seedling-linux`

### Traits for the facilities without one

Each trait goes beside the existing four in `system.rs`, with a stub in `system/stub.rs` for core's tests.

- [ ] Log sink and log source, for what `system/journal.rs` and `system/breadcrumb.rs` do with the journal: core keeps building the fields (`instance_log_fields`, `Breadcrumb`, `LogTarget`, `LogEntry`, `LogStreamOptions`) and hands entries to the sink and stream requests to the source. Breadcrumbs are written from the barrier with no handle in reach, so the sink is installed once at startup, as `sysconst::set` is
- [ ] Volume store: what `system/volume_store.rs` offers (create, remove, hold, restore, held-volume records, site volumes, snapshot, promote, backend match) plus the confined writes and owned-directory checks `system/actuator.rs` takes from `system/confined.rs`. Held-volume metadata and site-name claiming are decisions and stay in core
- [ ] Backend connector: how `system/backends.rs` reaches a working container runtime and process manager (today `PodmanRuntime::new` and `SystemdManager::connect`), so the probing and switching stay in core and the connecting comes from the platform
- [ ] NAT64 translator: `jool::setup_nat64` and `teardown_nat64`, called from the reconciler
- [ ] Machine identifier: `system::node_prefix_from_machine_id` keeps the derivation in core and takes the identifier from the platform

### Workload standing instead of Linux values

- [ ] `TransientUnitSpec` (`system/types.rs`) carries the workload's standing (a `WorkloadStanding`, or infrastructure) in place of `slice`, `oom_score_adjust`, `restart_sec`, `start_limit_interval_sec` and `start_limit_burst`
- [ ] `ProcessManager::sync_slices` takes the set of standings to provide for (each app's priority and the Deployment priorities it uses, plus infrastructure) in place of named, weighted `SliceSpec`s, keeping its `prune` flag
- [ ] The reconciler's slice computation (`system/reconcile.rs`, around the `priority::app_slice` calls) and the unit specs built in `system/actuator/pod.rs`, `system/caddy/startup.rs` and `system/resolver/startup.rs` produce standings
- [ ] `system/priority.rs`'s slice naming, weights and OOM mapping, with their tests and the slice-weight tests in `system/reconcile.rs`, move to `seedling-linux` with the systemd manager; `WorkloadStanding` stays in core
- [ ] `reserved.rs` keeps `infra` as a reserved app name as a literal of its own, since it no longer has `priority::INFRA_COMPONENT` to borrow
- [ ] The stub process manager in `system/stub.rs` records standings, and the reconciler tests that asserted slice names or weights assert standings instead

### Moving the Linux adapters

Each move carries its tests and `// spec:` citations with it.

- [ ] `system/podman.rs` → `seedling-linux`
- [ ] `system/systemd.rs` and `system/systemd/` → `seedling-linux`
- [ ] The journal reading and writing out of `system/journal.rs` and `system/breadcrumb.rs` → `seedling-linux`, implementing the log sink and source
- [ ] `system/data_plane.rs` and `system/data_plane/` (nftables and routes) → `seedling-linux`
- [ ] `system/netinfo.rs` → `seedling-linux`
- [ ] `system/jool.rs` → `seedling-linux`, implementing the NAT64 translator
- [ ] `system/confined.rs` and the btrfs and plain-directory storage of `system/volume_store.rs` → `seedling-linux`, implementing the volume store
- [ ] `HostSource` out of `system/host_probes.rs`, and `detect_available_memory` and `parse_mem_total_bytes` out of `sysconst.rs` → `seedling-linux`
- [ ] Reading `/etc/machine-id` → `seedling-linux`
- [ ] Make `SystemFacts::default()` report memory as undetermined

### Unix code that stays in core

- [ ] One helper for private file modes, used by `runtime/db.rs`, `runtime/secrets.rs`, `runtime/audit.rs` and `runtime/tls/serve.rs`: the Unix mode under `cfg(unix)`, `todo!()` on Windows
- [ ] `runtime/definition/auth.rs`: `getuid` under `cfg(unix)`, and a Windows implementation for what T6's credential lookup needs
- [ ] `runtime/barrier/runtime.rs`: decode signal names from exit codes with a table of core's own instead of `nix::sys::signal::Signal`, since this is portable data
- [ ] `oi/shells/registry.rs` and `oi/shells/session.rs`: the PTY size, `libc::kill` and `ExitStatusExt` under `cfg(unix)`, `todo!()` on Windows
- [ ] Unix-only tests (symlinks and modes in `oi/handler/volumes/tests.rs`, `runtime/db/tests.rs`, `runtime/secrets.rs`, `runtime/tls/serve.rs`) under `cfg(unix)`

### Dependencies

- [ ] Move `nftables`, `rtnetlink`, `podman-rest-client`, `systemd` and `zbus` from `crates/core/Cargo.toml` to `crates/linux/Cargo.toml`
- [ ] Drop `nix` from core once the barrier no longer needs it; make `rustix` and `libc` `cfg(unix)` target dependencies of core
- [ ] Confirm no other dependency of core fails to build for Windows, fixing or gating any that does

### Wiring the daemon

- [ ] `crates/daemon/src/main.rs` builds the platform from `seedling-linux` (backend connector, data plane, volume store, log sink and source, host source, NAT64 translator, machine identifier) and hands it to core where it builds `System` today
- [ ] The snapshotting check in `main.rs` asks the volume store rather than calling `volume_store::is_btrfs`
- [ ] `system::System` holds the volume store as a trait object

### CI and docs

- [ ] Add a job to `.github/workflows/rust.yml` running `cargo clippy -p seedling-core --all-targets -- -D warnings` on macOS and Windows
- [ ] Update `docs/` and `DEVELOP.md` wherever they name a module that moved or describe which crate holds what
- [ ] Run clippy, fmt, the spec style and citation checks, and the workspace tests on Linux

## Explicitly rejected

- Moving all of `system/` into `seedling-linux`, reconciler included. Another platform would have to rewrite the reconciler.
- Lifting only the evaluator into a new crate, leaving core Linux-only.
- Putting the Unix-only code in core behind platform traits.
- Gating `runtime/` and `oi/` behind a cargo feature that `seedling-ctl` leaves off.
- Running core's full test suite on macOS and Windows.
- Making `seedling-linux` compile to nothing off Linux so a bare workspace build passes everywhere.
- A portable default for host facts in core.
- Leaving the Linux-settled values (slices, OOM scores, restart pacing) in core.
- Moving unit naming to `seedling-linux`, which would rework how the reconciler names and finds units.
