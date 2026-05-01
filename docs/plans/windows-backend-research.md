# Windows backend — research notes

## Three framings considered

### A. Daemon-on-Windows-host, workloads-in-WSL2

Seedling daemon as a Windows service, driving podman/docker inside WSL2 over
some socket. BSL contract holds (Linux containers on a Linux kernel, just one
that lives in Hyper-V). Trait redesign accommodates it. Practical friction:
WSL2 networking is its own world, file-sharing performance across the
Windows/WSL2 boundary is bad, and the daemon-to-workload trust boundary
crosses a VM.

### B. Daemon-in-WSL2 (WSL2 hosts the host backend)

Seedling runs entirely inside a WSL2 Linux instance. Windows is just the bare
metal substrate. Not really a "Windows backend" — more a packaging story.
Initially the recommended path because the BSL contract, traits, and existing
host-backend code all stay unchanged.

### C. Native Windows containers

Workloads are Windows OCI images. Real "sibling backend". Massive scope —
roughly forking the runtime layer:

- Image format divergence (Linux OCI vs Windows OCI; non-overlapping
  ecosystems, the Linux ecosystem is ~all of it).
- Kernel-version coupling (process-isolation containers must match host
  closely; Hyper-V isolation lifts that but adds per-container VM overhead).
- Networking model entirely different (HNS, no nftables, no per-pod /64 v6).
- No systemd analogue (Windows Service Manager + Job Objects approximate it
  with different semantics).
- Snapshots via VSS or ReFS block clones, not BTRFS.
- ~80% of seedling-core has Linux undertones in concepts (image, mount, net,
  signal). Real Windows support is closer to forking the runtime than adding
  a backend.

Verdict: out of scope unless someone has a specific commercial reason to
orchestrate Windows-native workloads, and even then seedling probably isn't
the right tool — Windows already has Service Fabric, IIS, MSIX. The BSL value
proposition collapses without the Linux container ecosystem.

## Option B refined: WSL2 with a Windows Service

Initial decomposition of the Windows Service's responsibilities:

1. **WSL2 lifecycle.** Start the distro, restart on failure, dodge WSL2's
   `vmIdleTimeout` (8-minute idle shutdown by default) by keeping a process
   alive inside it. Wait on Hyper-V/LxssManager during service start.
2. **Network port plumbing** (originally — see "mirrored-mode trap" below).
3. **OI proxy.** Originally thought we'd need a TCP/HTTP proxy + a CLI shim;
   subsequently realised that with mirrored networking, no proxy is needed
   because Quinn-based QUIC ports on the WSL2 daemon are reachable directly
   from the Windows host. CLI runs as-is. Web UI hits Caddy directly.
4. **Storage controller.** This grew into the substantial item:

   - Allocate VHDX files (sparse, dynamic) for seedling state + workload
     storage.
   - `wsl --mount <path> --vhd --bare` to surface them as raw block devices
     inside WSL2. Real disks (`wsl --mount \\.\PHYSICALDRIVEn --bare`)
     supported the same way for Server installs with dedicated storage.
   - Single BTRFS pool spanning one or more block devices, subvolume per BSL
     named volume — matches the host backend's BTRFS-on-Linux semantics
     exactly. `HAS_SNAPSHOTS=true` via existing host-backend code paths, no
     new code in `crates/core`.
   - Capacity expansion: dynamic VHDX online-resize (`Resize-VHD` Windows-side
     + `btrfs filesystem resize max` inside WSL2), or add a new VHDX/disk to
     the pool (`btrfs device add`).
   - Mount lifecycle: Windows Service owns `wsl --mount` after every WSL2
     boot, before the daemon starts. Daemon can't come up cleanly until
     storage is mounted.

### Networking, originally

Two WSL2 networking modes:

- **NAT mode** (default, all Windows versions). WSL2 is a 172.x private
  network. Caddy inside WSL2 unreachable from outside the Windows host without
  `netsh interface portproxy add v4tov4` rules + Windows Firewall openings per
  ingress port. Service must keep these in sync as ingresses change. WSL2
  reboot reassigns the internal IP, so rules need refreshing. **Critical
  problem discovered late**: `netsh portproxy` is TCP-only, so QUIC (UDP)
  doesn't work through it at all. A UDP forwarder service would have to be
  written from scratch.
- **Mirrored mode** (Win11 22H2+ in theory). Caddy listens directly on the
  Windows host network. No portproxy. CLI + Web "just work" via
  `localhost:<port>`.

Initial recommendation: require mirrored mode, refuse NAT mode.

## The mirrored-mode trap

