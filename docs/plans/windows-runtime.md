# Windows Runtime: Plan, Open Questions, Spikes

Companion to the draft `runtime-windows.md`. Status as of 2026-07 design sessions.

## Where this fits

The goal is a second Seedling implementation with Windows-native primitives speaking the same operator interface, so the existing CLI (`ctl`), web UI, and protocol crates carry over unchanged. `interface.md` is the shared conformance surface; `language.md` is shared with capability-gated divergences; `runtime.md` splits into portable semantics and per-platform infrastructure.

## Workstreams

### 1. Spec restructuring (prerequisite for merge, not for prototyping)

- Extract the portable parts of `runtime.md` — reconciliation, generations, lifecycle operations, barriers, history/audit, faults, scheduling, GC principles — into a shared document (`runtime-core.md` or similar). Linux infrastructure (Podman, systemd, nftables, NAT64/jool, ULA-from-machine-id, volume snapshots) stays in `runtime-linux.md`.
- Add the `runtime-windows` spec entry (`w[...]` namespace) to `.config/tracey/config.styx`.
- Add a `capabilities` field to `/status` in `interface.md`, and `runtime.capability()` to `language.md`, with the shared vocabulary from `w[capability.map]`. Audit `i[...]` rules for ones that become capability-conditional (image endpoints are *not* among them — the OCI artifact design keeps them; snapshot/backup endpoints are, pending the backup rework).
- Restate `i[shell.exit]` to define negative codes as "terminated by the runtime" (platform-neutral wording; no semantic change on Linux).
- Conformance suite keyed to rule IDs, run against both daemons in CI. The failure mode it guards against is semantic drift, not wire incompatibility.

### 2. Backup rework (cross-runtime, separate track, ordering matters)

Replaces the flexible backup-app strategies with one embedded kopia method on all runtimes. Interactions to resolve *before* Windows implementation starts on adjacent areas:

- The operation-volume machinery (`r[operation.volume-param]` family, `kind: "volume"` params, reserved `_volume`/`_filename` keys) exists almost entirely for backup apps. If the rework removes its last consumers, drop it from the portable spec and from `w[action.volume-params]` rather than porting it.
- Scheduled backup fires were the main dynamic-Job churn source; their removal mostly defuses the per-invocation service-registration watch-items.
- Decide seedlingd's own principal (LocalSystem vs own virtual account) — determines how the embedded backup engine gets volume read access (`w[backup.v1]`).

### 3. Windows daemon + seedpod implementation

Follows the draft spec. Sequencing suggestion: supervisor process model and reattachment first (everything else composes with it), then networking/WFP, then artifacts/attach, then shells/actions.

## Open questions (decisions needed, owner: spec sessions)

| # | Question | Current lean |
|---|----------|--------------|
| Q2 | IPv4 fallback for v6-incapable dialers: 127/8 aliases vs none | Provide per-service v4 alias; verify Windows 127.x bind behavior in Spike C |
| Q3 | Pipe protocol frame format and versioning | Length-prefixed frames, hello with version + feature bits; pin during Spike A/B |
| Q4 | seedlingd's own service account | Prefer own virtual account for least privilege; requires explicit ACE for backup reads |
| Q5 | Installer/packaging for seedlingd itself + upgrade path | Chocolatey package vs MSI; not blocking design |
| Q6 | Whether `service_stop` profiles (Postgres) are reachable from `rt.signal` mapping or only from the stop ladder | Ladder only; `rt.signal(SIGTERM)` on a service-profile instance still means TerminateJobObject — revisit if a script needs "smart shutdown" semantics |
| Q7 | Windows counterpart of `threat-model.md`: full doc or section | Full doc, seeded from `w[wfp.honesty]` + governance ledger |
| Q8 | Migration/import path design (PM2+tarball → seedlingd, Postgres adoption, Caddy config takeover) | Idempotent, resumable per host; doctor verdict gates sequencing |

## Spikes (confirm before the corresponding rules lose their `[spike]` tag)

**A. Stop delivery + Job Objects** (afternoon). CTRL_BREAK into a `CREATE_NEW_PROCESS_GROUP` Node child sharing the supervisor's console, under a Job: clean cooperative shutdown, group event not striking the supervisor, exit-code capture, TerminateJobObject exit-code synthesis. Also measure CreateService+StartService dispatch latency for the dynamic-Job decision.

**B. ConPTY over QUIC** (afternoon). Bridge a ConPTY session across three streams with resize; confirm the merged-output behavior against what the web terminal tolerates. Side task: verify whether the Linux daemon already effectively merges stderr for TTY-attached shells (determines whether `w[shell.conpty]`'s empty-stderr note is a divergence or existing behavior).

**C. Networking on a worst-case image** (1–2 days; run against a disk image of the least-friendly of the 5 deployments, not a lab box). Loopback ULA aliases + `skipassource`; v4 127.x bind behavior (Q2); NRPT rule application and removal; WFP provider/sublayer install and ALE loopback classification coexisting with the deployment's actual EDR; `BIND_ADDRESS` end-to-end with a real Tamanu build including `w[net.bind-verify]` via the extended TCP table.

**D. Run Tamanu from read-only VHDX** (half day, mostly done by the Tamanu build work). Attach a produced artifact read-only, launch from the config blob, watch for writable-app-dir assumptions (temp files, logs beside code). Confirm decompressed-store digest verification cost per attach is negligible.

**E. Identity mechanics** (half day). Virtual-account logon under a hardening GPO baseline; stripped-token spawn; DeleteService-with-open-handles ghost behavior and the GC sweep for it; NTFS ACE inheritance breaking on volume creation.

## Rollout

- Fleet reality: ~5 deployments, ~25 Windows hosts, varying circumstances. Drift is assumed.
- Build `seedling doctor` early: per-host preflight for NRPT, WFP provider install, virtual-account logon, VHDX attach, Defender exclusions, Server version — reported through the same capabilities vocabulary as `/status`, aggregatable fleet-wide.
- Run doctor across all 25 hosts *before* the pilot; the resulting support matrix chooses the pilot (friendliest host) and the sequencing (weirdest last).
- Migration is per-host, idempotent, resumable: adopt existing Postgres service, take over Caddy config, replace PM2 supervision; no rebuild-from-scratch at field sites.

## Already settled elsewhere (do not reopen)

- `BIND_ADDRESS` format (as landed in Tamanu): comma-separated `IP:PORT` list, v6 bracketed, one entry per declared listener in declaration order; supersedes `PORT` when present. Speced in `w[net.bind-address]`.

- Artifact format and pipeline: Tamanu `vhdx-pack` is the reference producer; uncompressed-checksum annotation queued.
- ReFS inside artifact VHDXs: rejected (format-version drift, no benefit); NTFS normative. ReFS as *host volume* filesystem: opportunistic capability only.
- Reboot survival, snapshots, scaling, outbound-deny: v1 non-goals per `w[platform.non-goals]`.
- WSL2, Windows containers, wintun netstack, local accounts, AppContainer: rejected; rationale, fallback positions, and the governance ledger live in `windows-design-rationale.md`.
