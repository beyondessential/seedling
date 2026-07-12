# Windows Container Runtime: Plan, Open Questions, Spikes

Companion to the draft `runtime-windows-containers.md`. This is one of two candidate Windows runtime designs; a design session picks one. This plan records what is specific to the container design and does not restate the shared tracks.

## Shared tracks (not restated here)

- **Portable-spec restructuring**: extracting the portable core of `runtime.md`, adding the capabilities surface to `/status` and the language, restating the exit-code convention, and a rule-ID-keyed conformance suite in CI. Prerequisite for merging an implementation, not for prototyping.
- **Backup rework**: one embedded method across runtimes, specified separately. The runtime special-cases no application, so application-consistent capture is a property of the backup method and workload hooks, not of runtime behaviour.

## Design decisions carried into the spec

- **One base image, runtime-chosen.** The runtime pulls a single OS base matching the host build and layers every workload over it. No per-artifact base selection and no configurable mirror — either would multiply base downloads and defeat the point of composing locally.
- **Composition at prepare time.** The runnable image is composed when an app image is prepared into the store; starting an instance only stacks a scratch layer. Container start stays cheap and does no per-start layer work.
- **Lean pod.** The per-instance supervisor owns its container's lifetime and restart policy, and holds observed events in memory to hand to seedlingd on reconnect. If the pod is gone, seedlingd reconciles the instance from the observed world, exactly as it does when a Linux supervisor disappears. Durable record-keeping and logging live in seedlingd.
- **Bind like a normal container.** Workloads bind all interfaces inside their own network compartment; there is no injected bind address, because the compartment already isolates the listener.
- **Linux dataplane model.** Service-address and mount-graph reachability follow the portable dataplane rules, realised at the compartment boundary rather than reconstructed per workload.

## Workstream: Windows daemon + pod implementation

Sequencing: container lifecycle and pod reconnect first (everything composes with it), then composition and the base store, then networking and mount-graph enforcement, then infra services, then actions and shells.

## Open questions (owner: spec sessions)

| # | Question | Current lean |
|---|----------|--------------|
| Q1 | HCS/HNS from Rust: FFI the documented flat-C surface, vendor an existing wrapper, or bind a minimal C shim | Direct FFI to the HCS/HNS flat-C surface; treat existing wrappers as reference. Pinned in Spike A. |
| Q2 | Which HNS network mode gives a per-instance compartment with a routable service address, endpoint-scoped mount enforcement, and host public-port publishing for ingress | Evaluate in Spike C against a worst-case field image; the mode must express the mount graph without a host-firewall fallback. |
| Q3 | Composition mechanics: materialise the artifact filesystem into a layer (e.g. CimFS) and cache it; invalidation and store disk budget | Key composed layers on the app image digest; size-cap the store with pin protection. Measured in Spike B. |
| Q4 | Pod reconnect: identity verification and the in-memory event handoff protocol | Prototype in Spike A; verify the pod's process identity before adopting; hand off events on connect. |

## Spikes

- **A. Container lifecycle + pod reconnect** — create, start, stop a process-isolated container from Rust; pod independence from seedlingd; reconnect and event handoff; identity verification. Decides Q1 and whether workload-survives-seedlingd-restart holds in practice.
- **B. Composition + base store** — pull a build-matching base; materialise an artifact filesystem into a composed layer; measure per-composition cost and cache behaviour.
- **C. Networking on a worst-case image** (field disk image) — per-instance compartment and endpoint; service addresses; mount graph compiled to compartment-boundary enforcement with default-deny; ingress host public-port publishing; per-compartment resolver DNS; coexistence with field AV/EDR. Decides Q2.
- **D. Exec + ConPTY + volume shells** — run an action process inside a container; ConPTY shell attach and resize; volume shells via a container with volumes mapped.
- **E. Stop delivery** — console-control-event and named-event delivery to a workload inside its container; exit-code recording.

## Rollout

- Fleet reality: ~5 deployments, ~25 Windows hosts, drift assumed — including OS build number, which gates which base is usable.
- `seedling doctor` per-host preflight: container platform present, base image cached for the current build, network fabric creation, image composition, volume mapping, Defender exclusions, Server version — reported through the capabilities surface and aggregatable fleet-wide. A host patched without a matching base cached is an actionable doctor verdict.
- Pre-stage base images onto egress-restricted hosts during provisioning; the store is the single source the composer draws from, so a pre-staged base is indistinguishable from a pulled one.
- Run doctor across all hosts before the pilot; the support matrix (per-host build and base coverage) chooses the pilot and the sequencing.

## Migration is an operations concern

Whatever a field host runs today is migrated by packaging it as an ordinary artifact-backed workload and cutting over; quiescing and importing existing state is an operations runbook, out of scope for the runtime. There is deliberately no host-service-adoption mechanism in this design.

## Cost ledger for choosing this design

- **Build risk:** HCS/HNS-from-Rust (Q1) is a larger, less-documented integration surface than plain Win32 process and service APIs. Spike A retires it before commitment.
- **Operational dependency:** a workload cannot start on a host build with no matching base cached. Mitigated by pre-staging and doctor, but real.
- **Payoff:** isolation, addressing, and mount-graph enforcement come from the container boundary instead of being reconstructed out of host primitives, and isolation between workloads is enforced by the platform rather than by discretionary host filters.
