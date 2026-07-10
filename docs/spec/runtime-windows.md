The Seedling Windows Runtime is a second implementation of the Seedling runtime, targeting Windows Server hosts with native-process primitives instead of Linux containers. It conforms to the operator interface spec and the language spec, and to the portable portions of the runtime spec (reconciliation, generations, lifecycle operations, barriers, history, faults, scheduling), while replacing the Linux infrastructure sections (Podman, systemd units, nftables, NAT64, volume snapshots) with the mechanisms defined here.

> **Draft status.** This document records design decisions from the 2026-07 design sessions. Rules marked `[spike]` depend on prototype confirmation; see the companion plan document. Rejected alternatives and fallback positions are recorded in `docs/plans/windows-runtime-rationale.md`, not here. Rule IDs use the `win[...]` namespace, tracked as the `runtime-windows` spec in tracey.

Where this document is silent, the portable runtime spec applies unchanged. Where a Linux runtime rule (`r[...]`) is cited as *replaced*, the replacement here is normative for the Windows runtime and the cited rule does not apply.

# Scope and Platform

> win[platform.floor]
> The Windows runtime supports Windows Server 2019 and later, x64 only. All mechanisms specified here (ConPTY, NRPT, WFP ALE-layer classification of loopback traffic, `skipassource` address flags, VHDX attach APIs) must be available on the platform floor; a mechanism requiring a later version must be gated behind a [capability](#win--capability.map).

> win[platform.non-goals]
> The following are explicitly out of scope for the Windows runtime v1: workload survival across host reboot ([win[boot.cold]](#win--boot.cold)), volume snapshots and the snapshot-based backup strategies, Windows containers, WSL2, horizontal scaling (replica count is fixed at 1 per Deployment), blue/green instance replacement ([win[deploy.stop-start]](#win--deploy.stop-start)), and outbound-deny network policy.

# Boot and Reboot Semantics

> win[boot.cold]
> Workloads do not survive host reboot. Workloads must survive the seedlingd daemon being stopped, crashed, or upgraded within a boot.
>
> After host reboot, the daemon performs a cold reconciliation pass: it observes an empty world and starts all resources itself, in dependency order derived from the mount graph (database services before their dependents, ingress after backends). Boot is not a special case; it is reconciliation from an empty world.

> win[boot.replay]
> Interrupted-operation replay at boot (per `r[operation.lifecycle.events]`) composes with cold start: the replayed operation executes against a world in which no instance from the previous boot is running. Barrier replay rules must tolerate this — a replayed `rt.start()` finds nothing to adopt and starts fresh; a replayed observation of a terminated resource is satisfied vacuously. The action log's at-most-once guarantees (e.g. `r[rt.signal]`) apply unchanged.

# Deploys and Replacement

> win[deploy.stop-start]
> The Windows runtime v1 admits a weaker replacement model than the Linux runtime's rolling update (`r[update.rolling]`) and healthcheck replacement (`r[autonomous.healthcheck-replace]`), which this rule replaces: deploys and health-driven replacement are stop-then-start of the single instance. No replacement instance runs alongside the old one, and no traffic shifting occurs.
>
> The supervisor persists across the swap ([win[identity.lifecycle]](#win--identity.lifecycle)) and keeps holding the instance's service addresses ([win[net.listener]](#win--net.listener)), so inbound connections queue during the swap rather than being refused; the disruption window is the workload's stop plus start time, not a connection-refused outage.

# Process Model

The Windows runtime uses a per-instance supervisor process (working name **seedpod**), analogous to conmon in the podman stack: a small process that owns the workload so that neither the daemon's lifetime nor its address space is coupled to the workload's.

> win[supervisor.ownership]
> Each running instance is owned by exactly one supervisor process. The supervisor owns:
>
> - the **Job Object** enclosing the instance's process tree;
> - spawning the workload process and restarting it per policy on exit, without daemon involvement;
> - the instance's [service-address listeners](#win--net.listener) and relay;
> - stdout/stderr capture to the log store, and exit-status recording;
> - execution of the [stop sequence](#win--stop.ladder).
>
> The daemon is pure control plane: it reconciles desired state, creates, commands, and removes supervisors, and sits in no data path and no process ownership chain.

> win[supervisor.breakaway]
> Supervisors are spawned outside any job that terminates with the daemon: with `CREATE_BREAKAWAY_FROM_JOB` when the daemon is in a job permitting breakaway, and with no inheritable daemon handles. If the daemon runs inside a job without `JOB_OBJECT_LIMIT_BREAKAWAY_OK`, the runtime must fail supervisor creation with a diagnostic naming the restriction rather than silently nesting (nested jobs are legal on the platform floor but couple supervisor lifetime to the daemon's job, violating [win[boot.cold]](#win--boot.cold)'s within-boot survival property).

> win[supervisor.record]
> For each supervisor, the daemon persists an on-disk record containing at minimum: instance identity, supervisor PID, supervisor process start time, pipe name, assigned addresses and ports, and the instance's generation. PID + start time together disambiguate PID reuse: on reattachment, a record whose (PID, start time) does not match a live process describes a dead world and must be garbage-collected, not adopted.

> win[supervisor.reattach]
> Each supervisor serves a named pipe (`\\.\pipe\seedling-<instance-id>`) carrying: status reports, exit events that occurred while the daemon was down, and daemon commands. On startup and after restart, the daemon enumerates supervisor records, reattaches to each live supervisor's pipe, re-opens a waitable handle on the supervisor process, and reconciles any missed events into the observation history.

> win[supervisor.pipe-protocol]
> The pipe protocol must be version-skew-tolerant: a newer daemon must interoperate with older supervisors, since upgrading a supervisor requires draining its workload. The protocol opens with a hello frame carrying a protocol version and feature bits; unknown frame types are skipped, not fatal. The protocol should be pinned early and change rarely. `[spike]`

> win[supervisor.crash]
> The daemon detects supervisor death by waiting on the supervisor process handle. Supervisor death terminates the Job Object and therefore the workload; the runtime treats this as instance death and the reconciler restarts the instance per policy. Consequence to keep true: the supervisor's reliability budget is the workload's. The supervisor must remain small; features that grow its failure surface belong in the daemon.

The supervisor is also the data plane for mounts ([win[net.mount]](#win--net.mount)): a relay defect or resource exhaustion in the supervisor terminates the workload with it. This coupling is a stated property of the v1 design (alternatives are recorded in the design rationale document).

# Identity and SCM Integration

> win[identity.scm-entry]
> Exactly one Seedling component is registered as an auto-start SCM service: seedlingd itself. No workload or supervisor service is boot-started ([win[boot.cold]](#win--boot.cold)).

> win[identity.virtual-account]
> Each supervisor is registered as a `SERVICE_DEMAND_START` SCM service named `seedling-<instance>`, configured to run as its virtual service account `NT SERVICE\seedling-<instance>`. Registration exists for identity provisioning, not supervision: the SCM mints the account with no account object, no password, no password-policy interaction, and no interactive logon, and its SID is deterministic (derived from the service name), so the daemon can compute ACLs and WFP filters before first start.
>
> The daemon starts supervisors in dependency order via `StartService`. Supervisor services must not declare a dependency on seedlingd; workloads keep running with the daemon down. The supervisor carries a minimal service-control shim (report status, translate SERVICE_CONTROL_STOP into the [stop sequence](#win--stop.ladder)).

> win[identity.lifecycle]
> Service registration follows instance lifecycle, not deploys: the service is created when the instance is created and deleted when the instance is removed; deploys are stop/start of an existing service. Before calling `DeleteService`, the daemon and supervisor must close all handles to the service, since deletion is deferred while handles remain open; the [GC pass](#win--identity.gc) reaps registrations stuck in the marked-for-delete state.

> win[identity.dynamic-jobs]
> Dynamic-scope Jobs (action-spawned and shell-attached, per `l[job.type]` dynamic scope) also receive their own demand-start service and virtual account per instance. The full grant set — service registration, volume ACEs, WFP allows — is created before the Job starts and removed at operation (or shell session) end.
>
> Consequences the runtime must own: creation order is registration → ACEs → filters → start, teardown is the reverse, and both must be crash-safe. Scheduled actions generate service-install audit events (4697) per fire; the audit-event profile belongs in the Windows threat-model document.

> win[identity.gc]
> The reattachment/GC pass must sweep identity artifacts as well as processes: `seedling-`-prefixed service registrations, volume ACEs granting Seedling SIDs, and WFP filters under the Seedling provider GUID that match no live instance record or in-progress operation are removed. This extends the stuck-state recovery principle of `r[operation.cancel.stuck-recovery]` to identity residue.

> win[identity.non-admin]
> Workload processes never run elevated. The supervisor logs on as the instance's virtual account and spawns the workload inside the Job Object under a stripped token (restricted, no extra privileges, derived from the supervisor's own), so the workload holds a narrower slice than the supervisor.
>
> Recorded v1 limitation: workload and supervisor share a SID and are mutually unprotected — a compromised workload can kill its supervisor or interfere with the supervisor's listeners. The escalation (supervisor under its own SID, workload token derived) is deferred and invisible to the rest of the design.

> win[identity.file-permissions]
> The owner-only file permission rules of the portable spec (`r[infra.key.file-permissions]` and kin) are restated for NTFS: Seedling-managed data directories, volume roots, and secret files carry ACLs granting the owning instance SID, SYSTEM, and Administrators, and no other principal. Inheritance from parent directories must be broken on creation. Apps cannot modify WFP state (BFE mutation requires administrative rights), other instances' processes or volumes, or ingress configuration.

# Process Profiles

> win[profile.model]
> A **process profile** is the per-workload bundle of declarations the Windows runtime needs where the Linux runtime leans on container conventions:
>
> - the [stop method](#win--stop.methods), and for `named_event` profiles whether a reload event is supported;
> - whether the workload's bind is env-injected or configuration-managed ([win[net.bind-address]](#win--net.bind-address));
> - whether the workload must be spawned sharing a console and in its own process group for ctrl-event delivery (implied by the `ctrl_break`/`ctrl_c` stop methods).
>
> Profile properties describe the software, not the deployment — Node handles CTRL_BREAK, PostgreSQL wants its own shutdown protocol, wherever they run.

> win[profile.source]
> Profiles come from three sources, in increasing precedence:
>
> - **Built-in profiles** ship with the runtime for adopted native services that have no artifact ([win[postgres.native]](#win--postgres.native)).
> - **Artifact-declared profiles**: the artifact config blob carries the profile fields alongside the runconfig fields ([win[artifact.profile]](#win--artifact.profile)), following the precedent of `StopSignal` in the Docker image config.
> - **BSL overrides** adjust a profile per deployment, the same way `container.stop_signal` overrides an image's `StopSignal` on the Linux runtime. The concrete BSL surface is settled with the capability work (see the plan document).

# Networking

The Linux runtime's per-service IPv6 addressing survives on Windows as loopback aliasing: no driver, no userspace network stack.

> win[net.prefix]
> The runtime owns a ULA IPv6 prefix derived deterministically from the host's `MachineGuid` (RFC 4193-style hashing, mirroring the Linux runtime's machine-id derivation). All Seedling service addresses and instance-private addresses are allocated from this prefix. Addresses are added as aliases on the loopback interface with the `skipassource` flag set, so they are never selected as outbound source addresses. `[spike: v4 fallback stance]`

> win[net.listener]
> Each service address:port declared by an app's BSL is bound by the app's **supervisor**, which relays connections to the instance's private listener ([win[net.bind-address]](#win--net.bind-address)). Because the listen socket belongs to the supervisor, it survives workload crashes: inbound connections queue in the backlog during workload restart rather than being refused. Supervisor death drops the addresses, which correctly presents as "the pod is down".

> win[net.bind-address]
> The workload's real listeners are not on bare loopback: the runtime allocates each instance a private address inside the Seedling prefix and injects its listener assignments as the `BIND_ADDRESS` environment variable. `BIND_ADDRESS` is a comma-separated list of `IP:PORT` entries, IPv6 addresses in brackets (e.g. `[fdxx::a]:3000,[fdxx::a]:3001`), one entry per declared listener, in declaration order. When `BIND_ADDRESS` is present it supersedes `PORT`. The instance must bind exactly the entries it is handed, and nothing else. This places every workload listener inside the [default-deny](#win--wfp.default-deny), closing the bypass in which a plain-loopback port is dialable by any local process.
>
> Exception: workloads whose bind is configuration-managed rather than env-derived (PostgreSQL's `listen_addresses`) are flagged as such in their [process profile](#win--profile.model), and the runtime renders the address into their configuration instead.

> win[net.bind-verify]
> The supervisor must verify, as part of readiness, that every `BIND_ADDRESS` entry is held by a process inside the instance's Job Object. If the workload is listening elsewhere — notably on bare loopback, indicating `BIND_ADDRESS` was ignored — or an entry is unbound while the workload reports healthy, the supervisor files a fault naming the divergence. A workload that binds the wrong addresses must not be reported ready.

> win[net.mount]
> A mount (A may reach B) compiles to: A dials B's service address, which is B's supervisor's listener, which relays to B's private address. This is the role DNAT plays on Linux; A needs nothing installed on its side. The relay is a raw TCP byte relay: no PROXY protocol, no protocol awareness. Client identity for HTTP traffic is conveyed at layer 7 (ingress adds X-Forwarded-For). The relay must not impose idle timeouts; long-lived streams (WebSocket upgrades) are relayed until either side closes.

> win[net.resolver]
> The resolver concept of the portable spec survives intact: the [resolver special service](#win--special.resolver) serves DNS for the Seedling zone on a Seedling prefix address, and the runtime installs an NRPT rule scoping that zone (and only that zone) to the Seedling resolver. Global DNS resolution is untouched. NRPT rules are installed idempotently and removed on uninstall.

# Mount-Graph Enforcement (WFP)

> win[wfp.provider]
> All Seedling filters, the Seedling sublayer, and associated objects are installed as **persistent** WFP objects under a fixed, documented Seedling provider GUID. Persistence places enforcement in BFE, not in seedlingd: the allow/deny structure holds with the daemon down or crashed. Fixed GUIDs enable auditing installed filters against the mount graph (`seedling doctor`), idempotent replacement on upgrade, and a provider-scoped sweep on uninstall.

> win[wfp.default-deny]
> At the ALE connect layers, connections whose remote address falls within the Seedling prefix are blocked by default. Loopback traffic classifies through ALE like any other traffic on the platform floor.

> win[wfp.allows]
> The mount graph compiles to allow filters keyed on (user SID, remote address):
>
> - one allow per mount: A's instance SID may connect to B's service address;
> - one allow per instance: the instance's own supervisor SID may connect to the instance's private address (the relay hop);
> - one allow per ingress route: the ingress service SID may connect to the backing service address;
> - allows for every instance SID and special-service SID to the resolver address's DNS port;
> - bind/listen allows for each supervisor SID on its own addresses.
>
> SIDs are deterministic and addresses stable for the instance's life, so filters change when the mount graph changes or instances are created/removed — not on workload restarts. Dynamic-Job grants are operation-scoped per [win[identity.dynamic-jobs]](#win--identity.dynamic-jobs).

> win[wfp.honesty]
> The Windows threat-model document must state plainly: this is discretionary enforcement of the mount graph against non-privileged processes, not a sandbox. An administrative process can delete the filters. There is no per-connection authentication (parity with the Linux DNAT model, no regression). The host remains default-open to the external network; outbound-deny is out of scope for v1.

# Special Services

> win[special.model]
> A **special service** is a runtime-installed, SCM-registered native service with a built-in [process profile](#win--profile.model) and its own virtual account, outside the workload model: no artifact, no supervisor, no service-address relay. Special services bind their operational addresses directly rather than receiving `BIND_ADDRESS` injection.
>
> Special services are demand-start like supervisors ([win[identity.scm-entry]](#win--identity.scm-entry) is preserved: seedlingd remains the only auto-start entry) and are started by the daemon in dependency order — the resolver first, ingress after its backends. Their registrations are permanent runtime installations, exempt from the identity GC sweep ([win[identity.gc]](#win--identity.gc)). Adopted native services ([win[postgres.native]](#win--postgres.native)) share the mechanics but are adopted rather than installed.

> win[special.ingress]
> The ingress controller (Caddy) is a special service, replacing the Linux proxy infrastructure container rules (`r[infra.proxy.startup]` and kin). It binds the host's public ports directly and dials backing workloads on their service addresses like any mount edge. The daemon renders its configuration from desired state and applies changes via graceful config reload; route changes must not drop established connections. Ingress lifecycle semantics (`r[lifecycle.ingress]`) apply unchanged.

> win[special.resolver]
> The resolver is a special service (CoreDNS), replacing the Linux resolver infrastructure container (`r[infra.resolver]`). It binds the Seedling resolver address; the daemon renders its zone data and upstream forwarding configuration. NRPT scoping per [win[net.resolver]](#win--net.resolver) is unchanged.

# Shutdown and Signals

> win[stop.methods]
> Each [process profile](#win--profile.model) declares a stop method, one of:
>
> - `ctrl_break` / `ctrl_c`: the supervisor delivers `GenerateConsoleCtrlEvent` to the workload's process group. Requires the supervisor to share a console with the workload and to have spawned it with `CREATE_NEW_PROCESS_GROUP`, so the group-targeted event does not strike the supervisor. `[spike]`
> - `named_event`: the supervisor passes `SEEDLING_STOP_EVENT=<name>` in the environment and signals the named event to request shutdown. A `SEEDLING_RELOAD_EVENT` sibling may be declared for reload semantics.
> - `service_stop`: for SCM-managed sidecar workloads (native PostgreSQL), stop is delivered as the service control code (equivalently `pg_ctl stop` for Postgres profiles).
> - `none`: proceed directly to termination.

> win[stop.ladder]
> The stop sequence is: deliver the configured stop method → wait `stop_timeout` → `TerminateJobObject`. When the method is `none`, `stop_timeout` is ignored and a warning is logged, rather than silently waiting on a delivery that never happened. This ladder is the Windows restatement of the Linux `stop_signal`/`stop_timeout`/SIGKILL contract (`l[container.stop-signal]`, `l[container.stop-timeout]`).

> win[stop.host-shutdown]
> Host shutdown is delivered to supervisors as SCM stop/preshutdown; each supervisor runs its stop ladder within the SCM-granted window. Deployment documentation must state the `WaitToKillServiceTimeout` expectations, since the platform default stop window is shorter than a busy database wants. Workload data safety on host shutdown is bounded by this window; this is a stated property, not an emergent one.

> win[signal.map]
> `rt.signal(target, name)` retains POSIX signal names in BSL for portability. On the Windows runtime v1:
>
> - `SIGTERM`, `SIGINT`, `SIGQUIT`, `SIGKILL` map to **termination of the instance's Job Object without modifying desired state**. The reconciler's view is a process exit; whether the instance restarts follows from desired state, exactly as on Linux. This preserves the dominant script pattern (`rt.signal` → `.terminated_eventually()` → work → restart).
> - `SIGHUP` maps to signalling the target profile's declared reload event ([win[profile.model]](#win--profile.model)). On targets whose profile declares no reload event, it is skipped as below.
> - All other signals (`SIGUSR1`, …) are **skipped**: the action log records the delivery as `skipped (unsupported)` (reusing the at-most-once persistence of `r[rt.signal]`), and a warning event fires. A skipped signal must never be silently treated as delivered.
>
> Recorded caveat: Linux `SIGTERM` allows the target to flush and exit cleanly; `TerminateJobObject` does not. A script that depends on the workload's own shutdown work after a termination-intent signal is subtly wrong on Windows v1.

> win[signal.exit-codes]
> The runtime synthesizes the negative-exit-code convention: whenever the runtime itself terminated a process (stop ladder final rung, signal-mapped termination, session teardown), the recorded exit code is negative. `i[shell.exit]`'s convention thereby holds on both platforms: negative means "terminated by the runtime", non-negative is the process's own exit code. (`l[rt.executed.exit-code]` currently specifies host-convention values above 255 for signal-terminated commands; the spec restructuring aligns it with the same negative-code convention — see the plan document.)

# Capabilities

> win[capability.map]
> The runtime exposes a capability map with a shared vocabulary across three surfaces: BSL (`rt.capability(name) -> bool`), the operator interface (`/status` capabilities field), and `seedling doctor`. Capabilities are per-node, fixed at script evaluation, and stable across replay, composing with the deterministic-replay rules and the discover probe.
>
> Initial vocabulary (Windows v1 values in parentheses): `signal:terminate` (true), `snapshots` (false), `storage:block-clone` (true iff volume root is ReFS), `net:nat64` (false). Linux v1 values are all-true except `storage:block-clone` where applicable. Reload support is not a node capability: it is a property of the target's [process profile](#win--profile.model) ([win[signal.map]](#win--signal.map)). Scripts must branch on capabilities, not on OS identity.

# Action Execution Context

> win[action.exec]
> The Linux rule "`Executed` runs inside the target's running container" is restated: the command is spawned **inside the target instance's Job Object**, as the instance's virtual account, with the instance's rendered environment and declared working directory. Same-Job membership reproduces the container property that stopping the instance kills in-flight commands. The target-must-be-running precondition is retained for conformance parity.

> win[action.env-hygiene]
> The spawned command's environment is constructed from a minimal Win32 base (`SystemRoot` and peers required for process start) plus the artifact's declared `Env`, including its `PATH` entries [rebased onto the mount](#win--artifact.rebase). The daemon's environment, the host `PATH`, and machine-installed toolchains are never inherited. Commands resolve against the artifact's own runtime, not against whatever is installed on the host.

> win[action.volume-params]
> The operation-scoped volume binding machinery (`r[operation.volume-param]` and companions) ports unchanged as a name→path binding table; `app.external_volume()` resolves to an absolute, long-path-safe Windows path. *Note:* the backup rework may remove this machinery's only consumers across all runtimes; confirm before implementing (see plan document).

# Shell Sessions

> win[shell.conpty]
> A shell Job runs under a **ConPTY** pseudoconsole owned by its (dynamic-Job) supervisor. Stream mapping: operator input to the ConPTY input; ConPTY output to the stdout stream. ConPTY merges output; the stderr unidirectional stream of `i[stream.shell]` may carry nothing on Windows, and clients must not block on it. Resize requests map to `ResizePseudoConsole`. Exit codes follow [win[signal.exit-codes]](#win--signal.exit-codes). `[spike]`

> win[shell.volume]
> Volume shells replace the Linux ephemeral-container-with-`/mnt/{name}` contract: the runtime creates a per-session temporary directory of junctions, one per selected volume, named by display name, and launches the shell with that directory as its working directory.
>
> `read_only` is enforced by identity, not mount flags: the session runs under a per-session principal whose ACEs on the selected volume roots grant read-only (or modify, for writable sessions) access. This replaces `i[volumes.shell.caps]`'s Linux-capability rules with enforcement that binds every tool run inside the session.

# Artifacts

Windows workloads ship as read-only NTFS VHDX images inside OCI artifacts (ORAS-style, BES media types), activated by attachment rather than extraction. The Tamanu `vhdx-pack` pipeline is the reference producer; this profile is normative for what the runtime consumes.

> win[artifact.profile]
> A Windows artifact is an OCI manifest whose config blob has media type `application/vnd.au.bes.seedling.windows-vhdx.config.v1+json` and whose single layer has media type `application/vnd.au.bes.seedling.windows-vhdx.v1+zstd`. The config blob mirrors the Docker image config (`WorkingDir`, `Env`, `Entrypoint`, `Cmd`, `ExposedPorts`) plus `vhdx.rootDir`, the top-level directory inside the volume, and the [process profile](#win--profile.source) fields. Within a multi-platform index the manifest carries platform `unknown/unknown`; the runtime selects by config media type, never by platform fields or annotations. Only Windows on x64 is supported, so the config media type identifies the artifact unambiguously.

> win[artifact.verify]
> The registry digest covers the compressed layer; the producer additionally annotates the uncompressed VHDX's SHA-256. The runtime verifies the compressed digest at pull, verifies the uncompressed digest after decompression into the store, and re-verifies it **before every attach**. A pre-attach mismatch quarantines the store entry, files a fault, and triggers a re-pull; attach never proceeds on a mismatched image, since attaching hands the blob to the kernel filesystem parser.

> win[artifact.attach]
> Activation is: verify → attach read-only (`AttachVirtualDisk` with the read-only flag, folder mount point, no drive letter) → resolve entrypoint from the config → spawn under the Job Object. Read-only attach is unconditional; it neutralizes NTFS's mount-time write urges (dirty bit, `$LogFile` replay) and makes image immutability a mechanical property. Writable state lives in volumes.

> win[artifact.rebase]
> Config paths use forward slashes relative to the volume root. The runtime rebases `WorkingDir` and each `PATH` entry onto the mount point beneath `vhdx.rootDir` and normalizes separators before process creation.

> win[artifact.store]
> The image store is a directory of digest-named VHDX files plus manifest/config metadata. The `/images/*` operator surface (list, pull with backoff, pin, remove, discover) applies with unchanged semantics. GC of an unpinned, unreferenced image is detach-if-attached then delete. Store and volume roots should carry the documented Defender real-time-scanning exclusion (on-demand scanning retained), recorded as a deployment requirement.

# Storage, Volumes, Backups

> win[volume.model]
> A volume is a runtime-owned directory under the volume root, ACL'd per [win[identity.file-permissions]](#win--identity.file-permissions). NTFS is the supported floor; ReFS is detected and surfaces as `storage:block-clone` for future use. Volume snapshots are not implemented in v1; snapshot-dependent portable-spec rules are capability-gated off.

> win[backup.v1]
> Backups are performed by a single embedded method (kopia integration; cross-runtime rework, specified separately), not by the container-based backup-app strategies of the current Linux runtime. PostgreSQL is backed up via `pg_basebackup`/dump-based methods, never by filesystem copy of a live cluster. The embedded backup engine's read access to volumes is granted by explicit ACE if seedlingd runs under its own principal, or rides the SYSTEM ACE if it runs as LocalSystem (open question Q3 in the plan document).

# PostgreSQL

> win[postgres.native]
> PostgreSQL runs as a native SCM-managed service that the runtime manages rather than owns: its built-in [process profile](#win--profile.source) flags `config-managed bind` ([win[net.bind-address]](#win--net.bind-address)) and `service_stop` ([win[stop.methods]](#win--stop.methods)); the daemon renders `listen_addresses` and orders it first in cold-start dependency order. Migration of existing installations is covered in the plan document.
