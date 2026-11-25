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
So do `IDLE_CMD`, which scripts see at evaluation and so reads the same in `definitions check` on every platform, and the supervisor's unit-state vocabulary the observer reads (`start-limit-hit`, sub-states), which sits with unit naming. Neither stops core building on Windows.

### Specs follow the boundary

The generic specs name host facilities, the snapshotting volume store and the packet filter, rather than BTRFS and nftables; the Linux platform spec settles them with the detail the generic specs dropped.
The `nftables_failed` fault is `packet_filter_failed`.

## Build steps

Built one facility at a time rather than in the order below: each commit adds the core trait, moves the Linux adapter and wires the daemon, so the tree builds at every step. The crate itself arrived with the first facility moved, since an empty crate would warn of unused dependencies.

### Scaffolding

- [x] Create `crates/linux` as the `seedling-linux` package, depending on `seedling-core` and `seedling-protocol`, and add it to the workspace members
- [x] Make the `seedling` daemon depend on `seedling-linux`

### Traits for the facilities without one

Each trait goes beside the existing four in `system.rs`, with a stub in `system/stub.rs` for core's tests.

- [x] Log sink and log source (`system/journal.rs`): core builds a `JournalRecord` (level, message, `SEEDLING_*` fields) and keeps the field names; the sink is installed once with `journal::install_sink`, and the source is `System::journal`
- [x] Volume storage (`volume_store::VolumeStorage`): create, remove, is-snapshottable, snapshot and confined write. Core's `VolumeStore` stays a concrete type holding it, with the layout, held-volume records, site-name claiming and the owned-directory checks, which are portable `std` code
- [x] Backend connector: `backends::Connector` is public, and the platform's connector is `Platform::connector`
- [x] NAT64 translator: `system::AddressTranslator`, held as `System::translator`
- [x] Machine identifier: `node_prefix_from_machine_id` takes the identifier, which the daemon reads through `seedling_linux::host::machine_id`
- [x] `System::setup` takes a `Platform` (connector, data plane, journal, storage, translator); `setup_stubbed` takes the storage and journal, so stub mode keeps volumes and shows breadcrumbs as the host does, and fakes everything else

### Workload standing instead of Linux values

- [x] `TransientUnitSpec` carries a `UnitStanding` (infrastructure, or a workload's app and `WorkloadStanding`) in place of the slice, the OOM score and restart pacing
- [x] `ProcessManager::sync_groups` takes `AppStandings` (each app's priority and its Deployment levels, Normal always among them) in place of `sync_slices`, keeping its `prune` flag
- [x] The reconciler's `desired_standings`, `actuator/pod.rs`, `caddy/startup.rs` and `resolver/startup.rs` produce standings
- [x] Slice naming, weights, the OOM mapping and `desired_slices` live in `seedling_linux::systemd::priority` with their tests; `WorkloadStanding`, `UnitStanding` and `AppStandings` stay in core's `system/priority.rs`
- [x] Restart pacing is applied by the systemd manager to every unit it starts
- [x] `reserved.rs` has `INFRA_APP_NAME` as a literal of its own
- [x] The stub process manager records standings

### Moving the Linux adapters

- [x] `podman.rs`, `systemd.rs` and `systemd/`, with confinement moving alongside them as its last user in core
- [x] The journald reading and writing, as `seedling_linux::journal::Journal`
- [x] `data_plane.rs` and `data_plane/`, and `netinfo.rs`
- [x] `jool.rs`, as `seedling_linux::jool::Jool`
- [x] The BTRFS storage, as `seedling_linux::volume_storage::HostVolumeStorage`, with `is_btrfs`
- [x] `HostSource` and the `/proc/meminfo` read, as `seedling_linux::host`; `HostConstants::detect` takes the source
- [x] `SystemFacts::default()` reports memory undetermined. Core's own test builds fall back to a known memory figure instead, since the constants spec fails every evaluation while memory is undetermined

### Unix code that stays in core

- [x] `runtime/private_files.rs`: private file modes, directory modes and file identity, used by the database, secrets, audit log and TLS token
- [x] `runtime/local_socket.rs`: the Unix-socket HTTP client for the proxy's admin API and Tailscale's local API, which the plan did not foresee
- [x] Registry credentials: the container engine's primary file is the runtime directory on Linux and `~/.config/containers/auth.json` elsewhere, as containers-auth.json(5) says, with `USERPROFILE` standing for `HOME` on Windows
- [x] A signal table of core's own in `defs/container.rs`, used by the barrier's exit codes and by systemd's kill signal
- [x] Shells: the pty size, `kill` and `ExitStatusExt` under `cfg(unix)`, `todo!()` elsewhere; `ExecHandle` carries a `RawPty`
- [x] Unix-only tests under `cfg(unix)`, the shell tests that spawn jobs in `oi/shells/tests/jobs.rs`

### Dependencies

- [x] `nftables`, `rtnetlink`, `podman-rest-client`, `systemd` and `zbus` are `seedling-linux`'s
- [x] `nix` leaves core; `libc` is a `cfg(unix)` dependency and `rustix` a Linux one
- [x] Every other dependency builds for Windows and macOS (checked by cross-compiling with mingw and zig)

### Wiring the daemon

- [x] `crates/daemon/src/main.rs` builds the `Platform` from `seedling-linux` and installs the journal sink
- [x] The snapshotting check calls `seedling_linux::volume_storage::is_btrfs`, the Linux storage's own test, rather than core
- [x] `System` holds the `VolumeStore`, which holds the storage as a trait object

### CI and docs

- [x] A `portable` job in `.github/workflows/rust.yml` lints `seedling-core` with `--all-targets` on macOS and Windows
- [x] No doc named a moved module; `DEVELOP.md` says where platform code goes
- [x] Clippy, fmt, the spec style and citation checks and the CI greps pass
- [x] The reconciler files and clears the routes and packet-filter faults through one `settle_plane_fault`, which leaves a withheld apply's fault alone, and is tested directly
- [x] The workspace tests on Linux

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
- Moving `IDLE_CMD` or the supervisor's unit-state vocabulary to `seedling-linux`.
