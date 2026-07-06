# seedling-mac — macOS dev environment

## Audience and scope

For **app developers** writing BSL apps on macOS — not for production
deployment on macOS hosts. Goal: a one-command install that gives a Mac dev
the full seedling experience for building and testing apps, with parity to
what they'd get on a real Linux server.

Single-VM only. Multi-instance / project-profiles is explicitly out of scope.
One Mac runs one seedling-mac VM. If a dev needs isolation between projects,
they uninstall and reinstall, or use the existing app-namespace separation
inside the one VM.

## The principle

A seedling-mac install runs the **standard host backend** inside an
Apple-Virtualization-framework Linux VM. The capability set is identical to
the host backend on bare-metal Linux. We don't gate anything based on "this is
dev". Restrictions exist only where they're a real consequence of the
environment (no public IP on the host, no second machine on the LAN), and
even those have first-class workarounds.

This means an app developer can:

- Run the full BSL feature set, including signals, image warming, volume
  snapshots, BTRFS subvolumes, healthchecks, the works.
- Develop and test backup apps with real `VolumeSnapshot`-equivalent semantics
  (BTRFS on loop device — see Storage below).
- Get real, browser-trusted Let's Encrypt certs for their app ingresses if
  they own a DNS zone (DNS-01 challenge — see Networking below).
- Use the same `seedling-ctl` binary (cross-compiled darwin) and same Web UI
  they'd use against any other seedling daemon.

## Substrate: podman machine on Apple Virtualization framework

`podman machine` is the chosen substrate. Reasons:

- **Aligned with seedling's existing podman backend.** The smallest-delta
  substrate — podman is what the host backend already drives. The seedling
  daemon inside the VM is byte-for-byte the Linux build.
- **Apple Virtualization framework underneath.** Modern, supported, fast on
  Apple Silicon.
- **Free and OSS.** No commercial licence concerns.
- **Mature image distribution.** `podman machine init` pulls a known-good
  Fedora CoreOS image, which is a fine base.
- **Native virtiofs file sharing** — see File sharing below.

Lima/Colima would also work but adds a layer for marginal benefit. OrbStack
has the best UX but is commercial and unclear on integration licensing for
tooling like ours. Apple's `container` CLI is too new and too limited.
Decision: podman machine, with the option to revisit if a better substrate
emerges.

## Architecture

```
macOS host
├─ seedling-mac (Homebrew formula)
│   ├─ launchd LaunchAgent: ensures VM is up at login (configurable off)
│   ├─ seedling-ctl (existing CLI, cross-compiled darwin-arm64 / amd64)
│   └─ seedling-mac CLI: thin VM lifecycle wrapper
│       (init / start / stop / reset / status / logs / resize)
└─ Linux VM (Apple Virtualization framework, via podman machine)
    ├─ seedling daemon (host backend, byte-for-byte the Linux build)
    ├─ podman
    ├─ BTRFS-on-loop-device for /var/lib/seedling/volumes
    ├─ Caddy (existing)
    ├─ ext4 root for /
    └─ ~ virtiofs-shared from host at the same path
```

## Storage

The VM is provisioned with two virtual disks:

- **Root disk**: ext4, the OS + seedling daemon binary + ephemeral state.
  Sized small (e.g. 4–8 GB), backed by the podman machine image.
