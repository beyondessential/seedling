# Kubernetes backend

## Context

Seedling today runs on a single Linux host: podman for containers, systemd for
process supervision, nftables for L4 routing, Caddy for L7 ingress. The
`crates/core/src/system.rs` layer already exposes four backend traits
(`ContainerRuntime`, `ProcessManager`, `NetworkProxy`, `DataPlane`) intended to
make the host the first of several backends.

The value proposition of seedling is that it sits **above** Kubernetes in
abstraction (BSL is higher-level than K8s manifests), and **incidentally**
avoids needing K8s on single servers where K8s would be overkill. To deliver
that, we need a Kubernetes backend in addition to the host backend. The same
BSL apps must run on either, with backend-specific feature variation gated by
explicit capabilities — not by silent semantic drift.

## Locked-in decisions

These are decisions made during the design conversation that opened this plan.
Anything in this section is treated as a constraint by the rest of the
document. To redirect any of them, edit this section first.

1. **`rt.signal` is unsupported on K8s.** K8s has no native signal API, and
   `kubectl exec kill -SIGNAME 1` requires `kill` in the image which not all
   minimal images carry. BSL apps that need signals fail at install time on K8s
   with a clear error from the capability check. Most apps don't need it
   (restart-the-deployment is the portable pattern).
2. **K8s clusters must be IPv6-capable.** IPv4-only K8s is explicitly not
   supported. IPv6-only is fully supported and is the prospective deployment
   target.
