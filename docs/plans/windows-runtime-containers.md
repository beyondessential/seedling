# Windows Container Runtime: Plan, Open Questions, Spikes

Companion to the draft `runtime-windows-containers.md`. This design is an alternative to the process-native Windows runtime (`runtime-windows.md` / `windows-runtime.md`); the two share the portable-spec restructuring and the backup rework, and differ in the infrastructure layer. This document records only what is specific to the container design; where a workstream is shared, it points at the process-native plan rather than restating it.

## Where this fits

The goal is unchanged: a second Seedling implementation with Windows-native primitives speaking the same operator interface, so the existing CLI (`ctl`), web UI, and protocol crates carry over. The difference from the process-native draft is what supplies isolation, addressing, and mount-graph enforcement: the container boundary (HCS compute system + HNS compartment) instead of host primitives (Job Objects + per-instance SCM services/SIDs + loopback aliasing + WFP + supervisor relay).

What that buys, and why this plan exists:

- The entire per-instance and per-invocation identity apparatus (`win[identity.virtual-account]`, `win[identity.dynamic-jobs]`, the 4697 service-install audit churn and its two retreat positions) disappears — replaced by [wc[identity.container-principal]](../spec/runtime-windows-containers.md).
- The networking/enforcement stack collapses from loopback aliasing + `skipassource` + WFP filters + a supervisor byte-relay to a fabric with per-compartment endpoints and endpoint-scoped allows; mounts become direct-dial with no relay hop, and enforcement becomes mandatory at the namespace boundary rather than discretionary.
- The supervisor loses its data plane: it owns exit-policy, log capture, and the stop ladder only, shrinking its reliability budget.

What it costs, and what this plan must de-risk: a Rust integration against HCS/HNS, a just-in-time layer composition step, and a new operational dependency on a build-matching OS base being present per host.

## Workstreams

### 1. Spec restructuring (shared; prerequisite for merging, not for prototyping)

Identical to the process-native plan's workstream 1 — extract the portable core of `runtime.md` into a shared document, add the `capabilities` field to `/status` and `rt.capability()` to the language, restate the exit-code convention, and stand up a rule-ID-keyed conformance suite run against every runtime in CI. The capability vocabulary gains `isolation:namespace` and `net:compartment` ([wc[capability.map]](../spec/runtime-windows-containers.md)). No divergence from the process-native plan here.

### 2. Backup rework (shared; cross-runtime, separate track)

Unchanged from the process-native plan: one embedded kopia method across runtimes, and resolve the operation-volume machinery's fate before porting [wc[action.volume-params]](../spec/runtime-windows-containers.md). This design removes the last database-specific runtime behaviour, so application-consistent capture is entirely a property of the backup method plus workload hooks ([wc[backup.v1]](../spec/runtime-windows-containers.md)), not of the runtime.

### 3. Windows daemon + seedpod implementation

Sequencing suggestion: HCS compute-system lifecycle and supervisor reattachment first (Spike A — everything composes with it), then JIT composition and the base-image store (Spike B), then the HNS fabric and mount-graph enforcement (Spike C), then shells/actions (Spike D). Networking depends on compute systems existing; composition depends on the store; enforcement depends on endpoints.

## Open questions (decisions needed, owner: spec sessions)

| # | Question | Current lean |
|---|----------|--------------|
| Q1 | HCS/HNS from Rust: FFI `vmcompute`/computecore directly, vendor `hcs-rs`, or bind a minimal C shim? | Direct FFI to the documented HCS/HNS flat-C surface; treat `hcs-rs` as reference, not dependency (0.1.0, stale). Pinned in Spike A. |
| Q2 | Fabric HNS network mode (nat / l2bridge / transparent / ics) that both allocates stable per-endpoint addresses and supports endpoint-scoped allow/deny for the mount graph | Evaluate in Spike C against a worst-case field image; the mode must express `wc[fw.allows]` without a per-mount host-firewall fallback. |
| Q3 | Default base family when the artifact omits `base` — minimal vs fuller | Lean fuller (broadest workload compatibility); minimal is an artifact opt-in. Confirm the fuller base's build-match cadence is tolerable in Spike B. |
| Q4 | Composed/materialised-layer cache: key, invalidation, and disk budget across live generations + drained old generations | Key on (base build, app-layer digest); GC drained generations promptly; size-cap the store with pin protection. Measured in Spike B. |
| Q5 | Supervisor-death re-adoption: can a replacement supervisor cleanly re-own an orphaned compute system (`OpenComputeSystem` + re-attach console/exit-wait) without racing HCS? | Prototype in Spike A; if re-adoption is not clean, fall back to supervisor death terminating the compute system (the process-native `win[supervisor.crash]` behaviour) and document the regression. |