- **Storage disk**: a sparse raw image file mounted into the VM as a block
  device, formatted BTRFS, mounted at `/var/lib/seedling/volumes`. Default
  size 50 GB sparse (only consumes what's used). Configurable at install
  time and growable post-install (`seedling-mac resize`).

BSL named volumes become BTRFS subvolumes in the storage disk. This gives
identical semantics to the host backend on a real BTRFS-formatted server:

- `HAS_SNAPSHOTS = true`.
- Subvolume snapshots, source/destination clone semantics.
- Backup app development works end-to-end with real snapshot-driven flows.

Loop-device BTRFS isn't quite as performant as BTRFS on bare metal, but for
a dev workload it's fine — and crucially, it exercises the same code paths
the production backend uses, so backup apps developed on Mac behave the same
on a server.

The storage disk is independent of the OS disk. `seedling-mac reset` (without
`--keep-storage`) wipes both; with `--keep-storage` it rebuilds the OS but
keeps installed apps and their data.

## Networking

The VM gets an Apple-Virtualization-framework-managed network interface
(vmnet shared mode by default — VM has its own IP, reachable from the host
and from any process on the host's LAN if the operator chooses to advertise
it).

Address resolution for ingress hostnames is the operator's choice and is
explicitly outside seedling-mac's scope to manage:

- **Tailscale.** VM joins the dev's tailnet, ingress hostnames resolve via
  the dev's DNS to the VM's tailnet IP. Reachable from the dev's phone, other
  devices, etc. Probably the nicest setup.
- **Local DNS** (Pi-hole, dnsmasq, mDNS via Avahi-on-the-VM): resolves to
  the VM's IP on the LAN.
- **`/etc/hosts` + port-forward.** Simplest "I'm just testing on this Mac"
  setup. seedling-mac can optionally manage port-forwards from
  `localhost:<port>` to the VM, exposed as a CLI subcommand.

Cert acquisition is **orthogonal to address routing**:

- **ACME DNS-01** works from anywhere with no inbound reachability — the
  operator provides DNS API credentials (Cloudflare, Route53, etc.) for a
  zone they control, BSL ingresses use hostnames in that zone, certs are
  real Let's Encrypt certs. This is the primary recommended setup for devs
  who own a domain.
- **ACME HTTP-01 / TLS-ALPN-01** work only if the VM is publicly reachable
  on the relevant port. For most dev Macs this isn't the case, but it's not
  artificially blocked — if the dev has a setup where it works (e.g.
  Tailscale Funnel for HTTPS-on-tailnet-FQDN, or genuine port forwarding
  from a public IP), it'll work as in any other host backend deployment.
- **Internal CA** for fully-offline dev: existing host-backend feature, works
  in seedling-mac unchanged.
- **Manual / self-signed** for "I just want to click through the warning":
  also works, also unchanged.

QUIC is the OI transport (Quinn). The CLI on the macOS host connects to the
daemon's QUIC port on the VM's IP directly. No proxy, no shim, no port
forwarder needed for OI. Web UI same story — Caddy in the VM listens on its
ingress port, browser hits it via the VM's IP (or Tailscale, or a `/etc/hosts`
entry, etc.).

## File sharing

`~` is virtiofs-shared from the macOS host into the VM at the same path. So:

```
$ cd ~/projects/myapp
$ vim app.bsl                # editing on macOS
$ seedling-ctl install ./app.bsl   # install path resolves identically inside VM
```

This means the dev's normal editor workflow works without any sync, copy, or
path-translation step. Live-reload during dev iteration (mounting source code
into a container) also works — the BSL volume binding sees the host-backed
path through virtiofs.

Performance is acceptable for source-code-sized workloads. Heavy I/O loads
(database storage, build caches) should be in BTRFS volumes (VM-internal),
not on the virtiofs share.

## CLI surface

Two binaries ship in the Homebrew formula:

- **`seedling-ctl`** — the existing OI CLI, cross-compiled to
  `darwin-arm64`/`darwin-amd64`. Connects to the daemon over QUIC at the VM's
  IP. Usage is identical to a Linux host. Connection target configured once
  at `seedling-mac init` time.
- **`seedling-mac`** — VM lifecycle wrapper. Subcommands:
  - `init` — first-run setup: download image, allocate storage disk, start
    VM, perform first-boot configuration, register launchd agent. Idempotent
    (re-running on an installed system surfaces config knobs).
  - `start` / `stop` / `restart` — VM lifecycle.
  - `status` — VM running? daemon healthy? storage utilisation?
  - `logs` — tail daemon / VM journal logs.
  - `reset [--keep-storage]` — destroy VM (and optionally storage), recreate
    from base image. The "my dev env is broken" escape hatch.
  - `resize <new-size>` — grow the storage disk.
  - `shell` — interactive shell into the VM (debugging escape hatch).

Both binaries live in `/opt/homebrew/bin/` (or `/usr/local/bin/` on Intel)
and are signed + notarised so Gatekeeper doesn't complain.

## launchd integration

A `LaunchAgent` plist is installed at
`~/Library/LaunchAgents/eu.bearcove.seedling-mac.plist`, configured to:

