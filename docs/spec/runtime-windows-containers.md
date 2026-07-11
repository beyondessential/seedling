The Seedling Windows Container Runtime is a second implementation of the Seedling runtime, targeting Windows Server hosts by running each workload in a **process-isolated Windows container** composed just-in-time from a Microsoft OS base image and the workload's VHDX artifact. It conforms to the operator interface spec and the language spec, and to the portable portions of the runtime spec (reconciliation, generations, lifecycle operations, barriers, history, faults, scheduling), while replacing the Linux infrastructure sections (Podman, systemd units, nftables, NAT64, volume snapshots) with the mechanisms defined here.

> **Draft status.** This document is an alternative to the process-native Windows runtime draft (`runtime-windows.md`, `win[...]` namespace), for the design sessions to weigh against it. Where the process-native draft reconstructs isolation, addressing, and mount-graph enforcement out of host primitives (Job Objects, per-instance SCM services and virtual-account SIDs, loopback address aliasing, WFP filters, and a supervisor byte-relay), this design obtains them from the container boundary the platform already provides. Rules marked `[spike]` depend on prototype confirmation; see the companion plan document. Rule IDs use the `wc[...]` namespace, tracked as the `runtime-windows-containers` spec in tracey.

Where this document is silent, the portable runtime spec applies unchanged. Where a Linux runtime rule (`r[...]`) is cited as *replaced*, the replacement here is normative for this runtime and the cited rule does not apply.

# Scope and Platform