3. **NAT64/DNS64 is the cluster's responsibility.** Public image registries
   are still IPv4-only; v6-only clusters need a NAT64 gateway (Cilium, Tayga,
   Jool — operator's choice). seedling does not manage NAT64 in K8s mode.
4. **Seedling daemon runs in-cluster only.** No support for an external
   workstation seedling daemon talking to the K8s API. Development against
   `kind` / `microk8s` / a remote dev cluster is fine — those are still
   in-cluster from the daemon's perspective.
5. **State stays on SQLite, both backends.** K8s mode mounts the SQLite file
   on a PVC instead of a host directory. No PostgreSQL, no dual-DB migration
   path, no DB-engine abstraction layer. Trade-off: replicas=1 only. HA via DB
   replication (LiteFS, rqlite) is a separate, later concern that will apply to
   both backends uniformly if it happens.
6. **Daemon replicas = 1.** Reconciler is per-app exclusive-locked already.
   Leader election and HA are out of scope for v1.
7. **PostgreSQL operator-provided** if we ever switch — not bundled. Same
   policy for any future external dependency.
8. **Namespace per app + `seedling-system`.** Each installed BSL app gets its
   own namespace `seedling-app-<slug>`; the daemon, Caddy, and CoreDNS-or-equiv
   live in `seedling-system`. Free `NetworkPolicy` scoping, free per-app
   `ResourceQuota`, clean teardown via namespace delete, per-app RBAC for
   backups.
9. **Caddy stays as the L7+L4 termination layer.** Run as a `Deployment` in
   `seedling-system` with operator-configurable `Service` annotations (so EKS,
   GKE, MetalLB, etc. all work). seedling does not abstract over cloud LB
   providers — operators set their own annotations. Future migration to K8s
   Gateway API is possible but not v1. K8s `Ingress` is explicitly not used:
   per-controller path-strip semantics break BSL's no-strip contract, and it's
   HTTP-only.
10. **cert-manager is a prerequisite.** seedling translates BSL cert config to
    cert-manager `Certificate` + `Issuer` resources. Bundling cert-manager
    would conflict with clusters that already have it.
11. **Watch streams for everything observable.** K8s backend uses
    `kube`-rs `watcher`/`reflector` for Pods, StatefulSets, Jobs, Services,
    Ingresses, PVCs, VolumeSnapshots, Events. The world-observation history
    shape stays the same; only the source changes.
12. **Backend trait redesign is required.** The current four traits are at the
    wrong level (imperative per-container/per-unit) to be the cross-backend
    seam. They become internals of the host backend; new higher-level traits
    become the public seam. Detailed in §6.

## Open questions

These are not blocked on user input but should be revisited during
implementation. Listed for tracking.

- **CLI/OI access path in K8s mode.** Three options: `kubectl exec` into the
  daemon pod, port-forwarded API, or a dedicated Ingress for the OI. Probably
  all three (last as the production default). To be designed alongside the
  daemon's bootstrap.
- **Caddy's own Ingress for the seedling Web UI.** Bootstrap order — Caddy is
  the seedling-managed ingress, but the Web UI needs to be reachable before
  any BSL app is installed. Probably pre-seed a `seedling-system` site ingress
  pointing at the daemon pod.
- **NetworkPolicy stance.** Default deny + explicit allow per service mount, or
  default allow? Leaning default deny (matches the per-pod /64 isolation
  model on the host backend), but needs design.
- **Image warming on K8s.** `rt.warm_images` could pre-pull via a one-shot
  `Job` that runs `crictl pull` on every node, or via a `DaemonSet` with init
  containers. Or rely on imagePullPolicy + first-use lazy pull and skip
  warming on K8s. Probably the last for v1, with the trait method returning
  `Capabilities::supports_image_warming = false`.
- **VolumeSnapshot availability.** `HAS_SNAPSHOTS` becomes a runtime probe
  asking whether the cluster has at least one `VolumeSnapshotClass` matching
  the configured `StorageClass`. Need to land that probe before any backup
  app design.
- **Multi-node scheduling controls.** Can BSL `Deployment.scale=range(3,3)`
  spread across nodes via topology spread constraints? Probably yes but needs
  field on BSL or default config.

## Goals & non-goals

### v1 goals

- BSL apps that don't use unsupported primitives run unchanged on K8s.
- Single seedling daemon pod, in `seedling-system`, manages all installed apps
  in their own namespaces.
- All four BSL resource kinds — Deployment, Job, Service, Ingress — work.
- Volumes, including BTRFS-snapshot-equivalent backup app integration via
  `VolumeSnapshot`.
- TLS via cert-manager.
- Operator-configurable LB Service for Caddy.
- The host backend continues to work, identically, on the new trait seam.

### Non-goals (v1)

- HA daemon (replicas > 1, leader election).
- Multi-cluster federation.
- IPv4 K8s clusters.
- Bundled cert-manager / ingress controller / Postgres / NAT64.
- K8s Gateway API integration (deferred).
- Migration tools (host backend → K8s backend, or reverse).
- Cluster-wide multi-tenancy (one seedling per cluster, in its own
  `seedling-system`).
- `rt.signal` on K8s.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ seedling-system namespace                                       │
│                                                                 │
│  ┌───────────────────┐    ┌────────────┐    ┌────────────────┐ │
│  │ seedling-daemon   │    │ caddy      │    │ resolver       │ │
│  │ StatefulSet (1)   │    │ Deployment │    │ Deployment     │ │
│  │ ─ rusqlite        │    │ + LB Svc   │    │ + ClusterIP    │ │
│  │ ─ PVC: state      │    └────────────┘    └────────────────┘ │
│  │ ─ kube watch      │           ▲                              │
│  └─────────┬─────────┘           │                              │
│            │                     │ apply config                 │
│            │ kube API            │ (admin socket via Service)   │
│            │ (RBAC: cluster      │                              │
│            │  scope, scoped to   │                              │
│            │  managed ns)        │                              │
└────────────┼─────────────────────┼──────────────────────────────┘
             │                     │
             ▼                     ▼
┌─────────────────────────────────────────────────────────────────┐
│ seedling-app-<slug> namespace (one per installed BSL app)       │
│                                                                 │
│  ┌────────────────┐  ┌────────────┐  ┌──────┐  ┌─────────────┐ │
│  │ StatefulSet    │  │ Job (per   │  │ PVC  │  │ Service(s)  │ │
│  │ (BSL Deployment│  │  rt.start  │  │ ×N   │  │ (ClusterIP) │ │
│  │  scale=N)      │  │  on Job)   │  │      │  │             │ │
│  └────────────────┘  └────────────┘  └──────┘  └─────────────┘ │
│                                                                 │
│  + cert-manager Certificate, NetworkPolicy, ResourceQuota,      │
│    ServiceAccount, possibly VolumeSnapshot during backups       │
└─────────────────────────────────────────────────────────────────┘
```

### Daemon

- `StatefulSet` (not `Deployment`) so the PVC binding is stable across
  rescheduling.
- `replicas: 1`. Pod restart = brief downtime + state continuity, identical to
  systemd restart on the host backend.
- ServiceAccount `seedling-daemon` with cluster-scoped RBAC: namespace
  CRUD, plus full CRUD on `StatefulSet`, `Job`, `Pod`, `Service`,
  `PersistentVolumeClaim`, `VolumeSnapshot`, `cert-manager.io/Certificate`,
  `Secret`, `ConfigMap`, `NetworkPolicy`, `Event` within namespaces it manages.
  No cluster-admin.
- PVC bound to a `StorageClass` chosen at install time, holding the SQLite DB,
  the action log files, and any other persistent state. `ReadWriteOnce` is
  fine.
- Uses the in-cluster ServiceAccount token + CA via `kube` rs's in-cluster
  config.

### Caddy

- `Deployment` (not StatefulSet — no per-replica state). Replicas configurable;
  default 1.
- Single `Service`, `type: LoadBalancer`, with operator-configurable
  annotations passed through verbatim from seedling install config. Ports
  derived from declared site ingresses + app ingresses.
- Daemon writes Caddy admin config via the admin socket exposed by an
  in-namespace ClusterIP Service.

### Resolver

- Cluster already provides CoreDNS, but BSL's resolver model includes
  per-pod-network DNS rewrites and DNS64 synthesis. On K8s with cluster-level
  NAT64/DNS64, we may not need our own resolver.
- v1 plan: skip the seedling resolver entirely on K8s, lean on cluster DNS,
  document the cluster-DNS64 expectation.

### Per-app namespace

Each installed BSL app gets `seedling-app-<sanitised-name>`. Daemon creates
the namespace on install, deletes it on uninstall. Per-namespace:

- `ResourceQuota` derived from BSL `Deployment` memory/CPU limits × scale
  upper bound.
- Default-deny `NetworkPolicy` plus explicit allows for service mounts and
  ingress traffic.
- ServiceAccount with no permissions (workloads don't need K8s API access).

## BSL → Kubernetes mapping

This table is the source of truth for what each BSL primitive becomes. Drives
both the `K8sBackend` impl and the capability-gate logic.

| BSL primitive                | K8s realisation                                                  | Notes                                                                                            |
|------------------------------|------------------------------------------------------------------|--------------------------------------------------------------------------------------------------|
| App                          | One namespace `seedling-app-<slug>`                              | Created on install, deleted on uninstall.                                                        |
| Deployment (scale=N)         | `StatefulSet` with replicas=N                                    | Stable per-instance identity needed; Deployment hashes don't give it.                            |
| Deployment update strategy   | `RollingUpdate` (rolling) / `Recreate` (replace)                 | Direct map.                                                                                      |
| Deployment on_terminate      | StatefulSet pod always restarts                                  | Direct map.                                                                                      |
| Job (static)                 | `Job` with completions=1, parallelism=1, backoffLimit=0          | Single instance, never retries.                                                                  |
| Job (dynamic)                | New `Job` per action invocation, named with `operation_id`       | UUID5 derivation from operation_id stays the same.                                               |
| Container hardening defaults | `securityContext` (drop all caps, ro rootfs, runAsNonRoot)       | Direct map. PID/FD limits via `resources.limits` and pod-level `securityContext.sysctls`.        |
| Static volume writes         | Init container that materialises files from a Secret/ConfigMap   | Re-runs each pod start (matches BSL "applied on every container start" semantic).                |
| tmpfs static writes          | Init container against an `emptyDir { medium: Memory }`          | Re-applied each pod start naturally.                                                             |
| Healthcheck                  | seedling reconciles using `Pod.status.containerStatuses`         | **Don't** use liveness probe — too coarse. Readiness probe optional, controlled by BSL flag.     |
| Healthcheck `on_failure: replace` | seedling-orchestrated sibling spawn + traffic shift          | Implemented at the seedling reconciler level; K8s sees regular StatefulSet replicas.             |
| Service (TCP/UDP)            | `Service` ClusterIP                                              | seedling allocates the ClusterIP via Service spec.                                               |
| Service (HTTP)               | `Service` ClusterIP routed by Caddy with no-strip prefix routes  | Caddy is the L7 layer in both backends.                                                          |
| `localmount` stable address  | K8s Service ClusterIP (v6)                                       | BSL contract: "a stable address". Backend chooses how to source it.                              |
| Ingress                      | seedling-managed Caddy config + cert-manager `Certificate`       | NOT K8s `Ingress`. Caddy reads operator's LoadBalancer-bound external IP/hostname.               |
| Ingress hostname matching    | Reads `Service.status.loadBalancer.ingress` for Caddy LB         | Operator points DNS at this. BSL hostname is the SNI/Host-header match key.                      |
| Volume (named)               | `PersistentVolumeClaim` in app namespace                         | StorageClass operator-chosen at install.                                                         |
| Volume (tmpfs)               | `emptyDir { medium: Memory }`                                    | Direct map.                                                                                      |
| Volume (anonymous)           | `emptyDir`                                                       | Direct map.                                                                                      |
| Volume hold (delayed delete) | PVC `reclaimPolicy: Retain` + seedling-tracked release           | Operator confirmation gate stays in seedling.                                                    |
| External Volume              | Operator pre-provisioned PVC referenced by name                  | Operator config maps logical name to PVC.                                                        |
| Snapshots                    | `VolumeSnapshot` against the PVC                                 | Requires CSI driver with snapshot support. `HAS_SNAPSHOTS` becomes runtime probe.                |
| Backup source binding        | Snapshot → clone PVC (read-only) → mount in backup pod           | Operation-scoped, deleted at action end.                                                         |
| Backup destination binding   | Standard PVC (writable)                                          | Operation-scoped.                                                                                |
| `rt.exec`                    | `pods/exec` subresource                                          | At-most-once via seedling action log, unchanged.                                                 |
| `rt.signal`                  | **Unsupported.** Capability flag set false, install validates.   | Apps using `rt.signal` cannot install on K8s.                                                    |
| `rt.start`/`rt.stop`         | StatefulSet/Job apply or delete                                  | Declarative; observation via watch.                                                              |
| `rt.warm_certs`              | cert-manager `Certificate` with no Ingress reference yet         | Cert provisioned, traffic-binding deferred.                                                      |
| `rt.warm_images`             | **Optional / unsupported v1.** Capability flag set false.        | Defer to imagePullPolicy + lazy pull. Future: pre-pull DaemonSet.                                |
| `rt.write` to volume         | One-shot `Job` that mounts the PVC + writes the file             | Or daemon-side write via temporary mount-as-pod pattern. Logged per usual at-most-once.          |
| `rt.restart`                 | Trigger StatefulSet rolling restart (or replace per strategy)    | Same on_update strategy as initial deploy.                                                       |
| Crash loop detection         | Observe `CrashLoopBackOff` from pod status                       | Less tunable than systemd start-limit; documented limitation.                                    |
| Per-pod /64 IPv6             | Cluster CNI's pod-network-per-pod                                | Hidden from BSL; cluster-CIDR-derived not machine-id-derived.                                    |
| `HAS_SNAPSHOTS`              | Runtime probe of bound StorageClass's VolumeSnapshotClass        | True/false at install time.                                                                      |
| `HOST_HAS_IPV4`              | Cluster dual-stack probe                                         | False on v6-only clusters; documented assumption.                                                |
| `HOST_HAS_IPV6`              | Always true (v6 required)                                        | Capability prereq.                                                                               |
| Faults, action log, replay   | Unchanged (seedling-internal)                                    | DB layer is identical (rusqlite on PVC).                                                         |
| Generations, parameters      | Unchanged (seedling-internal)                                    | Same.                                                                                            |

## Backend trait redesign

The current `system.rs` exposes four traits — `ContainerRuntime`,
`ProcessManager`, `NetworkProxy`, `DataPlane` — at the wrong level for K8s.
They are imperative on host primitives (containers, units, bridges, nftables
rules). K8s wants declarative on cluster primitives (workloads, services,
ingresses).

These four traits don't get extended. Instead, they become **internals of the
host backend** — `HostBackend` composes `PodmanRuntime` + `SystemdManager` +
`NftablesDataPlane` + `CaddyProxy` to implement the new traits below, exactly
preserving today's behaviour.

The new cross-backend seam:

```rust
// crates/core/src/system/backend.rs (new)

pub trait Backend: Send + Sync + 'static {
    fn workload(&self) -> &dyn WorkloadBackend;
    fn traffic(&self) -> &dyn TrafficBackend;
    fn storage(&self) -> &dyn StorageBackend;
    fn capabilities(&self) -> &Capabilities;
}

