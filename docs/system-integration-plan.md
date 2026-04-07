# System Integration Layer — Implementation Plan

## Overview

The system integration layer (`src/system/`) is the bridge between the runtime's
abstract resource model and actual system primitives: podman containers, systemd
units, a Caddy reverse proxy, and kernel-level networking.

It is cleanly separated from the other two layers:

- `src/defs/` — BSL types and scripting; no I/O
- `src/runtime/` — scheduling, barriers, desired-state logic; no I/O
- `src/system/` — all system I/O; no BSL knowledge

The layer operates in two directions:

- **Observation**: inspect real system state → produce `ObservationFact` entries
  → caller persists them to `world_observations`
- **Actuation**: consume desired-state diffs → drive system backends → caller
  records in `autonomous_operations`

The reconciliation loop that coordinates these two directions lives at the top
level (wiring `src/runtime/` to `src/system/`), not inside either module.

---

## Design constraints

- **Static dispatch only.** Backends are selected at compile time via generics
  (`SystemDriver<C, P, N, D>`). No `dyn` trait objects for the backends.
- **Rootful podman** for the initial implementation. Rootless is a separate
  `ContainerRuntime` implementation chosen at startup, not a runtime flag.
- **tokio multi-threaded** async runtime throughout.
- **IPv6-only internal networking.** All pod-to-pod and pod-to-service traffic
  uses IPv6. External ingress is dual-stack. NAT64 is out of scope for now.
- **snafu** for typed errors. Per-backend error enums are internal; the system
  boundary exposes only `ObserveError` and `ActuateError`.
- **Pluggable by design.** The four backend traits are the extension points.
  Swapping any individual backend requires only a new struct implementing the
  corresponding trait.

---

## Concrete backends — first implementation

| Trait              | Struct              | Transport                                                     |
|--------------------|---------------------|---------------------------------------------------------------|
| `ContainerRuntime` | `PodmanRuntime`     | libpod REST API over unix socket at `/run/podman/podman.sock` |
| `ProcessManager`   | `SystemdManager`    | system D-Bus via `zbus`; transient + persistent unit control  |
| `NetworkProxy`     | `CaddyProxy`        | Caddy admin API; IP discovered by container inspection        |
| `DataPlane`        | `NftablesDataPlane` | nftables (via `nftables` crate) + rtnetlink routing table     |

---

## Module layout

```
src/system/
    mod.rs             SystemDriver<C,P,N,D>; pub re-exports of boundary types
    types.rs           All shared data types
    observer.rs        Observer<C,P,N,D>
    actuator.rs        Actuator<C,P,N,D>
    translate/
        mod.rs
        container.rs   DeploymentDef/JobDef → ContainerSpec  (pure functions)
        proxy.rs       active ingresses + service IPs → ProxyConfig (pure)
    podman.rs          PodmanRuntime: impl ContainerRuntime
    systemd.rs         SystemdManager: impl ProcessManager
    caddy.rs           CaddyProxy: impl NetworkProxy
    data_plane.rs      NftablesDataPlane: impl DataPlane
```

---

## IPv6 internal addressing

All internal networking uses IPv6. External traffic is handled by Caddy
(dual-stack). NAT64 for pod outbound IPv4 access is out of scope.

### ULA prefix structure

Addresses follow RFC 4193 ULA format: `fd` (8 bits) + Global ID (40 bits) +
Subnet ID (16 bits) + Interface ID (64 bits) = 128 bits.

The node's `/48` prefix is `fd5e:XXYY:ZZWW::/48`, derived as follows:

- **Bytes 0–1** (`fd5e`): fixed seedling ULA magic. Any address beginning with
  `fd5e:` is seedling-managed internal traffic.
- **Bytes 2–5** (`XX YY ZZ WW`): first four bytes of `SHA-256(machine-id)`,
  where `machine-id` is the whitespace-trimmed content of `/etc/machine-id`.
  Hashing rather than direct byte interpretation makes the derivation
  format-agnostic: it works identically whether the file contains a plain
  32-hex-character string, a UUID with dashes, or any other format.

### InstanceId encoding

Every resource instance's IPv6 address is derived from its `ResourceKind` and
`InstanceId` (UUID, 128 bits) using a single encoding applied uniformly across
all resource types:

```
Subnet ID (16 bits) = kind_byte (8 bits) || uuid[0] (8 bits)
Interface ID (64 bits) = uuid[1..9] (64 bits)
```

Where `kind_byte` is the `ResourceKind` enum discriminant (0–255; the current
10 kinds fit easily). This uses 9 bytes of the 16-byte UUID; the remaining 7
bytes are discarded. Collision probability within a kind is negligible (1 in
2^72).

Full address: `fd5e:edXX:XXXX:KKUU:UUUU:UUUU:UUUU:UUUU/128`
- `XX:XXXX` = per-node (24 bits)
- `KK` = kind byte
- `UU:UUUU:UUUU:UUUU:UUUU` = uuid[0..9] (72 bits)

Because services have their own `InstanceId`s (they are `ResourceInstance`s like
any other resource) and different kinds produce different `KK` bytes, addresses
are collision-free across all resource types with a single derivation function.

### Pod network prefixes

Each Deployment or Job instance gets its own IPv6 `/64` pod network. The prefix
is derived from the pod's `InstanceId` using the same encoding, truncated to
`/64`:

```
fd5e:edXX:XXXX:KKUU::/64
```

Containers on the pod network receive SLAAC addresses within this `/64`.

The host bridge for the pod network is assigned two addresses:
- `<pod-prefix>::1` — the gateway (default router for containers)
- `<pod-prefix>::2` — the **mount endpoint** (see Service mounts below)

### Service IPs

A `Service` (or `HttpService`) resource has its own `InstanceId` and therefore
a unique `/128` derived address in the ULA space. This is the service's stable
virtual IP. It does not change when the backing pod instances are replaced.

The DataPlane installs ECMP routes on the host routing table mapping each
service IP to the IPv6 addresses of its currently running backing pod instances.
When a pod instance starts or stops, the routes are updated.

---

## Backend traits

Async fn in traits requires `Send`-bounded futures for tokio-mt. Use the
`trait-variant` crate to generate a `Send`-bounded companion trait, or
`async_trait` — decide at implementation time.

### `ContainerRuntime`

Covers observation, image management, network/volume management, forced cleanup,
and interactive exec. Container lifecycle (create/start/stop) is intentionally
absent: that responsibility belongs to `ProcessManager` via transient units.