[microsoft/WSL#11154](https://github.com/microsoft/WSL/issues/11154) is the
relevant upstream bug. State as of 2026-05-01:

- **Open since 2024-02-14, two years.**
- **Confirmed broken on Windows Server 2025** across multiple builds throughout
  2025 and into 2026 (latest confirmation 2026-01-27 on Server build
  26100.32230 with WSL 2.7.0.0).
- Specific failure: `Wsl/Service/CreateInstance/CreateVm/ConfigureNetworking/
  0x803b0015` — mirrored networking can't initialise at all.
- Microsoft has not engaged in the thread. One commenter claims mirrored mode
  is "not supported and deprecated" on Server; that's a single user's
  assertion, not a Microsoft statement, but Microsoft's silence is
  consistent with WSL2-on-Server being a low priority.

Implications:

- Mirrored mode cannot be required if Windows Server is a target. And Server
  is the realistic target — nobody runs production apps on a workstation.
- NAT-mode fallback is dramatically worse than initially imagined: TCP-only
  portproxy means writing a UDP forwarder for QUIC, plus all the
  WSL2-IP-changes-on-reboot fragility, plus Windows Firewall profile
  interactions. This is a long tail of "weird bugs only on Windows Server"
  forever.
- WSL2 looks like a workstation feature that Microsoft has half-heartedly
  extended to Server. Building seedling on it for Server is building on sand.

## Pivot to Hyper-V VM

Rather than fight WSL2, use a normal Linux VM under Hyper-V.

**Trade-offs vs WSL2:**

| Concern | WSL2 | Hyper-V VM |
|---------|------|------------|
| Networking | Mirrored mode broken on Server; NAT mode requires custom UDP forwarder | External virtual switch = real bridged networking, fully supported on Server |
| Storage | VHDX-as-block-device + BTRFS, designed earlier | Same VHDX-as-disk + BTRFS, attached directly to the VM |
| Snapshots | BTRFS subvolumes inside WSL2 | BTRFS subvolumes inside the VM (identical) |
| Lifecycle | `wsl -d` + idle-timeout dodging | Hyper-V Manager / `Start-VM` |
| Boot time | Fast (shared kernel) | Slower (full VM boot) |
| Memory overhead | Low | Higher (full kernel, full userland) |
| Microsoft support stance | Workstation-focused, Server is best-effort | Production-grade on both Server and Workstation |
| Operator familiarity | Devs know WSL2; admins don't | Server admins know Hyper-V |

For a daemon running 24/7 on a server, the boot-time and memory overhead are
irrelevant. The networking and Microsoft-support-stance differences dominate.

**Hyper-V variant of the Windows Service responsibilities:**

1. Create or import the Linux VM at install (a known seedling-provided base
   image).
2. Attach VHDX disks: one for state (PVC-equivalent), one or more for the
   storage pool.
3. Attach the VM to an operator-chosen Hyper-V virtual switch (external for
   bridged, internal for host-only, or a private switch).
4. Start/stop/restart-on-failure of the VM. Wait on Hyper-V services during
   Windows boot.
5. Forward seedling daemon failure events to the Windows Event Log so admins
   see them where they look.

The seedling daemon binary inside the VM is the existing host backend, no
changes. The trait redesign from `k8s-backend.md` (host backend implementing
the new `Backend` traits) is what gets used.

### One path or two?

Considered:

1. **Hyper-V everywhere.** Same substrate for Win11 dev and Server prod. One
   set of bugs, one installer, one set of docs.
2. **WSL2 for Win11 dev, Hyper-V for Server prod.** Better local dev
   experience; doubles the supported-configuration surface, the test matrix,
   and the failure modes.

Lean: (1). WSL2's lightness on workstation is not worth doubling the
configuration surface, especially when Win11 users likely have Hyper-V
available anyway. If a Win11 dev wants tighter integration, they can run the
existing Linux build under WSL2 themselves manually — that's not a
seedling-supported path, but it's not blocked either.

## What stays unchanged

- BSL contract: untouched.
- Spec under `docs/spec/`: untouched.
- `crates/core` host backend: identical to the Linux build, runs inside the
  Linux VM.
- The trait redesign in `k8s-backend.md` is what makes this clean — host
  backend internals stay encapsulated.

## Decision points if revived

- Is Windows Server actually on the roadmap, or is "operator deploys a Linux
  VM themselves on whatever hypervisor" sufficient? If the latter, no
  seedling-side work is needed — just docs.
- One substrate (Hyper-V everywhere) or two (WSL2 + Hyper-V)?
- Installer mechanics: MSI, PowerShell module, or `seedling-ctl install
  --hyperv` driving the Hyper-V API directly?
- Upgrade story: replace VM image vs in-place upgrade inside the VM. The
  in-place path matches the Linux upgrade story exactly, so probably
  preferred.
- Does the Windows Service need its own UI (settings, diagnostics), or is
  Hyper-V Manager + Event Log enough?
- VM base image: build a custom seedling-flavoured image, or install on top
  of a stock Ubuntu / Debian / Alpine image? Custom is more turnkey, stock
  is more transparent.

## Open questions worth tracking

- Will Microsoft fix mirrored mode on Server, or quietly deprecate WSL2 on
  Server entirely? Watch microsoft/WSL#11154.
- Third-party WSL2 networking workarounds: one commenter on the issue
  mentioned `virtioproxy` "working well" — unclear what this is, may be
  worth investigating if WSL2 ever becomes the chosen path again.
- For Win11 development specifically, is "use the existing Linux build under
  WSL2 with whatever networking quirks the user can tolerate" sufficient as
  a community-supported path, distinct from a seedling-provided installer?

## References

- [microsoft/WSL#11154](https://github.com/microsoft/WSL/issues/11154) —
  upstream bug for mirrored mode failure on Server.
- `docs/plans/k8s-backend.md` — companion plan; the trait redesign there is
  the load-bearing piece for any non-Linux substrate.