pub struct Capabilities {
    pub supports_signals: bool,           // false on K8s
    pub supports_image_warming: bool,     // false on K8s v1
    pub supports_volume_snapshots: bool,  // runtime-probed
    pub has_ipv4_egress: bool,            // false on v6-only K8s
    pub has_ipv6_egress: bool,            // always true on K8s, host-probed elsewhere
    pub supports_btrfs_subvolumes: bool,  // host: maybe; K8s: false
    // ...
}

pub trait WorkloadBackend: Send + Sync + 'static {
    /// Idempotent. Specs include scale, image, env, mounts, healthcheck,
    /// resource limits, security context. Spec hash carried separately.
    fn ensure_workload<'a>(&'a self, spec: &'a WorkloadSpec)
        -> BoxFuture<'a, Result<(), BoxError>>;

    /// Watch-stream-backed observation. Returns current snapshot synchronously
    /// from a reflector cache; underlying watch keeps it fresh.
    fn observe_workload<'a>(&'a self, name: &'a WorkloadName)
        -> BoxFuture<'a, Result<WorkloadObservation, BoxError>>;

    fn delete_workload<'a>(&'a self, name: &'a WorkloadName, force: bool)
        -> BoxFuture<'a, Result<(), BoxError>>;

    fn exec<'a>(&'a self, instance: &'a InstanceId, argv: &'a [String], env: &'a [(String, String)])
        -> BoxFuture<'a, Result<i32, BoxError>>;

    /// Returns Capabilities::Unsupported on K8s.
    fn signal<'a>(&'a self, instance: &'a InstanceId, signal: &'a str)
        -> BoxFuture<'a, Result<bool, BoxError>>;

    /// Returns Capabilities::Unsupported on K8s v1.
    fn warm_image<'a>(&'a self, image_ref: &'a str)
        -> BoxFuture<'a, Result<(), BoxError>>;
}