```rust
pub trait ContainerRuntime: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    // Observation
    async fn inspect(&self, name: &str)
        -> Result<Option<ContainerState>, Self::Error>;
    async fn list(&self, filter: ContainerFilter<'_>)
        -> Result<Vec<ContainerSummary>, Self::Error>;

    // Images
    async fn image_exists(&self, reference: &str) -> Result<bool, Self::Error>;
    async fn pull_image(&self, reference: &str)   -> Result<(), Self::Error>;

    // Networks — one IPv6 /64 per pod instance.
    // The host bridge is assigned ::1 (gateway) and ::2 (mount endpoint).
    async fn network_exists(&self, name: &str)    -> Result<bool, Self::Error>;
    async fn create_network(&self, name: &str, prefix: Ipv6Net)
        -> Result<(), Self::Error>;
    async fn remove_network(&self, name: &str)    -> Result<(), Self::Error>;

    // Volumes
    async fn volume_exists(&self, name: &str)     -> Result<bool, Self::Error>;
    async fn create_volume(&self, name: &str)     -> Result<(), Self::Error>;
    async fn remove_volume(&self, name: &str)     -> Result<(), Self::Error>;

    // Forced cleanup (e.g. seedling crashed while container was running)
    async fn remove_container(&self, name: &str, force: bool)
        -> Result<(), Self::Error>;

    // Interactive exec (for BSL shell sessions)
    async fn exec(&self, name: &str, spec: ExecSpec)
        -> Result<ExecHandle, Self::Error>;
}
```

### `ProcessManager`

Container lifecycle goes through **transient** systemd units. Each container is
started as a transient `.service` unit whose `ExecStart` is `podman run [...]`.
Systemd owns supervision, restart policy, and journald logging.

Persistent units (`.socket` files, if ever needed for BSL-level socket
activation) are also managed here; they are the only units written to disk.
Persistent socket units are not used for external ingress ports — that
responsibility belongs to `DataPlane`.

```rust
pub trait ProcessManager: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    // Transient units — container lifecycle; no unit file written to disk.
    async fn start_transient(&self, spec: TransientUnitSpec)
        -> Result<(), Self::Error>;
    /// Sends the stop signal; returns immediately without waiting.
    /// Use `wait_unit_stopped` to block until the unit has fully stopped.
    async fn stop_unit(&self, name: &str)         -> Result<(), Self::Error>;
    /// Polls until the unit reaches an inactive or failed state, or the
    /// timeout elapses. Required before removing pod networks or volumes.
    async fn wait_unit_stopped(&self, name: &str, timeout: Duration)
        -> Result<(), Self::Error>;
    async fn unit_state(&self, name: &str)
        -> Result<Option<UnitState>, Self::Error>;
    async fn list_units(&self, prefix: &str)
        -> Result<Vec<UnitSummary>, Self::Error>;

    // Persistent units — written to the unit drop-in path.
    async fn write_unit(&self, name: &str, content: &str)
        -> Result<(), Self::Error>;
    async fn remove_unit(&self, name: &str)       -> Result<(), Self::Error>;
    async fn daemon_reload(&self)                 -> Result<(), Self::Error>;
    async fn start_unit(&self, name: &str)        -> Result<(), Self::Error>;
}
```

### `NetworkProxy`

Responsible only for Caddy routing configuration and listener management.
Port forwarding and service routing are handled by `DataPlane`. `apply_config`
is full-replace and idempotent.

```rust
pub trait NetworkProxy: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn is_healthy(&self) -> Result<bool, Self::Error>;
    async fn apply_config(&self, config: &ProxyConfig) -> Result<(), Self::Error>;
}
```

### `DataPlane`

Owns all kernel-level networking: nftables rules (ingress DNAT, service mount
DNAT6, FORWARD policy) and the IPv6 routing table (service IP routes, ECMP).
All nftables rules live in a single `seedling_net` table. `apply_rules` replaces
the rule set atomically; `apply_routes` replaces routing table entries.

```rust
pub trait DataPlane: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Atomically replace the complete nftables rule set in `seedling_net`.
    /// Idempotent. Covers ingress DNAT, FORWARD policy, and mount DNAT6.
    async fn apply_rules(&self, rules: &DataPlaneRules)
        -> Result<(), Self::Error>;

    /// Replace the complete set of IPv6 service routes in the routing table.
    /// Each route maps a service IP to one or more pod instance IPs (ECMP).
    async fn apply_routes(&self, routes: &[ServiceRoute])
        -> Result<(), Self::Error>;

    /// Remove all rules and routes owned by seedling. Called on shutdown.
    async fn clear_all(&self) -> Result<(), Self::Error>;
}
```

### `SystemDriver`

```rust
pub struct SystemDriver<C, P, N, D> {
    pub container: C,
    pub process:   P,
    pub proxy:     N,
    pub data_plane: D,
}
```

---

## Shared types (`src/system/types.rs`)

### Container observation

```rust
pub struct ContainerState {
    pub status:      ContainerStatus,
    pub health:      ContainerHealth,
    pub pid:         Option<u32>,
    pub exit_code:   Option<i32>,
    pub started_at:  Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
    /// The container's IPv6 address on its pod network, if known.
    pub pod_addr:    Option<Ipv6Addr>,
}

pub enum ContainerStatus { Created, Running, Paused, Exited, Unknown }
pub enum ContainerHealth { None, Starting, Healthy, Unhealthy }

pub struct ContainerSummary {
    pub name:   String,
    pub status: ContainerStatus,
    pub labels: HashMap<String, String>,
}

pub struct ContainerFilter<'a> {
    pub label:       Option<(&'a str, &'a str)>,
    pub name_prefix: Option<&'a str>,
}
```

### Container spec

Intermediate representation produced by the `translate/` layer. `podman_args`
turns it into the `ExecStart` argv for the transient systemd unit.

```rust
pub struct ContainerSpec {
    pub name:       String,           // instance.display_name
    pub image:      String,
    pub command:    Vec<String>,
    pub entrypoint: Vec<String>,
    pub env:        Vec<(String, String)>,
    pub mounts:     Vec<Mount>,
    pub network:    String,           // pod network name
    pub labels:     HashMap<String, String>,
    pub health:     Option<HealthCheckSpec>,
    /// Entries injected into /etc/hosts inside the container.
    /// Used to map `localmount` to the pod's ::2 mount endpoint address.
    pub hosts:      Vec<(String, IpAddr)>,
}

pub struct Mount {
    pub source:    MountSource,
    pub target:    String,
    pub read_only: bool,
}
pub enum MountSource { Volume(String), Bind(PathBuf), Tmpfs }

pub struct HealthCheckSpec {
    pub command:      Vec<String>,
    pub interval:     Duration,
    pub timeout:      Duration,
    pub retries:      u32,
    pub start_period: Duration,
}
```