- Start the VM at user login (default; togglable via
  `seedling-mac config startup auto|manual`).
- Restart the VM on failure (with backoff).
- Forward daemon-level failure events to macOS Console.app via `os_log` so
  Mac admins see seedling problems where they look.

The agent is deliberately a `LaunchAgent` (per-user) rather than a
`LaunchDaemon` (system-wide). Dev use is per-user; we don't need root, and
the VM doesn't need root.

## VM image distribution

The base VM image is built in CI and distributed as a release artifact.
`seedling-mac init` downloads the matching image for the installed
seedling-mac version. The image contains:

- Fedora CoreOS or Alpine (TBD — Alpine is smaller, Fedora has better
  out-of-the-box podman/BTRFS tooling).
- Seedling daemon binary at the version matching the macOS-side install.
- Pre-configured systemd units to start the daemon at VM boot.
- BTRFS tooling, podman, Caddy.
- vsock/virtio-fs guest tools.

Image versioning matches seedling-mac binary versioning. Upgrades download
the new image and re-base the VM (preserving the storage disk).

## Architecture variants

- **Apple Silicon (M1+)**: native ARM64 inside the VM. Multi-arch images
  (postgres, nginx, redis, caddy, etc. — basically all common projects)
  run natively. This is the primary target.
- **Intel Mac**: ARM64-only images need QEMU emulation inside the VM (slow
  but functional). amd64 images run natively. Best-effort support; Intel
  Macs are EOL hardware and we shouldn't gate features on them. Document
  the QEMU-emulation cost and move on.

The VM image is shipped in both arm64 and amd64 flavours; podman machine
picks the right one for the host arch.

## Capabilities reported

Identical to host backend on a Linux server with BTRFS:

- `supports_signals = true`
- `supports_image_warming = true`
- `supports_volume_snapshots = true`
- `supports_btrfs_subvolumes = true`
- `has_ipv4_egress` = host-derived
- `has_ipv6_egress` = host-derived

No artificial gating. If a BSL app installs on a Linux server, it installs in
seedling-mac. The only environmental difference is what the dev chooses to do
about address routing and certs, which is operator-config in any
environment.

## Bootstrap

```
brew tap bearcove/seedling
brew install seedling-mac

seedling-mac init                        # one-time setup; ~30s on Apple Silicon
                                          # (downloads VM image, creates VM,
                                          #  starts daemon, generates auth keys)

seedling-ctl install ./myapp.bsl         # works as on Linux
seedling-ctl logs myapp                  # works as on Linux
```

(Tap name is a placeholder. Final naming TBD.)

## Phasing

1. **Cross-compile seedling-ctl to darwin-arm64 / darwin-amd64.** Ensure the
   QUIC client + protocol crate work on macOS (likely zero changes needed
   given Quinn is portable; verify in CI).
2. **Build the VM image pipeline.** CI job that produces a versioned VM
   image with the daemon baked in. Decide Fedora CoreOS vs Alpine.
3. **`seedling-mac` lifecycle CLI.** VM bring-up via podman machine, storage
   disk allocation, BTRFS format on first boot, virtiofs share configuration.
4. **launchd LaunchAgent.** Install/uninstall, restart-on-failure, log
   forwarding to Console.app.
5. **Homebrew formula.** Tap setup, signing/notarisation, version pin
   alignment between seedling-mac CLI and VM image.
6. **Documentation.** Dev workflow guide, ingress / DNS-01 setup walkthrough,
   Tailscale integration recipe. Lives under `docs/seedling-mac/` (operator
   guides, not in `docs/spec/`).
7. **Hardening.** Reset / resize / upgrade flow polish, error messages,
   common-failure recipes.

## What stays unchanged

- BSL contract: no changes.
- `docs/spec/`: no changes. seedling-mac is packaging, not new behaviour.
- `crates/core`: no changes. The host backend runs unmodified inside the VM.
- The trait redesign work in `k8s-backend.md` is independent of seedling-mac
  and remains the priority for backend-abstraction work.

## Open questions

- **Image base distro: Fedora CoreOS or Alpine?** Fedora has better
  out-of-the-box BTRFS + podman + systemd-modern tooling; Alpine is smaller
  and faster to download. Probably Fedora CoreOS (closest to what production
  servers will run). Decide during phase 2.
