# Windows Container Runtime: Plan, Open Questions, Spikes

Companion to the draft `runtime-windows-containers.md`. It records the implementation approach, the artifact format, the open questions, and the spikes. The spec stays at behaviour altitude; the choices here are how that behaviour is realised.

## Shared tracks (not restated here)

- **Portable-spec restructuring**: extract the portable core of `runtime.md`, add the capabilities surface to `/status` and the language, restate the exit-code convention, stand up a rule-ID-keyed conformance suite in CI. Prerequisite for merging an implementation, not for prototyping.
- **Backup rework**: one embedded method across runtimes, specified separately.

## Artifact format: base-less OCI image, composed onto the runtime's base

A workload ships as an ordinary OCI image whose layers carry only the workload's own filesystem — no OS base baked in. The runtime completes it for execution by stacking its single [base](../spec/runtime-windows-containers.md) beneath the artifact's layers, exactly as a container platform stacks a base under app layers.

Why this over shipping a filesystem blob (e.g. a VHDX):

- **Content-addressed layers** give layer dedup and incremental pulls — successive workload versions transfer only their diff, which matters over constrained field links. A monolithic blob re-transfers in full every version.
- It is an **ordinary OCI image**, so build, push, signing, replication, and GC are standard registry tooling rather than a bespoke pack pipeline.
- **Composition is the platform-native layer-stack operation**, not a blob-to-layer materialisation step.
- A base-less image carries no OS, so "stack the runtime's base beneath it" is the natural completion and no base is redundantly shipped. Files land at real paths, so there is no root-directory indirection.

The correctness constraint is **base-independence**: restacking is sound when the layers only add the workload's own files and assume nothing about the base build (no mutation of system files, registry, COM, or services). `FROM scratch` enforces this; it also matches the target workloads (a directory of self-contained files, e.g. a bundled Node app). Workloads needing OS-level install steps do not fit and are out of scope for the format.

Two producer paths yield the same consumed artifact; a spike picks between them:

- **Literal `FROM scratch`** — zero base coupling, but the layers must be emitted in Windows layer form (Windows file metadata: ACLs, attributes), so the producer uses Windows-format layer tooling rather than a generic scratch build.
- **Stock `FROM mcr/...` with the base omitted on push** — 100% standard Windows `docker build`; the pushed image is already app-layers-only (base as a foreign reference), and the runtime ignores that reference and stacks its own base. The residual risk is a layer diffed over base build *X* restacked on build *Y*, which is safe under the same base-independence constraint.

## Implementation approach: containerd + runhcs shim + Windows snapshotter

The runtime is built on containerd's Windows stack rather than hand-rolling the compute plane on the HCS/HNS APIs. The deciding factor is the core property **"seedlingd can restart without losing workloads"**, and containerd's architecture delivers it by construction:

```
seedlingd → containerd → runhcs shim (per instance) → HCS compute system
```