No `publish` field: external port exposure is handled entirely by `DataPlane`
via nftables DNAT. App containers and the Caddy container are never port-published.

### Exec spec and handle

```rust
pub struct ExecSpec {
    pub command: Vec<String>,
    pub env:     Vec<(String, String)>,
    pub tty:     bool,
    pub user:    Option<String>,
}

// Opaque handle; details depend on the shell session subsystem design.
pub struct ExecHandle { /* ... */ }
```

### Transient unit spec

```rust
pub struct TransientUnitSpec {
    // App container naming: "seedling-{instance.display_name}.service"
    // Caddy infrastructure: "seedling-caddy.service" / "seedling-caddy-next.service"
    pub name:        String,
    pub description: String,
    pub exec_start:  Vec<String>,   // full `podman run [...]` argv
    pub restart:     TransientRestart,
}

pub enum TransientRestart { No, OnFailure, Always }
```

Containers are started with `podman run --rm`. When systemd stops the unit,
podman exits and the container is removed.

### systemd unit observation

```rust
/// `unit_state` returns `None` when the unit does not exist or is masked.
pub struct UnitState   { pub active: ActiveState, pub sub: String }
pub struct UnitSummary { pub name: String, pub state: UnitState }

pub enum ActiveState { Active, Activating, Deactivating, Inactive, Failed }
```

### DataPlane types

```rust
/// The complete desired state for all nftables rules in `seedling_net`.
/// Applied atomically; replaces the previous rule set entirely.
pub struct DataPlaneRules {
    /// External ingress: host port → Caddy's IPv6 address.
    pub ingress: Vec<IngressRule>,
    /// Service mount DNAT6: per-pod localmount:port → service-ip:port.
    pub mounts:  Vec<MountRule>,
    // FORWARD policy: allow all traffic within the seedling ULA prefix
    // (fd5e:ed::/24) is implicit and does not require explicit entries.
}

/// Redirects an external host port to Caddy's container address.
/// Applied in the nftables prerouting chain.
pub struct IngressRule {
    pub external_port: u16,
    pub proto:         ForwardProto,  // Tcp | Udp | Both
    pub caddy_addr:    SocketAddr,    // Caddy's IPv6 addr:port
}

/// DNAT6 rule translating a service mount port to the canonical service port.
/// Scoped to traffic originating from a specific pod's /64 prefix destined
/// for that pod's ::2 mount endpoint address.
pub struct MountRule {
    pub pod_prefix:    Ipv6Net,   // the mounting pod's /64
    pub mount_addr:    Ipv6Addr,  // pod-prefix::2
    pub mount_port:    u16,       // port declared in .mount()
    pub service_ip:    Ipv6Addr,  // target service's IPv6 address
    pub service_port:  u16,       // canonical service port
    pub proto:         ForwardProto,
}

/// Maps a service IP to one or more backing pod instance addresses via ECMP.
/// Applied to the host routing table via rtnetlink.
pub struct ServiceRoute {
    pub service_ip: Ipv6Addr,
    pub backends:   Vec<Ipv6Addr>,  // pod instance addresses; empty = blackhole
}

pub enum ForwardProto {
    Tcp,
    Udp,
    /// Emits one TCP rule and one UDP rule.
    Both,
}
```

### Proxy config

Full-replacement document sent to Caddy's admin API. Upstreams reference
service IPv6 addresses; Caddy reaches them through the host routing table.
Caddy does not join pod networks.

```rust
pub struct ProxyConfig {
    /// Ports Caddy should bind inside its container.
    /// Caddy adds or removes listeners hot via the admin API.
    pub listeners:     Vec<ProxyListener>,
    pub virtual_hosts: Vec<VirtualHost>,
}

pub struct ProxyListener {
    pub port:  u16,
    pub proto: ProxyListenerProto,
}

/// Protocol type for Caddy listener configuration.
/// Distinct from `ForwardProto` (nftables); Caddy's listener model is
/// HTTP/HTTPS/QUIC, not raw TCP/UDP.
pub enum ProxyListenerProto {
    Http,   // plain HTTP
    Https,  // TLS termination
    Quic,   // HTTP/3 over QUIC (UDP); requires tls_acme or manual cert
}

pub struct VirtualHost {
    pub hostname: String,
    pub tls_acme: bool,
    /// If present, add an HTTP→HTTPS redirect server block.
    /// Preserves the BSL `redirect(port, code)` arguments.
    pub redirect: Option<HttpRedirect>,
    pub routes:   Vec<ProxyRoute>,
}

pub struct HttpRedirect {
    pub from_port: u16,
    pub code:      u16,
}

pub struct ProxyRoute {
    pub prefix:    String,
    /// One upstream per scale instance: "http://[fd5e:ed...]:3000".
    /// ECMP routing at the kernel distributes connections across instances.
    pub upstreams: Vec<String>,
}
```

### Observation facts

The bridge between the system layer and the runtime history. `Observer` produces
these; the reconciler loop persists them to `world_observations`.

Each call to `observe` is scoped to one `ResourceInstance`, so facts are
subject-identified by the call. The reconciler maintains the pairing.

```rust
pub enum ObservationFact {
    // Container
    ContainerMissing,
    ContainerCreated,
    ContainerRunning  { pid: u32 },
    ContainerExited   { exit_code: i32 },
    ContainerHealthy,
    ContainerUnhealthy,

    // Network
    NetworkPresent,
    NetworkMissing,

    // Volume
    VolumePresent,
    VolumeMissing,

    // Systemd unit
    UnitActive,
    UnitInactive,
    UnitFailed,

    // Proxy
    ProxyReachable,
    ProxyUnreachable,
    RoutePresent { hostname: String },
    RouteAbsent  { hostname: String },
}
```

---

## Observer and Actuator

### Error types

Per-backend error enums (`PodmanError`, `SystemdError`, `CaddyError`,
`DataPlaneError`) are full snafu enums internal to each backend module and not
re-exported from `src/system/`. The boundary error types are also snafu enums,
but use `Box<dyn Error + Send + Sync + 'static>` as the source type to
intentionally erase the backend variant — callers see `ObserveError::Container`
but cannot match on `PodmanError` internals. This is opacity by design.