## Spikes (confirm before the corresponding rules lose their `[spike]` tag)

- **A. Compute-system lifecycle + reattach** — create/start/stop a process-isolated compute system from Rust; supervisor breakaway; daemon reattach after restart; supervisor-death re-adoption (Q5); pipe protocol (`wc[pod.*]`). This is the load-bearing spike: it decides Q1 and whether the daemon-independence property survives.
- **B. JIT composition + base store** — pull a build-matching base; materialise a VHDX subtree into a read-only container layer (CimFS); compose and run; measure per-composition cost and cache hit behaviour; base-drift detection for `seedling doctor` (`wc[base.*]`, `wc[compose.jit]`, `wc[artifact.compose]`).
- **C. Fabric networking on a worst-case image** (field disk image) — one HNS fabric; per-instance compartment + endpoint + stable address; mount graph compiled to endpoint allows with default-deny; ingress host-port mapping; per-compartment resolver DNS; coexistence with field AV/EDR (`wc[net.*]`, `wc[fw.*]`, `wc[special.*]`). Confirms Q2.
- **D. Exec + ConPTY into a running compute system** — spawn an action process inside an instance's compute system; ConPTY shell attach and resize; volume shells via a base-image compute system with volumes mapped; empty-stderr contract (`wc[action.exec]`, `wc[shell.*]`).
- **E. Stop delivery inside a compute system** — ctrl-event delivery to the workload process group inside the container, exit-code synthesis, named-event and reload paths (`wc[stop.*]`, `wc[signal.map]`).

## Rollout

- Fleet reality: ~5 deployments, ~25 Windows hosts, varying circumstances. Drift is assumed — and here drift has a new dimension: OS build number, which gates base-image availability (`wc[base.match]`).
- Build `seedling doctor` early: per-host preflight for Containers-feature/HCS/HNS presence, base-image availability for the current build and required families, fabric creation, VHDX attach + composition, Defender exclusions, Server version — reported through the same capabilities vocabulary as `/status`, aggregatable fleet-wide. Base-drift ("host patched, matching base not yet cached, no registry egress") must be a distinct, actionable doctor verdict.
- Pre-stage base images onto air-gapped or egress-restricted hosts as part of provisioning; the store is the single source the composer draws from, so a pre-staged base is indistinguishable from a pulled one.
- Run doctor across all 25 hosts *before* the pilot; the resulting support matrix (including per-host build and base coverage) chooses the pilot and the sequencing.

## Migration is an operations concern, not a runtime concern

The runtime special-cases no application. Whatever a field host runs today (a native PostgreSQL service, a PM2-supervised Node process, a hand-configured Caddy) is migrated by wrapping it as an ordinary artifact-backed workload and cutting over; how an operator quiesces and imports existing state is an operations runbook, out of scope for the runtime spec. There is deliberately no "adopt the host's existing database service" mechanism in this design — that coupling is what the process-native draft carries and what this draft removes.

## Relationship to the process-native draft

Both drafts target the same operator interface and share workstreams 1 and 2. They diverge only in workstream 3, and the divergence is total in the infrastructure layer: a design session should pick one, not merge them. The honest cost ledger for choosing this one over the process-native draft:

- **New build risk:** HCS/HNS-from-Rust (Q1) is a larger, less-documented integration surface than the Win32 primitives (Job Objects, WFP, SCM) the process-native draft stands on. Spike A retires or confirms this risk before commitment.
- **New operational dependency:** workloads cannot start on a host build with no matching base cached; native processes have no such dependency. Mitigated by pre-staging and doctor, but real.
- **Retained property to prove, not assume:** daemon-independent workload survival (`wc[boot.cold]`) depends on Q5 resolving cleanly. If it does not, the fallback narrows the property to match the process-native draft's supervisor-death behaviour.

Against those, the win is the collapse of the identity + networking + enforcement machinery — the process-native draft's most complex, most spike-laden, most operationally fragile third — into the container boundary, plus mandatory (rather than discretionary) isolation between workloads.