pub trait TrafficBackend: Send + Sync + 'static {
    /// Includes ClusterIP allocation for services, route registration with the
    /// L7 proxy.
    fn ensure_service<'a>(&'a self, spec: &'a ServiceSpec)
        -> BoxFuture<'a, Result<ServiceEndpoint, BoxError>>;

    /// Caddy-config-shaped on both backends, but realised through different
    /// transports (admin socket on host, ConfigMap-or-API on K8s).
    fn ensure_ingress<'a>(&'a self, spec: &'a IngressSpec)
        -> BoxFuture<'a, Result<(), BoxError>>;

    fn observe_ingress<'a>(&'a self, name: &'a IngressName)
        -> BoxFuture<'a, Result<IngressObservation, BoxError>>;

    fn delete_service<'a>(&'a self, name: &'a ServiceName)
        -> BoxFuture<'a, Result<(), BoxError>>;
    fn delete_ingress<'a>(&'a self, name: &'a IngressName)
        -> BoxFuture<'a, Result<(), BoxError>>;

    /// Returns the externally-reachable address(es) for ingress binding.
    /// Host: host's public IP / Tailscale FQDN. K8s: Caddy LB Service status.
    fn external_endpoints<'a>(&'a self)
        -> BoxFuture<'a, Result<Vec<ExternalEndpoint>, BoxError>>;
}