```rust
#[derive(Debug, Snafu)]
pub enum ObserveError {
    #[snafu(display("container backend: {source}"))]
    Container  { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("process manager: {source}"))]
    Process    { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("proxy: {source}"))]
    Proxy      { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("data plane: {source}"))]
    DataPlane  { source: Box<dyn std::error::Error + Send + Sync + 'static> },
}

#[derive(Debug, Snafu)]
pub enum ActuateError {
    #[snafu(display("container backend: {source}"))]
    Container  { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("process manager: {source}"))]
    Process    { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("proxy: {source}"))]
    Proxy      { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("data plane: {source}"))]
    DataPlane  { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("image {reference} not found and pull failed"))]
    ImageUnavailable { reference: String },
    #[snafu(display("resource kind {kind:?} is not supported by this actuator"))]
    UnsupportedKind  { kind: ResourceKind },
}
```

### `Observer`

```rust
pub struct Observer<C, P, N, D> {
    driver: SystemDriver<C, P, N, D>,
}

impl<C, P, N, D> Observer<C, P, N, D>
where
    C: ContainerRuntime,
    P: ProcessManager,
    N: NetworkProxy,
    D: DataPlane,
{
    /// Inspect all system primitives backing one resource instance.
    /// Returns timestamped facts; the reconciler loop persists them.
    pub async fn observe(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Result<Vec<(ObservationFact, SystemTime)>, ObserveError>;
}
```

### `Actuator`

```rust
pub struct Actuator<C, P, N, D> {
    driver: SystemDriver<C, P, N, D>,
}

impl<C, P, N, D> Actuator<C, P, N, D>
where
    C: ContainerRuntime,
    P: ProcessManager,
    N: NetworkProxy,
    D: DataPlane,
{
    /// Ensure all primitives for this instance exist and are running.
    pub async fn start(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Result<(), ActuateError>;

    /// Stop and remove all primitives for this instance.
    pub async fn stop(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Result<(), ActuateError>;

    /// In-place update (e.g. rolling a container to a new image or config).
    pub async fn update(
        &self,
        instance: &ResourceInstance,
        old: &Resource,
        new: &Resource,
    ) -> Result<(), ActuateError>;
}
```

---

## Network topology

### Pod networks

Each Deployment or Job *instance* gets exactly one IPv6 pod network. The
network's `/64` prefix is derived from the pod `InstanceId` using the ULA
encoding described above. No shared app-level or service-level network exists.

The host bridge for each pod network holds:
- `<pod-prefix>::1` — gateway; the container's default router
- `<pod-prefix>::2` — mount endpoint; where service mounts are accessed

Containers receive SLAAC addresses within the `/64`. Their IPv6 address on the
pod network is discovered by inspecting the container after startup.

### Service IPs and routing

`Service` and `HttpService` resources each have a stable `/128` IPv6 address
derived from their `InstanceId`. This address does not change when backing pod
instances are replaced.

The `DataPlane` maintains routing table entries (via rtnetlink) mapping each
service IP to the IPv6 addresses of its currently running pod instances. When
a pod starts or stops, `apply_routes` is called with the updated backend set.
ECMP distributes new connections across multiple backends for scale > 1.

`ExternalService` resources create no system primitives.

### Service mounts and the `localmount` endpoint

When a pod mounts a service at a declared port (e.g. `.mount(svc.port(4000))`),
the BSL contract is that the container can connect to `localmount:4000` and
reach the service, with the runtime handling all routing transparently.

Mechanism:

1. The container's `/etc/hosts` is injected with:
   `<pod-prefix>::2 localmount`
   The `::2` address lives on the host side of the pod bridge, so traffic to it
   naturally exits the container via the veth pair and reaches the host.

2. The `DataPlane` installs a `MountRule` on the host:
   traffic from `<pod-prefix>/64` to `<pod-prefix>::2:4000` is DNAT6'd to
   `<service-ip>:3000` (the canonical service port).

3. The DataPlane's ECMP routes then deliver the packet to a backing pod instance.

Port conflicts across pods are impossible: each pod's `::2` is a distinct IPv6
address derived from its unique `/64` prefix. Pod A and pod B can both mount
different services on port 4000 — the DNAT6 rules are keyed on different
destination addresses.

When a pod stops, its `MountRule`s are removed from the `DataPlane` state.
If the service has no backing pods, traffic reaches the service IP but finds no
route — connection refused, which is correct behaviour.

### Ingress and external traffic

Caddy handles all external traffic. It listens dual-stack (IPv4 and IPv6) on
the ingress ports declared by BSL `Ingress` resources. The `DataPlane` installs
`IngressRule`s mapping external host ports to Caddy's IPv6 container address
via nftables DNAT in the prerouting chain.

Caddy's upstreams are service IPv6 addresses (`http://[fd5e:ed...]:3000`).
Caddy reaches them through the host routing table — the same ECMP routes used
for pod-to-pod traffic. Caddy **does not join pod networks**. The admin API
exposure risk discussed earlier is eliminated structurally.

FORWARD policy: a single nftables rule allows all traffic where both source and
destination are within the seedling ULA prefix (`fd5e:ed::/24`). This covers
pod-to-service, Caddy-to-service, and mount endpoint traffic uniformly.

Note: nftables `prerouting` DNAT applies to traffic arriving from outside the
host. Traffic originating on the host itself (seedling health checks, monitoring,
etc.) bypasses `prerouting` and requires either a matching `output` chain DNAT
rule or `net.ipv4.conf.all.route_localnet=1` — see open questions.

---

## BSL resource → system primitives

| BSL resource      | Container | Pod network | Volume | Transient unit | Service IP | DNAT rule |
|-------------------|:---------:|:-----------:|:------:|:--------------:|:----------:|:---------:|
| `Deployment`      | N (scale) |      N      |        |       N        |            |           |
| `Job`             |     1     |      1      |        |       1        |            |           |
| `Volume`          |           |             |   1    |                |            |           |
| `ExternalVolume`  |           |             | claim  |                |            |           |
| `Service`         |           |             |        |                |     1      |           |
| `HttpService`     |           |             |        |                |     1      |           |
| `Ingress`         |           |             |        |                |            |    1+     |
| `ExternalService` |           |             |        |                |            |           |

An `Ingress` produces one or more `IngressRule`s (one per protocol on the
ingress port) and a virtual host in Caddy's config. The backing service's IP
is used as Caddy's upstream.

Volumes declared on a `Deployment` pod are created alongside the first instance
and shared across all instances of that deployment.

---

## Translate layer (`src/system/translate/`)

Pure functions; no async, no I/O.