The `io.containerd.runhcs.v1` shim is the per-instance supervisor — it *is* the `wc[pod]` of the spec. It owns the container via HCS and stays alive independently of both containerd and seedlingd. Survival at each cut point: seedlingd restart touches nothing below it (it is not in the runtime path); a containerd restart leaves shims and containers running and re-attaches on return (the same mechanism behind containerd's own zero-downtime upgrades); shim death drops its one instance (reconciled like any exit); only HCS/host death drops everything (reboot, a non-goal). This is a *stronger* daemon-independence story than hand-rolling the pod ourselves, because the decoupled supervisor and its reconnect are prebuilt and maintained by Microsoft.

**containerd is a seedling-managed infrastructure dependency, not an always-on peer daemon.** Because it restarts without workload loss, seedlingd owns its lifecycle like any other infra service (`wc[infra.services]`): it is the lowest dependency in the graph — **containerd → resolver → backends → ingress**, teardown reverse — started on demand and stopped when no workload remains. A host with no workloads runs one daemon (seedlingd, the SCM auto-start control plane and OI endpoint); a deploy brings containerd up, then the containers. Installation is placing the containerd and shim binaries and registering containerd as a demand-start service. containerd stays co-resident whenever any workload exists, because it is seedlingd's channel to observe exits and issue commands; it is torn down only when the world is empty.

**Restack is a content-store operation, not a custom snapshotter.** The snapshotter builds a chain from a manifest's layers, so to stack the runtime's base beneath a base-less artifact, seedlingd synthesises a merged image descriptor (base layers ++ artifact layers) in the content store and runs that. This is metadata only — no layer data is copied.

**Networking stays seedling-owned.** containerd's CNI is bypassed; seedlingd drives HNS directly for the fabric, per-compartment endpoints, mount-graph enforcement, ingress host-port publishing, and resolver DNS. This mirrors the Linux runtime, where seedling uses an engine for compute but owns the dataplane (nftables) itself.

**Rust ↔ containerd is gRPC** via the `containerd-client` crate. The fallback, if a spike disqualifies containerd (see Spike A), is to hand-roll the compute plane directly on the Compute\* flat-C APIs (`computecore` / `computestorage` / `computenetwork`) through the `windows` crate already used by the spikes — no second language, but seedlingd then owns the pod, reattach, and layer sequencing itself.

## Open questions (owner: spec sessions)

| # | Question | Current lean |
|---|----------|--------------|
| Q1 | Producer path for the artifact: literal `FROM scratch` vs stock `FROM mcr` with base omitted | Prefer stock build for tooling simplicity; confirm restack-compat across base builds in the format spike. |
| Q2 | Merged-descriptor restack: does the Windows snapshotter accept a synthesised base-plus-app chain and run it unmodified | Prototype in Spike B; if the snapshotter resists a synthesised chain, a thin custom snapshotter that always inserts the base is the fallback. |
| Q3 | HNS network mode giving per-instance compartment, routable service address, endpoint-scoped mount enforcement, and host public-port publishing for ingress | Evaluate in Spike C against a worst-case field image; the mode must express the mount graph without a host-firewall fallback. |
| Q4 | containerd lifecycle management: demand-start service vs seedlingd child; content-store and image GC ownership | Demand-start service driven by seedlingd; align image GC with the `/images` surface. Settled during Spike A. |

## Spikes

- **A. containerd survival + control (the decider)** — the load-bearing spike; its outcome chooses containerd (B) or the hand-rolled fallback (A). Exit criteria:
  1. A process-isolated Windows container keeps running across a full containerd service stop/start, and containerd re-attaches to the shim and resumes reporting task state. *(This is the make-or-break; if it fails, fall back to hand-rolling the pod.)*
  2. seedlingd drives containerd from Rust over gRPC: create, start, stop, exec, and receive exit events.
  3. On-demand lifecycle: seedlingd starts containerd on first workload and stops it when the world empties, without disturbing the content store or pulled images.
  4. If (1) fails, measure the thickness of the fallback: pod + reattach + layer sequencing directly on the Compute\* APIs, to price option A.
- **B. Composition + base store** — pull a build-matching base; synthesise a merged base-plus-app descriptor (Q2); run it; measure per-composition cost and layer cache/dedup behaviour; base pull-once-per-build and pre-staging.
- **C. Networking on a worst-case image** (field disk image) — per-instance compartment and endpoint; service addresses; mount graph compiled to compartment-boundary enforcement with default-deny; ingress host public-port publishing; per-compartment resolver DNS; coexistence with field AV/EDR. Decides Q3.
- **D. Exec + ConPTY + volume shells** — run an action process inside a running container; ConPTY shell attach and resize; volume shells via a container with volumes mapped.
- **E. Stop delivery** — console-control-event and named-event delivery to a workload inside its container; exit-code recording.

## Rollout

- Fleet reality: ~5 deployments, ~25 Windows hosts, drift assumed — including OS build number, which gates which base is usable.
- `seedling doctor` per-host preflight: container platform present, containerd and shim installed and version-compatible with the host build, base image cached for the current build, network fabric creation, image composition, volume mapping, Defender exclusions, Server version — reported through the capabilities surface and aggregatable fleet-wide. A host patched without a matching base cached is an actionable verdict.
- Pre-stage base images onto egress-restricted hosts during provisioning; the store is the single source composition draws from, so a pre-staged base is indistinguishable from a pulled one.
- Run doctor across all hosts before the pilot; the support matrix (per-host build and base coverage) chooses the pilot and the sequencing.

## Migration is an operations concern

Whatever a field host runs today is migrated by packaging it as an ordinary artifact-backed workload and cutting over; quiescing and importing existing state is an operations runbook, out of scope for the runtime. There is deliberately no host-service-adoption mechanism.

## Cost ledger

- **Footprint** — the second-daemon objection is dissolved: containerd is a seedling-managed infra dependency, absent on idle hosts and restartable without workload loss, so idle steady-state is still one daemon.
- **Seam** — Rust↔containerd is gRPC (mature-enough client) rather than in-process.
- **Compatibility matrix** — containerd × hcsshim × host build must be tracked fleet-wide, but it is a Microsoft-supported matrix surfaced by doctor.
- **Glue** — restack via synthesised merged descriptor (Q2) and seedling-owned HNS networking are ours to build; the latter is unavoidable in any option.
- **Gate** — the whole approach rests on Spike A criterion 1 (Windows shim-reconnect across a containerd restart). It holds by design; the spike confirms it on the platform floor, and the hand-rolled fallback is priced in the same spike.
- **Payoff** — the compute plane (image service, snapshotter/restack, per-instance supervisor, reattach) is reused and Microsoft-maintained rather than hand-rolled, and the daemon-independence property is delivered by construction.