pub trait StorageBackend: Send + Sync + 'static {
    fn ensure_volume<'a>(&'a self, spec: &'a VolumeSpec)
        -> BoxFuture<'a, Result<(), BoxError>>;

    fn delete_volume<'a>(&'a self, name: &'a VolumeName, retention: Retention)
        -> BoxFuture<'a, Result<(), BoxError>>;

    fn observe_volume<'a>(&'a self, name: &'a VolumeName)
        -> BoxFuture<'a, Result<VolumeObservation, BoxError>>;

    /// Returns Capabilities::Unsupported if !supports_volume_snapshots.
    fn snapshot_volume<'a>(&'a self, name: &'a VolumeName)
        -> BoxFuture<'a, Result<SnapshotHandle, BoxError>>;

    /// Materialise a snapshot as a new volume, ro or rw.
    /// Used for backup source binding (ro) and restore destination binding (rw).
    fn clone_snapshot<'a>(
        &'a self,
        snapshot: &'a SnapshotHandle,
        new_name: &'a VolumeName,
        mode: CloneMode,
    ) -> BoxFuture<'a, Result<(), BoxError>>;

    fn write_file<'a>(
        &'a self,
        volume: &'a VolumeName,
        path: &'a Path,
        contents: &'a [u8],
    ) -> BoxFuture<'a, Result<(), BoxError>>;
}
```

The shapes above are sketches, not finalised signatures. Concrete types
(`WorkloadSpec`, `ServiceSpec`, `IngressSpec`, `VolumeSpec`,
`WorkloadObservation`, etc.) are designed during phase 1 (see Phasing) to be
backend-neutral — driven by what BSL needs, not by what either backend
exposes.

### Why this shape, not "lift the four existing traits"

- `ContainerRuntime::create_container` + `ProcessManager::start_transient`
  collapse into `WorkloadBackend::ensure_workload`. The host backend's impl
  composes them; the K8s backend applies a `StatefulSet`. Neither impl leaks
  to the other side.
- `volume_mountpoint -> PathBuf` disappears from the cross-backend trait. The
  host backend uses host paths internally (it still needs them for podman
  mount specs); the K8s backend never has a host path to return. Callers
  outside the backend layer don't get a `PathBuf` they can't always interpret.
- `DataPlane` (nftables + IPv6 routes) becomes purely a host-backend internal,
  invoked from inside its `TrafficBackend` impl. K8s's `TrafficBackend`
  doesn't grow a no-op `DataPlane` member.
- `daemon_reload`, `reset_failed_unit`, `TransientUnitSpec` — all systemd-shaped
  primitives — don't appear in the new traits at all.

### The `System` struct

```rust
pub struct System {
    pub backend: Arc<dyn Backend>,
    pub volume_store: VolumeStore,  // moves into StorageBackend
}
```

Everything that today reaches into `system.container.list(...)` or
`system.process.start_transient(...)` rewrites to call through the new
trait. This is invasive in `crates/core/src/system/reconcile.rs` and
`actuator.rs` but mechanical.

## Subsystem details

### Workloads

A BSL `Deployment` becomes a K8s `StatefulSet`:

- `metadata.name` = `<deployment-name>` within app namespace.
- `spec.replicas` = current desired scale (within BSL `range`).
- `spec.serviceName` = `<deployment-name>-headless` (governing headless
  Service for stable DNS, even if the BSL Deployment has no public Service).
- `spec.updateStrategy.type` = `RollingUpdate` (BSL rolling) or `OnDelete`
  with seedling-orchestrated delete-then-create (BSL replace, since K8s
  StatefulSet has no `Recreate`).
- `spec.podManagementPolicy: Parallel` for replace, `OrderedReady` for
  rolling.
- Pod spec carries the full BSL container hardening profile.
- Init container that materialises static volume writes from a tmpfs-mounted
  Secret/ConfigMap into the target volume. Re-runs each pod start.

A BSL static `Job` becomes a K8s `Job`:

- `spec.completions=1, parallelism=1, backoffLimit=0`.
- Pod spec same hardening as Deployment.

A BSL dynamic `Job` (one per action invocation) is a fresh K8s `Job` per
invocation, named `<job-name>-<operation-id-prefix>`. Cleanup tied to action
completion + retention policy.

#### Healthcheck strategy

BSL healthcheck `on_failure: replace` requires sibling-spawn-then-promote
semantics that K8s liveness probes can't express. So:

- Liveness probe: not used (let the pod stay alive even when unhealthy; we
  manage the response).
- Readiness probe: optional, controlled by a BSL flag. When set, K8s gates
  Service backend membership on readiness. seedling reads pod readiness from
  watch.
- seedling reconciler observes pod status + healthcheck results (we run the
  healthcheck command via `pods/exec` on a schedule) and applies BSL's
  `on_failure: replace` logic by creating a sibling pod + draining traffic
  itself.

This mirrors what the host backend already does — keeps healthcheck logic
backend-uniform.

### Traffic

#### Services (ClusterIP)

BSL `Service` → K8s `Service` of type `ClusterIP`, in the app namespace.
Selector matches the StatefulSet pod labels. seedling does not allocate the
ClusterIP; K8s does, and seedling reads it back from `Service.spec.clusterIP`
to populate the `localmount` stable address contract.

#### Ingresses

Caddy in `seedling-system` is the only ingress termination. The daemon writes
Caddy admin config that routes `(hostname, port, proto)` to the appropriate
app-namespace Service ClusterIP. cert-manager `Certificate` resources provision
TLS certs into Secrets in `seedling-system`; Caddy reads them.

The Caddy `Service` type is `LoadBalancer`. Operator-supplied annotations on
that Service drive cloud LB provisioning. seedling reads
`Service.status.loadBalancer.ingress` to know the externally-resolvable IP /
hostname, and exposes it as the `external_endpoints()` answer for ingress
hostname binding.

For non-HTTP ingresses (TCP/UDP/DTLS), Caddy's L4 features handle termination.
The `Service` ports list is computed from the union of declared ingress ports.

### Storage

#### Named volumes

PVC per BSL named volume, in the app namespace, bound to operator-chosen
default `StorageClass`. PVC name `<volume-name>`. `accessModes:
[ReadWriteOnce]` baseline; `ReadWriteMany` only if BSL declares it (some
BSL volumes are mounted into multiple replicas — those need RWX storage).

#### tmpfs and anonymous

`emptyDir` (with `medium: Memory` for tmpfs) inside the pod spec. Lifecycle
tied to pod, which matches BSL anonymous-volume semantics.

#### Static writes

BSL: "static writes applied on scheduling and re-applied on every container
start". K8s realisation: an init container runs `cp` from a tmpfs-mounted
Secret/ConfigMap (containing the static contents) into the target volume.
Init containers re-run on every pod start, satisfying the BSL contract.

For tmpfs volumes, the same init container pattern works because
`emptyDir { medium: Memory }` is per-pod and starts empty.

#### Volume hold

BSL "named volumes are not deleted immediately, held for operator
confirmation". K8s realisation: PVC `persistentVolumeReclaimPolicy: Retain`
on the underlying PV, plus seedling-tracked release state. On confirmed
delete, seedling deletes the PV + underlying storage; on restore, it binds
a new PVC to the held PV.

#### Snapshots

`VolumeSnapshot` against the PVC. Requires that the bound `StorageClass` has
a matching `VolumeSnapshotClass`. On install, seedling probes the cluster for
this and sets `Capabilities::supports_volume_snapshots` accordingly.

For a backup app:
- Source binding: `VolumeSnapshot` of the source PVC → clone PVC
  (`spec.dataSource: VolumeSnapshot`) in the backup app's namespace, mounted
  read-only.
- Destination binding: a fresh PVC in the backup app's namespace, writable.
- Both lifecycles tied to the operation.

### Observation

`kube`-rs `watcher` + `reflector` for every kind seedling cares about:

- `StatefulSet`, `Job`, `Pod` → workload state.
- `Service`, `EndpointSlice` → service backend health.
- `cert-manager.io/Certificate` → cert state.
- `PersistentVolumeClaim`, `VolumeSnapshot` → volume state.
- `Event` → folded into world-observation history as additional facts.

The reflector cache feeds a sync `observe_*` API for the reconciler. Watch
disconnects trigger full re-list (kube-rs handles this); seedling's
world-observation history is append-only so reconciliation tolerates resyncs
naturally.

### Exec

`rt.exec(target, argv)` → `pods/exec` subresource via `kube`-rs. Action log
records exit code as today; replay reads it back, doesn't re-execute.

### Capabilities and validation

On install:

1. seedling evaluates the BSL script in dry-run mode to enumerate `rt.*`
   calls used.
2. Cross-references against `Capabilities`. Any unsupported primitive →
   install rejected with a clear error pointing to the line in the BSL script
   and the missing capability.
3. Same check on script update (don't allow updating an installed app to a
   script that uses unsupported primitives).

The host backend's `Capabilities` reports support for everything currently
supported. The K8s backend reports `supports_signals=false`,
`supports_image_warming=false`, `has_ipv4_egress=false`,
`supports_btrfs_subvolumes=false`. `supports_volume_snapshots` is runtime-set.

## Bootstrap & install

### Prerequisites (operator's responsibility)

Documented in `docs/k8s-install.md` (to write):

- IPv6-capable K8s cluster (dual-stack or v6-only).
- Cluster-level NAT64/DNS64 for v6-only clusters (Cilium NAT46, Tayga, Jool).
- cert-manager installed.
- A `StorageClass` (default or named) for seedling's PVCs, with a matching
  `VolumeSnapshotClass` if backups are wanted.
- A LB-provisioning controller (cloud-native, MetalLB, etc.) for the Caddy
  Service.
- kubeconfig for the install operator.

### Install method

`seedling-ctl install --kubeconfig <path>` (or a Helm chart, future).

The CLI:

1. Creates `seedling-system` namespace.
2. Applies daemon ServiceAccount + RBAC + StatefulSet + PVC.
3. Applies Caddy Deployment + Service (with operator-supplied annotations).
4. Waits for daemon pod to be ready.
5. Applies a self-managed site ingress for the seedling Web UI / OI API.

### Operator config surface

A `seedling-config.yaml` (ConfigMap or values file) carrying:

- `defaultStorageClass`
- `caddyServiceAnnotations: {...}`
- `caddyServiceType: LoadBalancer | NodePort | ClusterIP+hostNetwork`
- `cert-manager.defaultIssuer`
- `db.path` (PVC mount path; defaults to `/var/lib/seedling/state.db`)
- `appNamespacePrefix: seedling-app-` (default)
- (more during implementation)

## CLI / OI in K8s mode

Three modes, in priority order:

1. **Production**: dedicated site ingress in `seedling-system` for the OI API
   + Web UI, TLS via cert-manager. Operator uses normal seedling auth.
2. **Cluster-admin debug**: `kubectl exec` into the daemon pod and run
   `seedling-ctl` against the local Unix socket.
3. **Dev**: `kubectl port-forward` the daemon's API port and run `seedling-ctl`
   from a workstation.

## Phasing

Phases are sequenced to keep the host backend always-working and the
trait-redesign work landed before any K8s-specific code lands.

### Phase 1 — backend trait redesign, host-only

- Define `Backend`, `WorkloadBackend`, `TrafficBackend`, `StorageBackend`,
  `Capabilities` and the supporting `*Spec` / `*Observation` types in
  `crates/core/src/system/backend.rs`.
- Implement `HostBackend` that composes today's `PodmanRuntime`,
  `SystemdManager`, `NftablesDataPlane`, `CaddyProxy` to satisfy the new
  traits.
- Migrate `crates/core/src/system/reconcile.rs`, `actuator.rs`, and the
  daemon's startup paths to use the new traits exclusively.
- Behaviour identical to today. Tests pass.

Output of phase 1: a single backend, but the seam is in the right place. K8s
backend slots in alongside without further trait churn.

### Phase 2 — capability gating

- Implement `Capabilities` enforcement in the install path.
- Add the dry-run BSL evaluator that enumerates `rt.*` use.
- Host backend reports all-supported; nothing rejected on host.

### Phase 3 — K8s backend skeleton

- `crates/core/src/system/k8s/` with module structure mirroring
  `crates/core/src/system/podman.rs` / `systemd.rs`.
- `kube`-rs dependency added.
- `K8sBackend` implementing the new traits with `todo!()` bodies.
- Daemon binary grows a `--backend k8s` flag and an in-cluster startup path
  (RBAC discovery, kube client setup, namespace ensure).

### Phase 4 — workloads on K8s

- `WorkloadBackend` impl: StatefulSet for Deployment, Job for Job.
- Watch-backed observation.
- Init-container static-write materialiser.
- `pods/exec` for `rt.exec`.
- Healthcheck command execution loop.
- BSL Deployment/Job test apps green end-to-end.

### Phase 5 — traffic on K8s

- In-cluster Caddy Deployment + Service.
- `TrafficBackend` impl: Service + Caddy admin config.
- cert-manager integration for ingress certs.
- Operator-configurable LB Service annotations.

### Phase 6 — storage on K8s

- `StorageBackend` impl: PVC for named, emptyDir for tmpfs/anon.
- `VolumeSnapshot` for snapshots.
- Backup app source/destination binding via clone PVC.

### Phase 7 — bootstrap & operator UX

- `seedling-ctl install --kubeconfig`.
- `seedling-config.yaml` schema.
- Web UI / OI ingress site-ingress.
- Documentation: `docs/k8s-install.md`, updates to `docs/runtime-overview.md`.

### Phase 8 — hardening

- NetworkPolicy default-deny per app namespace, explicit allow rules.
- ResourceQuota per app namespace.
- End-to-end tests on `kind` (CI-friendly), spot tests on real cloud
  cluster.
- Capability reporting reflected in Web UI (so operators see why a BSL app
  can't install).

## Spec changes

Each phase that ships behaviour visible to BSL authors or operators must update
the corresponding spec under `docs/spec/` first, then implement, then add
tests. Notable areas:

- `language.md`: nothing structural — BSL contract is preserved. Add a
  capability-gate paragraph noting that `rt.signal`, `rt.warm_images`, and the
  BTRFS subvolume features may be unavailable depending on backend.
- `runtime.md`: adds a "backends" section describing the abstraction,
  enumerating capabilities, describing the install-time validation.
- `interface.md`: operator-visible additions — `seedling-ctl install
  --kubeconfig`, capability reporting in app status, K8s-specific config
  surface.
- New: a backend-specific operator guide alongside `docs/networking.md`,
  describing K8s prerequisites and bootstrap.

## Things to watch

- **Caddy admin config size on K8s.** With many ingresses, the admin payload
  grows. Need to confirm the admin API handles incremental updates well, or
  switch to a config-source-of-truth-on-disk model where Caddy reloads.
- **Init container time tax.** Every pod start runs a static-write init
  container, even if writes are unchanged. Fast in practice, worth measuring
  on the test suite.
- **Watch disconnects under load.** kube-rs `reflector` handles this but
  full re-list of large namespaces is expensive. Maybe introduce per-app
  scoped watches rather than cluster-wide.
- **PVC binding latency.** First-time PVC creation can take seconds on some
  CSI drivers. The reconciler must tolerate "PVC pending → bound" as a
  scheduled-but-not-ready state. Already in the BSL volume lifecycle model;
  just needs wiring.
- **VolumeSnapshot cleanup.** Snapshots accumulate cost. Operation-scoped
  snapshots for backups must be deleted at action end, with a sweeper for
  orphans (action interrupted by daemon crash).