```rust
// container.rs

pub fn deployment_spec(
    def:      &DeploymentDef,
    instance: &ResourceInstance,
    params:   &BTreeMap<String, String>,
    /// The pod network name and its IPv6 /64 prefix (derived from InstanceId).
    network:  &(String, Ipv6Net),
    /// Service mounts: (mount_port, service_ip, canonical_port).
    mounts:   &[(u16, Ipv6Addr, u16)],
) -> ContainerSpec;

pub fn job_spec(
    def:      &JobDef,
    instance: &ResourceInstance,
    params:   &BTreeMap<String, String>,
    network:  &(String, Ipv6Net),
    mounts:   &[(u16, Ipv6Addr, u16)],
) -> ContainerSpec;

/// Produces the `podman run [...]` argv from a ContainerSpec.
/// This is the ExecStart value for the transient systemd unit.
pub fn podman_args(spec: &ContainerSpec) -> Vec<String>;

// proxy.rs

/// A resolved upstream for one service: the service's IPv6 address
/// and the internal port Caddy should send traffic to.
pub struct ServiceUpstream {
    pub service_ip:   Ipv6Addr,
    pub service_port: u16,
}

/// Derives the IPv6 address for a resource instance.
/// Applies the ULA encoding: node_prefix /48 + kind_byte + uuid bytes.
pub fn instance_ipv6(node_prefix: &Ipv6Net, instance: &ResourceInstance)
    -> Ipv6Addr;

/// Derives the pod network /64 prefix for a pod instance.
pub fn pod_network_prefix(node_prefix: &Ipv6Net, instance: &ResourceInstance)
    -> Ipv6Net;

/// Builds the full ProxyConfig from the current set of active ingresses
/// and their resolved service upstreams.
///
/// The Ingress → Service → Deployment resolution (finding which service
/// backs an ingress and which pod instances back that service) is performed
/// by the caller; this function receives already-resolved data.
pub fn build_proxy_config(
    ingresses: &[(IngressDef, ServiceUpstream)],
    caddy_addr: SocketAddr,
) -> ProxyConfig;
```

---

## Podman libpod API (`src/system/podman.rs`)

- Crate: `podman-rest-client` (v0.13+), with features `uds` only (no `ssh`).
- Socket: `/run/podman/podman.sock` (rootful); use `Config::guess()` or supply
  the path explicitly.
- API version: pinned to v5 via `client.v5()`. No runtime negotiation.
- The crate exposes libpod-specific methods (e.g. `container_inspect_libpod`,
  `network_connect_libpod`) rather than the Docker-compat equivalents.
- Authentication: none (unix socket, permissions enforced by the OS).

`PodmanRuntime` wraps a `PodmanRestClient` and translates between the crate's
generated request/response types and the `ContainerRuntime` trait types in
`src/system/types.rs`. This translation layer is intentionally thin; the trait
types exist so the rest of the codebase never imports `podman-rest-client`
types directly.

`create_network` passes the IPv6 `/64` prefix to podman when creating the
network, and adds `::1` and `::2` to the resulting bridge interface. Podman's
`--ipv6` flag and the `subnets` field in the network creation request carry
the prefix.

---

## systemd manager (`src/system/systemd.rs`)

- D-Bus: system bus via `zbus` (async, pairs with tokio).
- One `zbus::Connection` is created at startup and held for the process
  lifetime. `zbus` async connections are `Send + Sync` and safe to share across
  tasks via `Arc` or by storing directly in `SystemdManager`.
- Interface: `org.freedesktop.systemd1.Manager` on `/org/freedesktop/systemd1`.
- Transient units are created via `StartTransientUnit`.
- Persistent unit files (if needed) go to `/etc/systemd/system/` (rootful).
- Unit naming convention for app containers: `seedling-{display_name}.service`.
  Since display names are already hyphen-separated components (e.g.
  `myapp-web-abc123`), all app units are enumerable with `list_units("seedling-")`.
- Caddy's units are named `seedling-caddy.service` and
  `seedling-caddy-next.service` (during blue/green upgrade). These are fixed
  infrastructure names, not derived from the display-name convention.

`start_transient` maps `TransientUnitSpec` to a `StartTransientUnit` D-Bus call
with at minimum:
- `Description`
- `ExecStart` (the `podman run [...]` argv)
- `Restart` property
- `StandardOutput=journal`, `StandardError=journal`

systemd has no role in external port binding or service routing. Its
responsibilities are:
1. Lifecycle management of the seedling daemon itself (persistent service unit).
2. Transient unit supervision of app containers and the Caddy container.
3. Persistent socket units if BSL-level socket activation is ever needed.

---

## Caddy proxy (`src/system/caddy.rs`)

- Caddy is managed **out of band**: it is not tracked in `resource_instances`
  and does not go through the normal `Actuator` start/stop path. Seedling
  manages Caddy's container and transient unit directly at startup as
  infrastructure, distinct from user-declared BSL resources.
- Caddy does **not** join pod networks. It reaches pod containers via their
  service IPv6 addresses through the host routing table, using the same ECMP
  routes installed by `DataPlane` for pod-to-pod traffic.
- Caddy listens **dual-stack** externally: `0.0.0.0` and `[::]` on each
  declared ingress port, so both IPv4 and IPv6 external clients are served.
- Caddy's upstreams are service IPv6 addresses
  (e.g. `http://[fd5e:ed...]:3000`). The DataPlane's ECMP routes handle
  distribution across scale instances.
- Caddy is attached to a stable `seedling-proxy` network (IPv6-only). Its IP
  on that network is **dynamic**: podman assigns it at container creation time
  and seedling discovers it by inspecting the container.
- `CaddyProxy` holds the current admin API address in an
  `Arc<RwLock<SocketAddr>>` updated on every Caddy container change.
- The admin API is accessed at `http://[<current-caddy-ip>]:2019`. It is only
  reachable on the `seedling-proxy` network, not from pod networks and not
  from outside the host.
- Caddy requires a persistent named volume (`seedling-caddy-data`) mounted at
  `/data` inside the container. This stores ACME account keys and certificate
  cache. Without it, every Caddy restart triggers fresh ACME challenges and
  will hit Let's Encrypt rate limits in production.

`apply_config` sends the full config document to `POST /config/` using Caddy's
JSON config API. Caddy applies it atomically with no traffic drop.

### Version management and upgrades

Caddy's image reference (e.g. `docker.io/library/caddy:2.9`) is part of
seedling's own configuration, not the BSL script. It is versioned and
distributed alongside seedling itself.

Upgrades use a **blue/green strategy**: the new container is fully prepared and
configured before traffic is cut over, using an atomic `DataPlane` rule
replacement as the cutover mechanism.