- **Storage disk format on first boot.** Allocate sparse and format BTRFS
  during `init`, or defer to first daemon start? Probably during `init` so
  init failures are visible immediately.
- **VM resource defaults.** 4 GB RAM, 4 vCPU, 50 GB sparse storage feels
  right for a dev environment. Configurable via `seedling-mac config`.
- **Upgrade story for the VM image.** Re-base preserving storage disk works
  for major upgrades; for in-place daemon-only upgrades, possibly support
  swapping the daemon binary in the running VM via a podman machine ssh
  pathway. Phase 7.
- **Multi-arch image-pull behaviour.** When a BSL app references a
  registry/image-only-on-amd64 and we're on Apple Silicon, do we silently
  enable QEMU emulation, fail loudly, or warn? Probably warn-on-install,
  fail-on-pull-if-no-platform-match, with a `--allow-emulation` opt-in.

## Non-goals (explicit)

- Multiple seedling-mac instances per Mac (project profiles, etc.). Single
  VM, single daemon, period.
- Production deployment on macOS hosts. macOS isn't a server platform for
  seedling.
- Native macOS workloads (process-level seedling on Mac). Same reasoning as
  the Windows native case — out of scope, would be a different product.
- Headless / unattended Mac use. seedling-mac assumes there's a logged-in
  user; CI use cases should run a Linux build directly.

# Addendum: daemon development loop

## Audience and scope

The plan above targets **app developers** writing BSL on macOS. This addendum
covers **daemon developers** — people hacking on seedling itself from a Mac.

The frontends are already fine natively: `seedling-ctl`, `seedling-protocol`,
and `seedling-web` (plus the Vite frontend) have no Linux-only dependencies
and build on darwin today. The daemon does not: `seedling-core` links
**libsystemd** (journal reads in `system/journal.rs`, journal sends in
`system/breadcrumb.rs` — the `SystemdManager` itself is zbus and portable),
so `cargo build -p seedling` fails on macOS at the `libsystemd-sys`
pkg-config step, before any of the runtime's Linux assumptions even matter.

So the loop is necessarily: **edit on macOS, compile somewhere Linux, run in
the seedling-mac VM.** Two loops for the compile step, in order of
recommendation.

## Loop A: compile inside the VM (baseline, works day one)

The repo checkout lives on the Mac and is visible inside the VM through the
virtiofs `~` share at the same path — so the dev edits in their normal macOS
editor with zero sync. Compilation runs inside the VM:

- **`CARGO_TARGET_DIR` on VM-local disk** (e.g. `/var/cache/seedling-dev/target`),
  never on the virtiofs share. Source reads over virtiofs are fine (source
  trees are small); a `target/` dir over virtiofs is unusable.
- **Rust toolchain inside the VM** via rustup on first use, or baked into a
  `-dev` flavour of the VM image (open question below).
- **`watchexec` inside the VM needs `--poll`** — inotify events don't
  propagate across virtiofs, so the stock `watch-build` recipe won't fire on
  mac-side edits without it. Same applies to `watch-run` watching the target
  dir.
- `watch-run` itself is unchanged: it's already a Linux-side invocation.

On Apple Silicon the VM is virtualised, not emulated, so rustc speed inside
the VM is close to native — the real costs are virtiofs metadata scans on
incremental builds and RAM pressure. Bump the dev VM to ~8 GB / all cores
(`seedling-mac config`); the app-dev defaults (4 GB / 4 vCPU) are tight for
`cargo build` on this workspace.

## Loop B: cross-compile on the host (optimised, needs setup)

Rustc runs natively on macOS targeting the VM's triple —
`aarch64-unknown-linux-gnu` on Apple Silicon, `x86_64-unknown-linux-gnu` on
Intel — and the binary lands on the virtiofs-shared path where the VM-side
`watch-run --poll` picks it up (or a mac-side recipe restarts the daemon via
`podman machine ssh`). Fastest incremental loop; no VM resources spent on
compilation.

The catch is C dependencies:

- **aws-lc-sys** (rustls default crypto provider, via quinn/wtransport) and
  **rusqlite's bundled SQLite** compile C. `cargo-zigbuild` handles both —
  zig cc provides the Linux cross toolchain and lets us pin the glibc version
  to the VM image's (`--target aarch64-unknown-linux-gnu.2.3x`). Host needs
  `cmake` for aws-lc-sys.
- **libsystemd** is the real friction: zig provides libc, not libsystemd. The
  cross build needs a sysroot containing `libsystemd.so`, its headers, and
  its `.pc` file, extracted from a container image of the VM's distro
  (`podman create` + `podman cp`), with `PKG_CONFIG_SYSROOT_DIR` /
  `PKG_CONFIG_PATH` / `PKG_CONFIG_ALLOW_CROSS=1` pointed at it. This should
  be a one-shot `just mac-sysroot` recipe, re-run when the VM image's distro
  version bumps.

Verdict: worth building, but as a phase after Loop A works — Loop A has no
setup cliff and is acceptable on Apple Silicon. Loop B matters most for
heavy daemon iteration and for Intel Macs (where in-VM compiles compete with
an already-slower machine).

## Loop C: no VM — remote Linux

Everything in `DEVELOP.md` as-is on any Linux box (physical, cloud, or a
teammate's server), with the Mac as a thin client: editor via remote-SSH or
git push/pull, `seedling-ctl` (darwin build) and the browser talking QUIC/HTTP
to the remote daemon directly. Zero engineering, network-dependent. Fine as a
fallback or for devs who already live in remote-dev tooling; not the
recommended default because it abandons the offline, self-contained property
seedling-mac exists to provide.

## What works natively on macOS (no VM at all)

- `cargo build` / `test` / `clippy` for `seedling-ctl`, `seedling-protocol`,
  `seedling-web`; the Vite frontend, vitest, and (against a stubbed daemon)
  Playwright e2e.
- **Not** `seedling-core` / `seedling` — blocked by the libsystemd link.

A possible future quality-of-life knob: feature-gate the journal integration
so `cargo check --workspace` and clippy pass on darwin for editor/CI
ergonomics. Explicitly **not** proposed: runtime stubs or a mock backend to
*run* the daemon on macOS — that's the backend-trait discussion in
`k8s-backend.md`, and faking podman/nftables/jool on a dev laptop diverges
dev behaviour from production for no gain over the VM.

## Tests

Unit/integration tests for `protocol`, `web`, and the frontend run natively
on macOS. `seedling-core` tests run inside the VM (Loop A environment):
`podman machine ssh` + `just test`, with the same `CARGO_TARGET_DIR` note.
Don't bother making `just test` cross-execute via zigbuild + a runner; the VM
is right there.

## Justfile sketch

Names TBD; the shape:

```
mac-build:      # Loop B: cargo zigbuild --target <triple> (watchexec on host)
mac-sysroot:    # one-shot: extract libsystemd sysroot from VM-distro container
mac-restart:    # podman machine ssh -- systemctl restart seedling-dev
mac-test:       # podman machine ssh -- just test  (VM-local target dir)
```

Loop A needs no new recipes beyond documenting `--poll` and
`CARGO_TARGET_DIR`; consider folding both into `watch-build`/`watch-run`
automatically when a virtiofs mount is detected.

## Open questions

- **`-dev` VM image flavour?** Baking rustup + toolchain + `just` +
  `watchexec` into a dev variant of the CI-built image avoids per-dev setup,
  at the cost of a second image in the pipeline. Alternative: a
  `seedling-mac dev-setup` command that rustup-installs into the standard
  image.
- **inotify over virtiofs.** Confirm on current podman machine / AVF versions
  whether events propagate (behaviour has shifted across virtiofsd versions);
  if they do, drop `--poll`.
- **Sysroot pinning.** The Loop B sysroot must track the VM image's distro
  release; decide whether `mac-sysroot` derives the container tag from the
  installed image version automatically.
- **Daemon-under-test vs seedling-mac's own daemon.** A daemon dev's
  `watch-run` instance and the LaunchAgent-managed production-ish daemon
  fight over the data dir and ports. Simplest rule: daemon devs run
  `seedling-mac config startup manual` and own the daemon lifecycle
  themselves; document it.