> wc[platform.floor]
> The runtime supports Windows Server 2019 and later, x64 only, with the Containers feature and the Host Compute Service (HCS) and Host Networking Service (HNS) present. All mechanisms specified here (process-isolated container creation via HCS, per-container network compartments via HNS, just-in-time layer composition, VHDX attach APIs) must be available on the platform floor; a mechanism requiring a later version must be gated behind a [capability](#wc--capability.map).

> wc[platform.isolation]
> Workloads run under **process isolation**, not Hyper-V isolation: each container shares the host kernel and carries no utility VM. Hyper-V isolation — which would lift the [base-image build-match constraint](#wc--base.match) at the cost of a per-container VM — is out of scope for v1 and recorded as the version-drift escape hatch in the rationale document.

> wc[platform.non-goals]
> The following are explicitly out of scope for v1: workload survival across host reboot ([wc[boot.cold]](#wc--boot.cold)), volume snapshots and snapshot-based backup strategies, Hyper-V-isolated and Linux (LCOW) containers, horizontal scaling (replica count is fixed at 1 per Deployment), blue/green replacement of special services ([wc[special.upgrade]](#wc--special.upgrade)), and outbound-deny network policy.

# Boot and Reboot Semantics

> wc[boot.cold]
> Workloads do not survive host reboot. Workloads must survive the seedlingd daemon being stopped, crashed, or upgraded within a boot.
>
> After host reboot, the daemon performs a cold reconciliation pass: it observes an empty world and starts all resources itself, in dependency order derived from the mount graph (backends before their dependents, ingress after backends). Boot is not a special case; it is reconciliation from an empty world.

> wc[boot.replay]
> Interrupted-operation replay at boot (per `r[operation.lifecycle.events]`) composes with cold start: the replayed operation executes against a world in which no instance from the previous boot is running. Barrier replay rules must tolerate this — a replayed `rt.start()` finds no compute system to adopt and starts fresh; a replayed observation of a terminated resource is satisfied vacuously. The action log's at-most-once guarantees (e.g. `r[rt.signal]`) apply unchanged.

# Base Images and Just-in-Time Composition

The runtime does not ship base images and does not build or ship per-workload container images. It pulls the appropriate Microsoft OS base once per host, and composes each workload's container image at run time by layering the workload's artifact over that base. The only image publisher the runtime trusts is Microsoft, for the OS base the host is already running.

> wc[base.source]
> OS base images are pulled from a configured upstream OCI registry (default: the Microsoft Artifact Registry, `mcr.microsoft.com`) or an operator-configured mirror. The runtime never authors an OS base; it consumes Microsoft's. Which base *family* a workload requires (a minimal base for self-contained runtimes, a fuller base for software needing broader Win32 surface) is declared by the artifact ([wc[artifact.profile]](#wc--artifact.profile)), not chosen by the runtime — the runtime's job is to obtain the matching build of the family the artifact names.

> wc[base.match]
> Process isolation shares the host kernel, so a base image is usable only if its OS build number matches the host's. The runtime selects, per host, the base whose build matches the running host build. A workload whose required base family has no build-matching image available must not be started: the runtime files a fault naming the missing base and the host build, rather than starting a mismatched container (which the platform would refuse or, worse, run with undefined behaviour). This makes host-build drift a first-class operational condition surfaced through [`seedling doctor`](#wc--capability.map).

> wc[base.pull]
> Base images are pulled ahead of need and cached in the [image store](#wc--artifact.store), keyed by family and build. A base pull happens once per (family, build) per host: on first use, and again after a host update bumps the build number. `seedling doctor` reports, per host, which base families are present for the current build, so that a host patched without network egress to the registry is detected before its workloads fail to start. Operators may pre-stage bases into the store out of band; the store is the single source the composer draws from.

> wc[compose.jit]
> A workload's runnable image is composed just-in-time from two read-only layers — the selected [OS base](#wc--base.match) and the workload's [app layer](#wc--artifact.compose) — with a writable scratch layer on top. No per-workload image is pre-built, shipped, or pushed to any registry; composition is a local operation over the store. The composed image is cached keyed on (base build, app-layer digest) so repeat starts of the same generation skip re-composition. A change to either input produces a distinct cache key.

# Deploys and Replacement

> wc[deploy.replace]
> Workload deploys and health-driven replacement retain the portable zero-downtime semantics (`r[update.rolling]`, `r[autonomous.healthcheck-replace]`), mediated by the [network fabric](#wc--net.fabric): the replacement generation is composed and started in its own compute system with its own [fabric endpoint](#wc--net.compartment) alongside the old one; when the replacement reports ready, the daemon repoints the old generation's [service address](#wc--net.service-address) to the new endpoint and drains the old. Connections in flight to the old generation drain until closed; new connections resolve to the new generation. No Seedling process sits between client and workload during the switch — the switch is a fabric routing change, not a relay retarget.
>
> Instance identity (the service name and its stable fabric address) persists across deploys: a deploy changes which compute system backs the address, not the address.

# Compute Model

Each running instance is one process-isolated container — an HCS **compute system** — owned by a small per-instance supervisor process (working name **seedpod**), analogous to conmon in the podman stack and to `containerd-shim-runhcs` in the containerd stack. The supervisor exists so that neither the daemon's lifetime nor its address space is coupled to the workload's. Unlike the process-native design's supervisor, this one carries no data plane: networking and isolation are the compute system's, so the supervisor holds no listeners and relays no traffic.

> wc[instance.compute-system]
> Each running instance is exactly one HCS compute system enclosing the workload's process tree, its [network compartment](#wc--net.compartment), its mapped volumes, and its scratch layer. Terminating the compute system terminates the whole tree; this is the container analogue of the Linux cgroup/namespace teardown and replaces the Job Object of the process-native design. A deploy transiently owns two compute systems for one instance (old and new generation) per [wc[deploy.replace]](#wc--deploy.replace); steady state is one.

> wc[pod.ownership]
> Each compute system is owned by exactly one supervisor process. The supervisor owns:
>
> - creating the compute system from the [composed image](#wc--compose.jit) and starting the workload process, restarting it per policy on exit without daemon involvement;
> - stdout/stderr capture to the log store, and exit-status recording;
> - execution of the [stop sequence](#wc--stop.ladder).
>
> The compute system's endpoint on the [fabric](#wc--net.fabric) carries inbound traffic directly to the workload; the supervisor is not in that path. The daemon is pure control plane: it reconciles desired state, creates, commands, and removes supervisors, and sits in no data path and no process-ownership chain.

> wc[pod.breakaway]
> Supervisors are spawned so that their lifetime is not coupled to the daemon's: outside any job or service-control grouping that terminates with the daemon, and with no inheritable daemon handles. If the platform environment would couple supervisor lifetime to the daemon's, the runtime must fail supervisor creation with a diagnostic naming the restriction rather than silently nesting, since coupled lifetime violates [wc[boot.cold]](#wc--boot.cold)'s within-boot survival property. Compute systems created by a supervisor outlive the supervisor's creator handle: HCS retains the compute system, so a supervisor restart re-opens it rather than recreating it.

> wc[pod.record]
> For each supervisor, the daemon persists an on-disk record containing at minimum: instance identity, supervisor PID, supervisor process start time, compute system ID, pipe name, assigned fabric endpoint and addresses, and the instance's generation. PID + start time together disambiguate PID reuse: on reattachment, a record whose (PID, start time) does not match a live process describes a dead world and must be garbage-collected, not adopted.

> wc[pod.reattach]
> Each supervisor serves a named pipe (`\\.\pipe\seedling-<instance-id>`) carrying status reports, exit events that occurred while the daemon was down, and daemon commands. On startup and after restart, the daemon enumerates supervisor records, reattaches to each live supervisor's pipe, re-opens a waitable handle on the supervisor process, re-opens the compute system by ID to confirm liveness, and reconciles any missed events into the observation history.

> wc[pod.pipe-trust]
> The pipe namespace is first-come-first-served, so neither end trusts it blindly. The supervisor creates the pipe with an ACL restricting connection to the daemon's service SID, SYSTEM, and Administrators. The daemon verifies on every connect that the pipe server process is the recorded supervisor — matching PID and process start time against the [supervisor record](#wc--pod.record) — before trusting anything read from the pipe. A pipe whose server fails verification is not adopted: the daemon files a fault and leaves the record to the dead-world GC rules.

> wc[pod.pipe-protocol]
> The pipe protocol must be version-skew-tolerant: a newer daemon must interoperate with older supervisors, since upgrading a supervisor requires draining its workload. The protocol opens with a hello frame carrying a protocol version and feature bits; unknown frame types are skipped, not fatal. The protocol should be pinned early and change rarely. `[spike]`

> wc[pod.crash]
> The daemon detects supervisor death by waiting on the supervisor process handle. Because the compute system outlives its creator, supervisor death does not by itself kill the workload; the daemon re-adopts the orphaned compute system through a replacement supervisor on the next reconciliation pass, restoring the exit-policy and stop-sequence owner. A compute system whose supervisor is dead and which matches no live record is reaped by the [GC pass](#wc--identity.gc). The supervisor must remain small: it carries no data plane, so its failure surface is confined to exit-policy and log capture, not to workload reachability.

# Identity

Because isolation is the container boundary, this runtime does not mint a host security principal per instance. The elaborate per-instance and per-invocation SCM-service-plus-virtual-account machinery of the process-native design — and the service-install audit churn it generates — does not exist here.

> wc[identity.daemon-entry]
> Exactly one Seedling component is registered as an auto-start SCM service: seedlingd itself. It runs under its own virtual service account (`NT SERVICE\seedlingd`), not LocalSystem; access it needs beyond that account's default slice is granted by explicit ACE, chiefly its SID on the standard volume ACLs ([wc[identity.file-permissions]](#wc--identity.file-permissions)). No supervisor or workload is a boot-started or demand-start SCM service ([wc[boot.cold]](#wc--boot.cold)); supervisors are ordinary child processes ([wc[pod.breakaway]](#wc--pod.breakaway)).

> wc[identity.container-principal]
> A workload runs under a container-local, non-administrative account inside its compute system, not under a host account. Cross-instance isolation is the compute-system and [compartment](#wc--net.compartment) boundary — one instance cannot see another's processes, its private filesystem, or its network namespace — not a host SID plus discretionary filtering. No per-instance host service registration, virtual account, or deterministic host SID is created; there is therefore no per-instance or per-action service-install audit event, no marked-for-delete service residue, and no account-review surface.

> wc[identity.file-permissions]
> A volume is a host directory [mapped into](#wc--volume.model) the instance's compute system. Host-side, Seedling-managed data directories, volume roots, and secret files carry ACLs granting the daemon's service SID ([wc[identity.daemon-entry]](#wc--identity.daemon-entry)), SYSTEM, and Administrators, and no other principal; inheritance from parent directories is broken on creation. Inside the container, the workload reaches only the volumes mapped into its own compute system, and only with the access the mapping grants ([read-only or read-write](#wc--volume.model)). An instance cannot reach another instance's volumes because they are not mapped into its compartment, not because an ACL forbids it.

> wc[identity.non-admin]
> Workload processes never run elevated: the container account is non-administrative and cannot modify host firewall/fabric state, other instances' compute systems or volumes, or ingress configuration. The compute system further denies the workload any host-level view. Escalation to distinct per-workload host principals is unnecessary in this model and is not attempted.

> wc[identity.gc]
> The reattachment/GC pass sweeps container artifacts as well as processes: compute systems, fabric endpoints and their firewall policies, composed image layers, scratch layers, and volume mappings that match no live instance record or in-progress operation are removed. This extends the stuck-state recovery principle of `r[operation.cancel.stuck-recovery]` to container residue. Base images in the store are exempt (they are shared infrastructure, reaped only by store GC per [wc[artifact.store]](#wc--artifact.store)).

# Process Profiles

> wc[profile.model]
> A **process profile** is the per-workload bundle of declarations the runtime needs where the Linux runtime leans on container conventions:
>
> - the [stop method](#wc--stop.methods), and whether a reload signal is supported;
> - whether the workload's bind is env-injected or configuration-managed ([wc[net.compartment]](#wc--net.compartment));
> - whether the workload must be spawned in its own process group for ctrl-event delivery (implied by the `ctrl_break`/`ctrl_c` stop methods).
>
> Profile properties describe the software, not the deployment. The runtime special-cases no particular application: a database, a web server, and a batch job are all workloads distinguished only by their declared profile.

> wc[profile.source]
> Profiles come from two sources, in increasing precedence:
>
> - **Artifact-declared profiles**: the artifact config blob carries the profile fields alongside the runconfig fields ([wc[artifact.profile]](#wc--artifact.profile)), following the precedent of `StopSignal` in the Docker image config.
> - **BSL overrides** adjust a profile per deployment, the same way `container.stop_signal` overrides an image's `StopSignal` on the Linux runtime. The concrete BSL surface is settled with the capability work (see the plan document).
>
> There are no built-in profiles for host-native services: every workload has an artifact.

# Networking

Each instance runs in its own network compartment — a real Windows network namespace — joined to a single Seedling fabric. There is no loopback address aliasing, no `skipassource` bookkeeping, and no per-instance host address plumbing; the app binds ordinary ports inside its own namespace.

> wc[net.fabric]
> The runtime maintains one Seedling HNS network (the **fabric**) that all instance and special-service endpoints join. Its address prefix is derived deterministically from the host's `MachineGuid` (RFC 4193-style hashing, mirroring the Linux runtime's machine-id derivation), so it is stable across daemon restarts and collides with nothing routable. The fabric is created idempotently and removed on uninstall.

> wc[net.compartment]
> Each instance's compute system has its own network compartment with exactly one endpoint on the [fabric](#wc--net.fabric), assigned a stable address from the fabric prefix for the instance's life. The workload binds its declared ports inside its own compartment; the runtime injects the assignment as the `BIND_ADDRESS` environment variable — a comma-separated list of `IP:PORT` entries, IPv6 addresses in brackets, one entry per declared listener in declaration order — superseding `PORT` when present, exactly as landed in Tamanu. Because the compartment is isolated, a workload that ignores `BIND_ADDRESS` and binds bare loopback is reachable only from within its own compartment: the divergence is a correctness fault the supervisor may report, not a security bypass ([wc[fw.default-deny]](#wc--fw.default-deny) already contains it).
>
> Exception: workloads whose bind is configuration-managed rather than env-derived are flagged as such in their [process profile](#wc--profile.model), and the runtime renders the address into their configuration instead.

> wc[net.service-address]
> Each instance has a stable **service address** on the fabric — its endpoint address — under which other instances reach it. Unlike the process-native design, the service address is the workload's own endpoint, not a supervisor-held listener that relays inward: there is no relay hop and no separate private address. The [resolver](#wc--net.resolver) maps the instance's service name to this address.

> wc[net.mount]
> A mount (A may reach B) compiles to a [fabric firewall allow](#wc--fw.allows): A's endpoint may open connections to B's service address and declared port. A dials B directly across the fabric; nothing is installed on A's side and no Seedling process is traversed. This is the direct-dial model the process-native design records as a future escape hatch; here it is the default, because the compartment boundary makes it safe. The path is protocol-agnostic; client identity for HTTP is conveyed at layer 7 (ingress adds X-Forwarded-For). No idle timeout is imposed; long-lived streams (WebSocket upgrades) are relayed by the fabric until either side closes.

> wc[net.resolver]
> The resolver concept of the portable spec survives intact: the [resolver special service](#wc--special.resolver) serves DNS for the Seedling zone on a fabric address, and the fabric's per-compartment DNS configuration points instances at it. Global DNS resolution on the host is untouched; compartments carry their own resolver configuration, so no host-wide resolution-policy rule is installed. Resolver configuration is applied idempotently and removed on uninstall.

# Mount-Graph Enforcement

> wc[fw.provider]
> All Seedling fabric firewall policies are installed as endpoint-scoped policies under a fixed, documented Seedling identity, and are reconstructed by the daemon from desired state on start. Enforcement lives at the compartment boundary in the platform network stack, not in seedlingd: the allow/deny structure holds with the daemon down or crashed. The fixed identity enables auditing installed policy against the mount graph (`seedling doctor`), idempotent replacement on upgrade, and a scoped sweep on uninstall.

> wc[fw.default-deny]
> Traffic between fabric endpoints is default-deny at the compartment boundary: an endpoint may reach only the peers its compiled [allows](#wc--fw.allows) name. Because the policy is attached to the endpoint from outside the compartment, a non-privileged workload cannot remove or weaken its own restrictions — the enforcement is mandatory against workloads, not discretionary. An address that has not been allocated to an instance cannot be squatted, because endpoints and their addresses are minted by the runtime, not by workloads.

> wc[fw.allows]
> The mount graph compiles to allow policies keyed on (source endpoint, destination address:port):
>
> - one allow per mount: A's endpoint may connect to B's service address and declared port;
> - one allow per ingress route: the ingress endpoint may connect to the backing service address;
> - allows for every instance and special-service endpoint to the resolver address's DNS port.
>
> Endpoints and their addresses are stable for the instance's life, so policies change when the mount graph changes or instances are created/removed — not on workload restarts.

> wc[fw.honesty]
> The Windows threat-model document must state plainly what this does and does not guarantee. It **is** a real network-namespace boundary between workloads: cross-instance reachability, process visibility, and filesystem visibility are enforced by the platform, mandatorily, against non-privileged workloads — a stronger position than the process-native design's discretionary host filters. It is **not** a defence against a host administrator, who can reconfigure the fabric or the compute systems. There is no per-connection authentication (parity with the Linux DNAT model, no regression). The host remains default-open to the external network; outbound-deny is out of scope for v1.

# Special Services

> wc[special.model]
> A **special service** is a runtime-managed service outside the workload model: the [ingress controller](#wc--special.ingress) and the [resolver](#wc--special.resolver). Each runs in its own compute system and joins the fabric like any endpoint, but binds its operational addresses directly rather than receiving `BIND_ADDRESS` injection, and is not deployed from a user artifact. Special services are started by the daemon in dependency order — the resolver first, ingress after its backends. Their compute systems and endpoints are permanent runtime installations, exempt from the identity GC sweep ([wc[identity.gc]](#wc--identity.gc)).

> wc[special.ingress]
> The ingress controller (Caddy) is a special service, replacing the Linux proxy infrastructure container rules (`r[infra.proxy.startup]` and kin). Its compute system binds the host's public ports through a fabric port-mapping (host `:443`/`:80` forwarded to the ingress endpoint), and it dials backing workloads on their service addresses like any mount edge. The port mapping is fabric state independent of the daemon, so the daemon can restart without dropping public traffic. The daemon renders the ingress configuration from desired state and applies changes via graceful config reload; route changes must not drop established connections. Ingress lifecycle semantics (`r[lifecycle.ingress]`) apply unchanged.

> wc[special.resolver]
> The resolver is a special service (CoreDNS), replacing the Linux resolver infrastructure container (`r[infra.resolver]`). Its compute system binds the Seedling resolver address; the daemon renders its zone data and upstream forwarding configuration. Per-compartment DNS scoping per [wc[net.resolver]](#wc--net.resolver) is unchanged.

> wc[special.upgrade]
> Special services hold their own binds precisely so that no Seedling process sits in their data path: the daemon and every supervisor can stop or restart without dropping public traffic or DNS. The recorded trade is that special services are not blue/green replaceable — a special-service binary upgrade is stop-then-start of its compute system with a brief outage. Configuration changes do not incur this: they apply via graceful reload ([wc[special.ingress]](#wc--special.ingress)). A zero-downtime ingress upgrade would require a Seedling-held front bind relaying to the controller, reintroducing the data-path coupling this design exists to avoid; it is explicitly rejected for v1.

# Shutdown and Signals

> wc[stop.methods]
> Each [process profile](#wc--profile.model) declares a stop method, one of:
>
> - `ctrl_break` / `ctrl_c`: the supervisor delivers `GenerateConsoleCtrlEvent` to the workload's process group inside the compute system. Requires the workload to have been spawned in its own process group. `[spike]`
> - `named_event`: the supervisor passes `SEEDLING_STOP_EVENT=<name>` in the environment and signals the named event to request shutdown. A `SEEDLING_RELOAD_EVENT` sibling may be declared for reload semantics.
> - `none`: proceed directly to termination.

> wc[stop.ladder]
> The stop sequence is: deliver the configured stop method → wait `stop_timeout` → terminate the compute system. When the method is `none`, `stop_timeout` is ignored and a warning is logged, rather than silently waiting on a delivery that never happened. This ladder is the restatement of the Linux `stop_signal`/`stop_timeout`/SIGKILL contract (`l[container.stop-signal]`, `l[container.stop-timeout]`); the final rung terminates the whole compute system, so no workload subprocess survives.

> wc[stop.host-shutdown]
> Host shutdown is delivered to seedlingd as SCM stop/preshutdown; the daemon runs each instance's stop ladder within the SCM-granted window, in reverse dependency order. Deployment documentation must state the `WaitToKillServiceTimeout` expectations, since the platform default stop window is shorter than a busy stateful workload wants. Workload data safety on host shutdown is bounded by this window; this is a stated property, not an emergent one.

> wc[signal.map]
> `rt.signal(target, name)` retains POSIX signal names in BSL for portability. On this runtime v1:
>
> - `SIGTERM`, `SIGINT`, `SIGQUIT`, `SIGKILL` map to **termination of the instance's compute system without modifying desired state**. The reconciler's view is a process exit; whether the instance restarts follows from desired state, exactly as on Linux. This preserves the dominant script pattern (`rt.signal` → `.terminated_eventually()` → work → restart).
> - `SIGHUP` maps to signalling the target profile's declared reload event ([wc[profile.model]](#wc--profile.model)). On targets whose profile declares no reload event, it is skipped as below.
> - All other signals (`SIGUSR1`, …) are **skipped**: the action log records the delivery as `skipped (unsupported)` (reusing the at-most-once persistence of `r[rt.signal]`), and a warning event fires. A skipped signal must never be silently treated as delivered.
>
> Recorded caveat: Linux `SIGTERM` allows the target to flush and exit cleanly; compute-system termination does not. A script that depends on the workload's own shutdown work after a termination-intent signal is subtly wrong on this runtime v1; the [stop ladder](#wc--stop.ladder) is the path that runs the workload's shutdown.

> wc[signal.exit-codes]
> The runtime synthesizes the negative-exit-code convention: whenever the runtime itself terminated a process (stop ladder final rung, signal-mapped termination, session teardown), the recorded exit code is negative. `i[shell.exit]`'s convention thereby holds on both platforms: negative means "terminated by the runtime", non-negative is the process's own exit code. (`l[rt.executed.exit-code]` currently specifies host-convention values above 255 for signal-terminated commands; the spec restructuring aligns it with the same negative-code convention — see the plan document.)

# Capabilities

> wc[capability.map]
> The runtime exposes a capability map with a shared vocabulary across three surfaces: BSL (`rt.capability(name) -> bool`), the operator interface (`/status` capabilities field), and `seedling doctor`. Capabilities are per-node, fixed at script evaluation, and stable across replay, composing with the deterministic-replay rules and the discover probe.
>
> Initial vocabulary (this runtime's v1 values in parentheses): `isolation:namespace` (true), `net:compartment` (true), `signal:terminate` (true), `snapshots` (false), `storage:block-clone` (true iff volume root is ReFS), `net:nat64` (false). Linux v1 values differ where the mechanism differs (`isolation:namespace` and `net:compartment` describe the container boundary; the Linux runtime reports its own equivalents). Reload support is not a node capability: it is a property of the target's [process profile](#wc--profile.model). Scripts must branch on capabilities, not on OS identity.

# Action Execution Context

> wc[action.exec]
> The Linux rule "`Executed` runs inside the target's running container" is restated directly: the command is spawned **as a new process inside the target instance's compute system**, under the same container account the workload runs under ([wc[identity.container-principal]](#wc--identity.container-principal)), in the same compartment, with the instance's rendered environment and declared working directory. Compute-system membership reproduces the container property that stopping the instance kills in-flight commands, and needs no per-invocation identity or firewall provisioning. The target-must-be-running precondition is retained for conformance parity.

> wc[action.env-hygiene]
> The spawned command's environment is constructed from a minimal base (the variables required for process start on the container base), plus the artifact's declared `Env` with its `PATH` entries [rebased onto the composed image](#wc--artifact.rebase), plus the runtime-rendered instance environment (`BIND_ADDRESS` and kin) — the same environment the workload itself sees. The daemon's environment, the host `PATH`, and machine-installed toolchains are never inherited. Commands resolve against the artifact's own runtime inside the container, not against whatever is installed on the host.

> wc[action.volume-params]
> The operation-scoped volume binding machinery (`r[operation.volume-param]` and companions) ports unchanged as a name→path binding table, resolving to absolute, long-path-safe paths as mapped inside the compute system. *Note:* the backup rework may remove this machinery's only consumers across all runtimes; confirm before implementing (see plan document).

# Shell Sessions

> wc[shell.conpty]
> A shell session runs as a process launched inside the target instance's (or a volume session's) compute system under a **ConPTY** pseudoconsole owned by its supervisor. Stream mapping: operator input to the ConPTY input; ConPTY output to the stdout stream. ConPTY merges output; the stderr unidirectional stream of `i[stream.shell]` may carry nothing on Windows, and clients must not block on it. Resize requests map to `ResizePseudoConsole`. Exit codes follow [wc[signal.exit-codes]](#wc--signal.exit-codes). `[spike]`

> wc[shell.volume]
> Volume shells replace the Linux ephemeral-container-with-`/mnt/{name}` contract with a purpose-composed shell container: the runtime creates a compute system from the [base image](#wc--base.match) with the selected volumes mapped in, one per selected volume named by display name, and launches the shell with that directory as its working directory. `read_only` is enforced by the volume mapping mode (read-only or read-write), not by per-session host principals: the mapping binds every tool run inside the session, replacing `i[volumes.shell.caps]`'s Linux-capability rules.

# Artifacts

Workloads ship as read-only NTFS VHDX images inside OCI artifacts (ORAS-style, BES media types), the same producer output the process-native runtime consumes. This runtime differs only in activation: rather than attaching the VHDX and running the process directly on the host, it materialises the VHDX into a read-only container layer and composes it over an OS base ([wc[compose.jit]](#wc--compose.jit)).

> wc[artifact.profile]
> A Windows artifact is an OCI manifest whose config blob has media type `application/vnd.au.bes.seedling.windows-vhdx.config.v1+json` and whose single layer has media type `application/vnd.au.bes.seedling.windows-vhdx.v1+zstd` — identical to the process-native runtime, so one producer serves both. The config blob mirrors the Docker image config (`WorkingDir`, `Env`, `Entrypoint`, `Cmd`, `ExposedPorts`) plus `vhdx.rootDir`, the top-level directory inside the volume, and the [process profile](#wc--profile.source) fields. This runtime additionally reads an optional `base` field naming the required OS base family and minimum version floor ([wc[base.source]](#wc--base.source)); absent, a default fuller base family is assumed. Within a multi-platform index the manifest carries platform `unknown/unknown`; the runtime selects by config media type. Only Windows on x64 is supported, so the config media type identifies the artifact unambiguously.

> wc[artifact.verify]
> The registry digest covers the compressed layer; the producer additionally annotates the uncompressed VHDX's SHA-256. The runtime verifies the compressed digest at pull, verifies the uncompressed digest after decompression into the store, and re-verifies it **before composing** the app layer. A mismatch quarantines the store entry, files a fault, and triggers a re-pull; composition never proceeds on a mismatched image, since it hands the blob to the kernel filesystem parser.

> wc[artifact.compose]
> Activation is: verify → attach the VHDX read-only and materialise its `vhdx.rootDir` subtree into a read-only container layer → compose [selected OS base, app layer] into a runnable image ([wc[compose.jit]](#wc--compose.jit)) → create the compute system with the instance's volumes mapped and its [fabric endpoint](#wc--net.compartment) attached → resolve the entrypoint from the config → start. The base and app layers are read-only; all writes land in the discardable scratch layer, so image immutability is a mechanical property and writable state lives only in volumes. The materialised app layer is cached by app-layer digest so repeat activations skip re-materialisation.

> wc[artifact.rebase]
> Config paths use forward slashes relative to the volume root. The runtime rebases `WorkingDir` and each `PATH` entry onto the app layer's location in the composed image and normalizes separators before process creation.

> wc[artifact.store]
> The image store is a directory of digest-named VHDX files and materialised layers plus manifest/config metadata, alongside the cached OS [base images](#wc--base.pull). The `/images/*` operator surface (list, pull with backoff, pin, remove, discover) applies with unchanged semantics and additionally lists cached base images. GC of an unpinned, unreferenced image is: discard its composed and materialised layers, then delete. A base image is removed only when no cached app composition references its build and it is unpinned. Store and volume roots should carry the documented Defender real-time-scanning exclusion (on-demand scanning retained), recorded as a deployment requirement.

# Storage, Volumes, Backups

> wc[volume.model]
> A volume is a runtime-owned host directory under the volume root, ACL'd per [wc[identity.file-permissions]](#wc--identity.file-permissions) and mapped into the consuming instance's compute system at a rendered mount path. NTFS is the supported floor; ReFS is detected and surfaces as `storage:block-clone` for future use. Volume snapshots are not implemented in v1; snapshot-dependent portable-spec rules are capability-gated off. A volume is mapped read-only or read-write per the consuming declaration ([wc[shell.volume]](#wc--shell.volume) uses the same mechanism for shell sessions).

> wc[backup.v1]
> Backups are performed by a single embedded method (kopia integration; cross-runtime rework, specified separately), not by container-based backup-app strategies. The embedded backup engine runs in seedlingd, whose service SID is on the standard volume ACLs ([wc[identity.file-permissions]](#wc--identity.file-permissions)); it reads volume contents from the host side without entering any workload's compute system, so no per-workload grant is needed. Application-consistent backup of stateful workloads (quiescing, log-based capture) is a property of the backup method and the workload's declared hooks, not of any database-specific runtime behaviour: the runtime special-cases no application.