**Upgrade sequence:**

1. Pull the new image while the old Caddy container continues serving traffic.
2. Start the new Caddy container (`seedling-caddy-next`) on `seedling-proxy`;
   podman assigns it an IP.
3. Inspect the new container to discover its IPv6 address on `seedling-proxy`.
4. Poll `is_healthy` (with retries and a timeout) — Caddy needs time to
   initialise before its admin API accepts connections.
5. Apply the full `ProxyConfig` to the new container via its admin API.
   (The new container is not yet receiving external traffic.)
6. Atomically replace the `IngressRule` set in `DataPlane` so all rules point
   to the new container's address. New connections are now routed to the new
   container. The kernel's conntrack table preserves established connections to
   the old container, allowing them to drain naturally.
7. Persist the active Caddy container name (`seedling-caddy-next`) to the DB.
   This is the crash-recovery oracle.
8. Update `CaddyProxy`'s internal `SocketAddr` to the new container's admin
   API address. Subsequent config updates go to the new container.
9. Stop the old transient unit (`stop_unit` + `wait_unit_stopped` with
   timeout; force-stop on timeout). The old container is removed by `--rm`.
10. Record `seedling-caddy` as the new canonical active container name in the
    DB.

**Startup reconciliation:**

At startup, seedling runs a Caddy reconciliation pass before entering the main
loop:

1. Inspect the running Caddy container (if any) and read its image digest.
2. Compare against the configured digest.
3. If they match and Caddy is healthy: discover its current IP, update
   `CaddyProxy`, apply the current `ProxyConfig`, and proceed.
4. If they differ or Caddy is absent/unhealthy: run the upgrade sequence above.
   If no old container exists, steps 6 and 9 are skipped.

This same path handles crash recovery (see below).

**Crash mid-Caddy-upgrade:**

The DB is the oracle for which Caddy container is active. The upgrade sequence
writes the active container name at step 7 (after DataPlane cutover) and step
10 (after cleanup). On startup, seedling reads the DB to determine which
container was last active.

If both `seedling-caddy` and `seedling-caddy-next` exist:
- DB says `seedling-caddy` is active → crash before step 7 (cutover not yet
  done). Stop and remove `seedling-caddy-next`; proceed with the recorded
  active container.
- DB says `seedling-caddy-next` is active → crash between steps 7 and 10
  (cutover done, old not yet cleaned). Stop and remove `seedling-caddy`;
  initialise `CaddyProxy` from `seedling-caddy-next`.

If only `seedling-caddy-next` exists and DB says it is active: old container
was already cleaned up. Initialise `CaddyProxy` from `seedling-caddy-next`.

In all cases no container rename is needed — the DB name is the authority.

---

## DataPlane (`src/system/data_plane.rs`)

`NftablesDataPlane` implements `DataPlane` using the `nftables` crate (v0.6+,
`tokio` feature) for nftables management and rtnetlink for IPv6 routing table
manipulation. Seedling calls the crate's typed Rust API; the crate internally
drives the `nft` binary in JSON mode (`nft -j`). The `nft` binary must be
present on the host — this is a runtime dependency of the crate, not of
seedling's code directly.

### nftables table structure

All rules live in a single table: `table inet seedling_net {}`.

**`prerouting` chain** (type nat, hook prerouting, priority dstnat):
- `IngressRule`s: DNAT external IPv4/IPv6 traffic on ingress ports to Caddy's
  IPv6 address.
- `MountRule`s: DNAT6 traffic from each pod's `/64` destined for that pod's
  `::2:mount_port` to the target service IP and canonical port.

**`forward` chain** (type filter, hook forward, priority filter):
- Single rule: allow all traffic where both source and destination are within
  `fd5e:ed::/24` (the seedling ULA prefix). This covers all pod-to-service and
  Caddy-to-service routing without per-pod rules.

`apply_rules` flushes the table and rewrites all chains in a single atomic
`nft` transaction. Idempotent: applying the same state twice is safe.

### Routing table

`apply_routes` manages IPv6 host routes (via rtnetlink) for service IPs:

- Each `ServiceRoute` with one backend → a `/128` host route to that backend
  via the appropriate pod network bridge.
- Each `ServiceRoute` with multiple backends → ECMP routes (equal-weight
  multipath) to all backends. The kernel distributes new connections per-flow
  using a consistent hash, so a given TCP connection always reaches the same
  backend.
- An empty `backends` list → a blackhole route (service exists but has no
  running instances; connections fail fast rather than timing out).

Example nftables ruleset for two ingress ports and one service mount:

```
table inet seedling_net {
    chain prerouting {
        type nat hook prerouting priority dstnat; policy accept;
        # Ingress
        tcp dport 80  dnat to [fd5e:ed12:3456:ff01::2]:80
        udp dport 80  dnat to [fd5e:ed12:3456:ff01::2]:80
        tcp dport 443 dnat to [fd5e:ed12:3456:ff01::2]:443
        udp dport 443 dnat to [fd5e:ed12:3456:ff01::2]:443
        # Mount: pod A's port 4000 → svc1:3000
        ip6 saddr fd5e:ed12:3456:0a00::/64 \
            ip6 daddr fd5e:ed12:3456:0a00::2 \
            tcp dport 4000 \
            dnat to [fd5e:ed12:3456:0200:aabb:ccdd:eeff:1122]:3000
    }
    chain forward {
        type filter hook forward priority filter; policy accept;
        ip6 saddr fd5e:ed::/24 ip6 daddr fd5e:ed::/24 accept
    }
}
```

Note: UDP DNAT at port 443 is required for QUIC (HTTP/3) when Caddy's `quic`
ingress option is set.

---

## Seedling restart and upgrade

Seedling restarts (clean or crash, including mid-upgrade binary replacement) are
largely transparent because the critical design decisions already compose well:

| Concern                     | Survives restart? | Reason                                               |
|-----------------------------|:-----------------:|------------------------------------------------------|
| nftables rules              | yes               | kernel-owned; process lifecycle irrelevant           |
| IPv6 routing table          | yes               | kernel-owned                                         |
| App containers              | yes               | systemd transient units; not tied to seedling's PID  |
| Caddy container             | yes               | same — another transient unit                        |
| Caddy routing config        | yes               | lives in Caddy's process memory; Caddy keeps running |
| Pod networks                | yes               | podman networks persist independently                |
| Volumes                     | yes               | persistent by definition                             |
| DB state / operation replay | yes               | already persisted; existing replay infrastructure    |

**What seedling re-establishes on startup:**

The only in-memory state that must be reconstructed is `CaddyProxy`'s current
`SocketAddr`. Seedling inspects running containers for the active Caddy instance,
reads its IP on `seedling-proxy`, and re-initialises `CaddyProxy`. Everything
else is handled by the reconciliation loop's first tick, which observes actual
system state and converges from there.

**DB schema migrations:**

If a new version of seedling requires a schema change, migrations run before
the reconciliation loop starts. No special handling beyond what the existing
migration infrastructure provides.

---

## Reconciliation loop

The reconciler runs as a background task, ticking at a fixed interval (5 s).
Each tick executes a sequence of phases. All phases operate on a
point-in-time snapshot of the desired state computed at the start of the tick.
The complete DataPlane and proxy state is recomputed and reapplied from scratch
on every tick — it is never accumulated across ticks, so the loop is
inherently idempotent.

### File structure

Each phase lives in its own file, with `src/system/reconcile.rs` as the
coordinating top module (using the `foo.rs` / `foo/sub.rs` convention):

```
src/system/reconcile.rs                — Reconciler struct, tick() coordination,
                                         node_prefix_from_machine_id()
src/system/reconcile/phase2_pods.rs   — Observe + actuate Deployments/Jobs
src/system/reconcile/phase3_volumes.rs — Observe + actuate Volumes
src/system/reconcile/phase4_routes.rs  — Service route computation
src/system/reconcile/phase5_rules.rs   — DataPlane nftables rules
src/system/reconcile/phase6_proxy.rs   — Proxy config (Caddy)
```

Phase 1 (desired state snapshot) is a single `compute()` call and lives
directly in `reconcile.rs`. Phase 7 (bridge `::2` check) is deferred pending
the bridge name persistence prerequisite described below.

### Phase 1 — Desired state snapshot

Under a brief lock on the App, compute the desired state:

- **Steady state** (no active operation): every resource in the AppDef is
  desired at `Ready`. The `compute(app_name, &def, None)` path in
  `runtime/desired.rs` handles this.
- **During an operation**: desired state comes from the action log — only
  resources the action closure has explicitly `rt.start()`'d or `rt.stop()`'d
  are included, at the state they were placed into.

The lock is dropped before any async work begins.

Scale is deferred: `compute_steady` currently produces exactly one instance
per resource via `get_or_create_singleton`. True N-instance scale handling
(N stable instance IDs per slot, starting/stopping to match declared scale)
is a separate work item.

### Phase 2 — Observe and actuate Deployments and Jobs

For each Deployment or Job instance in the desired state:

1. **Observe** via `Observer::observe`: pod network presence, container
   lifecycle state (missing / created / running / exited), systemd unit state.
2. **Decide**:
   - desired=`Ready` and container not running → call `Actuator::start`.
   - desired=`Unscheduled` and container running or unit active →
     call `Actuator::stop`.
   - Otherwise → no action this tick.
3. **Collect running pods**: if the container is currently running (from the
   observation, before any actuation this tick), call `container.inspect()` to
   obtain its IPv6 address on the pod network. Build a `running_pods` list of
   `(instance, pod_prefix, pod_ip)` for use in Phases 4 and 5.

Running pod IPs are intentionally collected from the pre-actuation observation.
A container started during this tick will not yet have a SLAAC address assigned
and will appear in routes only on the next tick. This one-tick lag is expected
behaviour and must be documented with a comment at the collection site in the
code.

Errors for individual instances are logged and skipped; they do not abort
the tick or affect other instances.

### Phase 3 — Observe and actuate Volumes

For each Volume instance in the desired state:

1. **Observe**: does the named volume exist?
2. **Decide**: desired=`Ready` and missing → create; desired=`Unscheduled`
   and present → remove.

`ExternalVolume` and `ExternalService` are no-ops; skip them entirely.

### Phase 4 — DataPlane: service routes

For each `Service` and `HttpService` resource in the AppDef snapshot:

1. Derive the service's stable `/128` IPv6 address from the node prefix and
   the service's persisted instance ID (via `instance_ipv6`).
2. Scan `running_pods`: collect the pod IPv6 addresses of every running pod
   instance whose definition has a `tcp_binding`, `udp_binding`, or
   `http_binding` pointing to this service name.
3. If there is at least one backend, emit a `ServiceRoute { service_ip,
   backends }`.

Call `data_plane.apply_routes(&routes)` with the complete replacement set.
`ExternalService` resources are skipped — they represent services outside
seedling's control and produce no routes.

### Phase 5 — DataPlane: nftables rules

Build `DataPlaneRules { ingress, mounts }`:

**IngressRules** — one per `Ingress` resource in the AppDef snapshot:

- `external_port`: the ingress's declared listen port.
- `caddy_addr`: Caddy's IPv6 address on the `seedling-proxy` network,
  paired with the same port (Caddy listens on the same port number internally).
- `proto`: `Tcp` normally; `Both` if the ingress has `.dtls()` or `.quic()`.
- If the ingress has a redirect configured (e.g. HTTP→HTTPS), add a second
  `IngressRule` for the redirect source port using `proto: Tcp`.

**MountRules** — for each running pod that declares service mounts:

For each `ServicePort` in the pod's `service_mounts` list:

- `pod_prefix`: the pod's `/64` network prefix (derived from node prefix +
  instance ID).
- `mount_addr`: `pod_prefix::2` — the bridge's mount endpoint address.
- `mount_port`: `service_port.port`.
- `service_ip`: the target service's stable IPv6 address.
- `service_port`: `service_port.port`.
- `proto`: `Tcp` (UDP service mounts are not yet supported).

Call `data_plane.apply_rules(&rules)`.

### Phase 6 — Proxy config (Caddy)

For each `Ingress` resource in the AppDef snapshot:

1. Look up the ingress's target service name (`ingress.service.name`).
2. Derive the service's stable IPv6 address.
3. Find the upstream port: scan all pod definitions for the first
   `tcp_binding` or `http_binding` that references this service; use
   `pod_port` (the port the container actually listens on). Fall back to
   the ingress's declared port if no binding is found.
   With pure L3 ECMP routing there is no port translation between the service
   port and the pod port — packets arrive at the pod with the destination port
   unchanged. The code must assert `pod_port == service_port.port` with a TODO
   message noting that port translation is not yet supported.
4. Build a `ServiceUpstream { service_ip, service_port: upstream_port }`.

Pass all `(ingress_def, upstream)` pairs to `build_proxy_config()`, then
call `proxy.apply_config(&proxy_config)`.

### Phase 7 — Bridge `::2` address check (startup reconciliation)

For each pod instance whose network is known to exist, verify that
`pod_prefix::2` is assigned to the bridge interface. If the address is
absent (e.g. after a crash between `create_network` and the rtnetlink
assignment), re-add it via rtnetlink.

**This phase depends on a prerequisite that is not yet implemented** — see
"Bridge name in-memory map" below. Phase 7 must not be coded until that
prerequisite is in place.

### Prerequisite: bridge name in-memory map

When Podman creates a network, its API response includes the Linux bridge
interface name (the `network_interface` field in the libpod response). This
name is needed by Phase 7 to look up the bridge and check whether `::2` is
assigned. Bridge names are kept in a `HashMap<String, String>` (network name
→ bridge interface name) held directly on the `Reconciler`. No DB table is
needed.

Required changes:

1. **`ContainerRuntime::list_networks`** — add a new trait method:
   ```rust
   fn list_networks<'a>(&'a self, prefix: &'a str)
       -> BoxFuture<'a, Result<Vec<NetworkSummary>, BoxError>>;
   ```
   where `NetworkSummary` (a new type in `types.rs`) carries at minimum:
   ```rust
   pub struct NetworkSummary {
       pub name: String,
       pub bridge_name: String,
   }
   ```
   `PodmanRuntime` implements this by calling `network_list_libpod` filtered
   to names starting with `prefix`, extracting `network_interface` from each
   result.

2. **Startup population** — before entering the reconciliation loop, call
   `list_networks("seedling-")` and populate the `Reconciler`'s
   `bridge_names` map from the result. This recovers bridge names for any pod
   networks that survived a crash or restart.

3. **Map maintenance during normal operation** — `create_network` returns the
   bridge name (change return type from `()` to `String`); the reconciler
   inserts the entry into its map immediately after a successful
   `Actuator::start`. When `Actuator::stop` calls `remove_network`, the
   reconciler removes the corresponding entry.

4. **Phase 7 lookup** — Phase 7 reads bridge names directly from the
   `Reconciler`'s in-memory map.

### Error handling across phases

- An error in one phase does not skip later phases.
- Within a phase, an error for one resource is logged and skipped; the
  reconciler continues to the next resource.
- The Caddy `admin_addr` handle is read under an async read lock at the
  start of phases 5 and 6; if the address is not yet IPv6 (e.g. Caddy has
  not started), those phases are skipped for that tick and an error is logged.

---

## Implementation order

1. **`src/system/types.rs`** — all shared data types; no logic, no deps.
2. **`src/system/mod.rs`** — `SystemDriver` struct and re-exports.
3. **`src/system/translate/`** — pure conversion functions, testable without
   any backend. Start with `instance_ipv6`, `pod_network_prefix`, then
   `container.rs` → `podman_args`, then `proxy.rs`.
4. **`src/system/podman.rs`** — `PodmanRuntime`; stub all methods with
   `todo!()`, then implement incrementally starting with `inspect`, `list`,
   and `create_network` (with IPv6 prefix).
5. **`src/system/systemd.rs`** — `SystemdManager`; stub first, implement
   `start_transient` and `wait_unit_stopped` as the first two live methods.
6. **`src/system/caddy.rs`** — `CaddyProxy`; stub first, implement
   `apply_config` and `is_healthy`.
7. **`src/system/data_plane.rs`** — `NftablesDataPlane`; implement
   `apply_rules` (nftables) and `apply_routes` (rtnetlink) in isolation before
   wiring into the actuator.
8. **`src/system/observer.rs`** — `Observer`; implement `observe` per resource
   kind, starting with `Deployment`.
9. **`src/system/actuator.rs`** — `Actuator`; implement `start` and `stop` for
   `Deployment` and `Volume` first, then `Service`/`Ingress` (which coordinate
   DataPlane state updates).
10. Implement the reconciliation loop.
11. Implement the real main.rs.

---

## Open questions for implementation time

- **rtnetlink crate**: adding ECMP IPv6 routes requires rtnetlink. The `rtnetlink`
  crate (from the `netlink-packet-*` family) is the most complete option.
  Evaluate whether it supports ECMP multipath routes and IPv6 adequately before
  committing.

- **Localhost → ingress port access**: nftables `prerouting` DNAT does not
  apply to traffic originating on the host. Accessing ingress ports from
  localhost (seedling health checks, monitoring, etc.) requires either a
  matching `output` chain DNAT rule or `net.ipv4.conf.all.route_localnet=1`.
  Decide whether to add the output rule unconditionally or document the
  limitation.

- **ip_forward and IPv6 forwarding**: inter-pod routing requires
  `net.ipv4.ip_forward=1` and `net.ipv6.conf.all.forwarding=1`. Rootful podman
  typically sets these; confirm and document the assumption.

- **SLAAC vs. static addressing for containers**: pod containers receive IPv6
  addresses via SLAAC within their `/64`. The actuator discovers container
  addresses via `inspect`. Confirm podman's SLAAC behaviour with IPv6-only
  networks and whether `--ip6` can be used for deterministic assignment.

- **`::2` reservation on pod bridges**: ~~resolved~~. `create_network` assigns
  `<prefix>::2/64` to the bridge via rtnetlink immediately after the podman
  `network_create_libpod` call returns (the response includes
  `network_interface`, the bridge name). Linux supports multiple addresses per
  interface; netavark assigns `::1` and seedling independently assigns `::2` —
  no conflict. **Residual crash-recovery gap**: a crash between the podman API
  call succeeding and the rtnetlink `address().add()` call leaves a network
  that exists without `::2`. Because `network_exists` returns `true` for that
  network, `create_network` is not re-entered on the next startup and `::2`
  remains absent. The startup reconciliation pass (step 8/9) must check for
  `::2` on every known pod bridge — readable via `handle.address().get()`
  filtered by interface index — and assign it if missing, as part of repairing
  `NetworkPresent` state.

- **Client IP visibility**: with DNAT for ingress, Caddy sees the original
  client IP (conntrack handles reverse translation). Verify this holds with the
  dual-stack setup and document whether Caddy's `trusted_proxies` config needs
  adjustment.

- **Ingress → Service → pod resolution**: the caller of `build_proxy_config`
  must resolve `Ingress → Service → running pod instances → IPv6 addresses`.
  This traversal of the BSL resource graph needs a documented home in the
  actuator or a dedicated resolver utility.

- **NAT64**: pods have no outbound IPv4 internet access. This is a known
  limitation; NAT64 support is deferred to a future implementation.
