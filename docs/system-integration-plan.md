# System Integration Layer — Implementation Plan

## Overview

The system integration layer (`src/system/`) is the bridge between the runtime's
abstract resource model and actual system primitives: podman containers, systemd
units, a Caddy proxy, and kernel-level port forwarding.

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
  (`SystemDriver<C, P, N, F>`). No `dyn` trait objects for the backends.
- **Rootful podman** for the initial implementation. Rootless is a separate
  `ContainerRuntime` implementation chosen at startup, not a runtime flag.
- **tokio multi-threaded** async runtime throughout.
- **snafu** for typed errors. Per-backend error enums are internal; the system
  boundary exposes only `ObserveError` and `ActuateError`.
- **Pluggable by design.** The four backend traits are the extension points.
  Swapping any individual backend requires only a new struct implementing the
  corresponding trait.

---

## Concrete backends — first implementation

| Trait            | Struct               | Transport                                                     |
|------------------|----------------------|---------------------------------------------------------------|
| `ContainerRuntime` | `PodmanRuntime`    | libpod REST API over unix socket at `/run/podman/podman.sock`  |
| `ProcessManager`   | `SystemdManager`   | system D-Bus via `zbus`; transient + persistent unit control   |
| `NetworkProxy`     | `CaddyProxy`       | Caddy admin API; IP discovered by container inspection         |
| `PortForwarder`    | `NftablesForwarder`| nftables DNAT via `nft` binary in JSON mode (`nft -j`)        |

---

## Module layout

```
src/system/
    mod.rs            SystemDriver<C,P,N,F>; pub re-exports of boundary types
    types.rs          All shared data types
    observer.rs       Observer<C,P,N,F>
    actuator.rs       Actuator<C,P,N,F>
    translate/
        mod.rs
        container.rs  DeploymentDef/JobDef → ContainerSpec  (pure functions)
        proxy.rs      active ingresses → ProxyConfig + ForwardingRules (pure)
    podman.rs         PodmanRuntime: impl ContainerRuntime
    systemd.rs        SystemdManager: impl ProcessManager
    caddy.rs          CaddyProxy: impl NetworkProxy
    nftables.rs       NftablesForwarder: impl PortForwarder
```

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

    // Networks
    // One podman network is created per pod instance (see Network topology).
    // Caddy joins and leaves pod networks dynamically as pods come and go.
    async fn network_exists(&self, name: &str)    -> Result<bool, Self::Error>;
    async fn create_network(&self, name: &str)    -> Result<(), Self::Error>;
    async fn remove_network(&self, name: &str)    -> Result<(), Self::Error>;
    async fn list_networks(&self)                 -> Result<Vec<String>, Self::Error>;
    async fn connect_network(&self, container: &str, network: &str)
        -> Result<(), Self::Error>;
    async fn disconnect_network(&self, container: &str, network: &str)
        -> Result<(), Self::Error>;

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
responsibility belongs to `PortForwarder`.

```rust
pub trait ProcessManager: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    // Transient units — container lifecycle; no unit file written to disk.
    async fn start_transient(&self, spec: TransientUnitSpec)
        -> Result<(), Self::Error>;
    /// Sends the stop signal to the unit; returns immediately without waiting.
    /// Use `wait_unit_stopped` to block until the unit has fully stopped.
    async fn stop_unit(&self, name: &str)    -> Result<(), Self::Error>;
    /// Polls until the unit reaches an inactive or failed state, or the
    /// timeout elapses.  Required before removing pod networks or volumes.
    async fn wait_unit_stopped(&self, name: &str, timeout: Duration)
        -> Result<(), Self::Error>;
    async fn unit_state(&self, name: &str)
        -> Result<Option<UnitState>, Self::Error>;
    async fn list_units(&self, prefix: &str)
        -> Result<Vec<UnitSummary>, Self::Error>;

    // Persistent units — written to the unit drop-in path.
    async fn write_unit(&self, name: &str, content: &str)
        -> Result<(), Self::Error>;
    async fn remove_unit(&self, name: &str)  -> Result<(), Self::Error>;
    async fn daemon_reload(&self)            -> Result<(), Self::Error>;
    async fn start_unit(&self, name: &str)   -> Result<(), Self::Error>;
}
```

### `NetworkProxy`

Responsible only for Caddy routing configuration and listener management.
Port forwarding from external host ports to Caddy's container ports is handled
separately by `PortForwarder`. `apply_config` is full-replace and idempotent.

```rust
pub trait NetworkProxy: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn is_healthy(&self) -> Result<bool, Self::Error>;
    async fn apply_config(&self, config: &ProxyConfig) -> Result<(), Self::Error>;
}
```

### `PortForwarder`

Manages kernel-level DNAT rules that redirect external host ports to Caddy's
container address. Operates independently of Caddy's container lifecycle —
rules persist across Caddy restarts and are only changed when the ingress port
set changes. Supports TCP, UDP, or both per rule.

```rust
pub trait PortForwarder: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Replace the complete active ruleset atomically.
    /// Idempotent: applying the same ruleset twice is safe.
    /// Any rule not present in the new set is removed.
    async fn apply_rules(&self, rules: &[ForwardingRule])
        -> Result<(), Self::Error>;

    /// Remove all forwarding rules owned by seedling.  Called on shutdown.
    async fn clear_rules(&self) -> Result<(), Self::Error>;
}

pub struct ForwardingRule {
    pub external_port: u16,
    pub proto:         ForwardProto,
    /// Fixed IP and port of the Caddy container on the proxy network.
    pub destination:   SocketAddr,
}

pub enum ForwardProto {
    Tcp,
    Udp,
    /// Convenience: emits one TCP rule and one UDP rule.
    Both,
}
```

### `SystemDriver`

```rust
pub struct SystemDriver<C, P, N, F> {
    pub container: C,
    pub process:   P,
    pub proxy:     N,
    pub forwarder: F,
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

Intermediate representation produced by the `translate/` layer. Not passed
directly to any backend; instead, `podman_args(&spec) -> Vec<String>` produces
the `ExecStart` argv for the transient unit.

```rust
pub struct ContainerSpec {
    pub name:       String,           // instance.display_name
    pub image:      String,
    pub command:    Vec<String>,
    pub entrypoint: Vec<String>,
    pub env:        Vec<(String, String)>,
    pub mounts:     Vec<Mount>,
    pub networks:   Vec<String>,      // pod network name(s)
    pub labels:     HashMap<String, String>,
    pub health:     Option<HealthCheckSpec>,
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

Note: there is no `publish` field on `ContainerSpec`. External port exposure is
handled entirely by `PortForwarder` via nftables DNAT, not by container port
publishing. App containers are never published; Caddy's container is never
published either.

### Exec spec and handle

```rust
pub struct ExecSpec {
    pub command: Vec<String>,
    pub env:     Vec<(String, String)>,
    pub tty:     bool,
    pub user:    Option<String>,
}

// Opaque handle; details depend on how the shell session subsystem works.
pub struct ExecHandle { /* ... */ }
```

### Transient unit spec

```rust
pub struct TransientUnitSpec {
    // Naming convention: "seedling-{instance.display_name}.service"
    // display_name is already hyphen-separated component parts, so this
    // naturally keeps all seedling units under the "seedling-" prefix and
    // enumerable via list_units("seedling-").
    pub name:        String,
    pub description: String,
    pub exec_start:  Vec<String>,   // full `podman run [...]` argv
    pub restart:     TransientRestart,
}

pub enum TransientRestart { No, OnFailure, Always }
```

Containers are started with `podman run --rm`, consistent with how quadlets
work. When systemd stops the unit, podman exits and the container is removed.

### systemd unit observation

```rust
/// `unit_state` returns `None` when the unit does not exist or is masked.
pub struct UnitState   { pub active: ActiveState, pub sub: String }
pub struct UnitSummary { pub name: String, pub state: UnitState }

pub enum ActiveState { Active, Activating, Deactivating, Inactive, Failed }
```

### Proxy config

Full-replacement document sent to Caddy's admin API. The translate layer builds
this from all active `Ingress` resources and the current set of running pod
instances that back them.

Caddy is told which ports to bind *inside its container* via `listeners`. Caddy
supports adding new server listeners hot via the admin API, so changing the
listener set does not require a container restart in the common case.

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
    pub prefix:   String,
    pub upstreams: Vec<String>,  // one per scale instance e.g. "http://myapp-web-abc123:8080"
}
```

### Observation facts

The bridge between the system layer and the runtime history. `Observer` produces
these; the reconciler loop persists them to `world_observations`.

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
`NftablesError`) are full snafu enums internal to each backend module and not
re-exported from `src/system/`. The boundary error types are also snafu enums,
but use `Box<dyn Error + Send + Sync + 'static>` as the source type to
intentionally erase the backend variant — callers see `ObserveError::Container`
but cannot match on `PodmanError` internals. This is opacity by design, not an
accident of the error library choice.

```rust
#[derive(Debug, Snafu)]
pub enum ObserveError {
    #[snafu(display("container backend: {source}"))]
    Container { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("process manager: {source}"))]
    Process   { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("proxy: {source}"))]
    Proxy     { source: Box<dyn std::error::Error + Send + Sync + 'static> },
}

#[derive(Debug, Snafu)]
pub enum ActuateError {
    #[snafu(display("container backend: {source}"))]
    Container { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("process manager: {source}"))]
    Process   { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("proxy: {source}"))]
    Proxy     { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("port forwarder: {source}"))]
    Forwarder { source: Box<dyn std::error::Error + Send + Sync + 'static> },
    #[snafu(display("image {reference} not found and pull failed"))]
    ImageUnavailable { reference: String },
    #[snafu(display("resource kind {kind:?} is not supported by this actuator"))]
    UnsupportedKind { kind: ResourceKind },
}
```

### `Observer`

```rust
pub struct Observer<C, P, N, F> {
    driver: SystemDriver<C, P, N, F>,
}

impl<C, P, N, F> Observer<C, P, N, F>
where
    C: ContainerRuntime,
    P: ProcessManager,
    N: NetworkProxy,
    F: PortForwarder,
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
pub struct Actuator<C, P, N, F> {
    driver: SystemDriver<C, P, N, F>,
}

impl<C, P, N, F> Actuator<C, P, N, F>
where
    C: ContainerRuntime,
    P: ProcessManager,
    N: NetworkProxy,
    F: PortForwarder,
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

Each Deployment or Job *instance* gets exactly one podman network, named after
`instance.display_name`. No shared app-level or service-level network exists.

`Service` and `HttpService` resources are **proxy configuration entries**, not
podman network entities. A service that a pod exposes becomes an upstream in
Caddy's routing config. The upstream address is the pod container's address on
its pod network, at the port the container listens on internally.

`ExternalService` resources create no system primitives.

### Proxy network

A single stable network named `seedling-proxy` connects Caddy to seedling's
management plane. Caddy's IP on this network is **dynamic**: podman assigns it
at container creation time and seedling discovers it by inspecting the container.

`CaddyProxy` holds the current admin API address in an `Arc<RwLock<SocketAddr>>`
updated on every Caddy container change (startup, upgrade, crash recovery).
DNAT rules reference Caddy's current IP and are replaced atomically whenever
that IP changes — either during a blue/green upgrade cutover or after crash
recovery brings up a new container.

### Pod lifecycle and Caddy connectivity

- When a pod starts:
  1. Create the pod network (`{display_name}`).
  2. Start the container attached to that network.
  3. Connect the Caddy container to the pod network so it can reach the upstream.
  4. Apply updated `ProxyConfig` and `ForwardingRule`s.

- When a pod stops:
  1. Disconnect Caddy from the pod network.
  2. Apply updated `ProxyConfig` and `ForwardingRule`s (remove the upstream).
  3. Stop the container (`stop_unit`).
  4. Wait for the unit to reach an inactive state (`wait_unit_stopped`) before
     proceeding — podman will refuse to remove a network with active endpoints.
  5. Remove the pod network.

### External port forwarding

Seedling does not bind external ports. Instead, `PortForwarder` installs
nftables DNAT rules that redirect host-level traffic at the kernel before it
reaches any userspace socket. The rules point to the **currently active** Caddy
container's IP, which changes only during blue/green upgrades and crash recovery.

DNAT rules are replaced atomically (single `nft` transaction) in two scenarios:
a new ingress port is added or removed (rare), or the active Caddy IP changes
(upgrade or recovery). In steady state, rules are untouched across reconciliation
ticks.

Note: nftables `prerouting` DNAT applies only to traffic arriving from outside
the host. Traffic originating on the host itself (e.g. seedling's own health
checks on port 80) bypasses `prerouting` and requires either a matching `output`
chain DNAT rule or `net.ipv4.conf.all.route_localnet=1` to reach Caddy via the
ingress port. This is noted as an open question for implementation.
</thinking>

Caddy binds ports inside its container based on `ProxyConfig.listeners`. Caddy
supports adding and removing listener addresses hot via its admin API.

nftables DNAT is kernel-level forwarding — no per-packet userspace involvement.
TCP and UDP are both supported natively. A single `ForwardingRule` with
`ForwardProto::Both` emits two nftables rules covering both protocols.

---

## BSL resource → system primitives

| BSL resource      | Container | Pod network | Volume | Transient unit | Proxy config | DNAT rule |
|-------------------|:---------:|:-----------:|:------:|:--------------:|:------------:|:---------:|
| `Deployment`      | N (scale) |      N      |        |       N        |              |           |
| `Job`             |     1     |      1      |        |       1        |              |           |
| `Volume`          |           |             |   1    |                |              |           |
| `ExternalVolume`  |           |             | claim  |                |              |           |
| `Service`         |           |             |        |                |   upstream   |           |
| `HttpService`     |           |             |        |                |  upstream+   |           |
| `Ingress`         |           |             |        |                | virtual host |     1+    |
| `ExternalService` |           |             |        |                |              |           |

An `Ingress` resource produces one or more DNAT rules (one per protocol on the
ingress port) and a virtual host entry in Caddy's config. Caddy is connected to
the pod network of the backing deployment, not listed separately above.

Volumes declared on a `Deployment` pod are created alongside the first instance
and shared across all instances of that deployment.

---

## Translate layer (`src/system/translate/`)

Pure functions; no async, no I/O. Takes BSL definition types and instance
identity, returns system types.

```rust
// container.rs
pub fn deployment_spec(
    def: &DeploymentDef,
    instance: &ResourceInstance,
    params: &BTreeMap<String, String>,
) -> ContainerSpec;

pub fn job_spec(
    def: &JobDef,
    instance: &ResourceInstance,
    params: &BTreeMap<String, String>,
) -> ContainerSpec;

/// Produces the `podman run [...]` argv from a ContainerSpec.
/// This is the ExecStart value for the transient systemd unit.
pub fn podman_args(spec: &ContainerSpec) -> Vec<String>;

// proxy.rs

/// A resolved upstream: one running pod instance and its reachable address
/// on the pod's network (IP:port, as seen from Caddy after network connect).
/// The caller resolves instances → addresses via ContainerRuntime::inspect.
pub struct PodUpstream {
    pub instance: ResourceInstance,
    pub address:  SocketAddr,
}

/// Builds the full ProxyConfig and the corresponding ForwardingRules from the
/// current set of active ingresses, their resolved pod upstreams, and the
/// active Caddy container's IP (used as the DNAT destination).
///
/// One ingress may have multiple upstreams when the backing Deployment runs
/// at scale > 1; all instances appear as upstreams in the same ProxyRoute.
pub fn build_proxy_config(
    ingresses: &[(IngressDef, Vec<PodUpstream>)],
    caddy_ip:  IpAddr,
) -> (ProxyConfig, Vec<ForwardingRule>);
```

---

## Podman libpod API (`src/system/podman.rs`)

- Crate: `podman-rest-client` (v0.13+), with features `uds` only (no `ssh`).
- Socket: `/run/podman/podman.sock` (rootful); use `Config::guess()` or
  supply the path explicitly.
- API version: pinned to v5 via `client.v5()`. No runtime negotiation.
- The crate is generated from the official podman v5 swagger file and exposes
  libpod-specific methods (e.g. `container_inspect_libpod`,
  `network_connect_libpod`) rather than the Docker-compat equivalents.
- Authentication: none (unix socket, permissions enforced by the OS).

`PodmanRuntime` wraps a `PodmanRestClient` and translates between the crate's
generated request/response types and the `ContainerRuntime` trait types defined
in `src/system/types.rs`. This translation layer is intentionally thin; the
trait types exist so the rest of the codebase never imports
`podman-rest-client` types directly.

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
  `myapp-web-abc123`), all app units are naturally grouped under the
  `seedling-` prefix and enumerable with `list_units("seedling-")`.
- Caddy's units are named `seedling-caddy.service` and
  `seedling-caddy-next.service` (during blue/green upgrade). These are
  fixed infrastructure names, not derived from the display-name convention.

`start_transient` maps `TransientUnitSpec` to a `StartTransientUnit` D-Bus call
with at minimum:
- `Description`
- `ExecStart` (the `podman run [...]` argv)
- `Restart` property
- `StandardOutput=journal`, `StandardError=journal`

systemd has no role in external port binding. Its responsibilities are:
1. Lifecycle management of the seedling daemon itself (persistent service unit).
2. Transient unit supervision of app containers and the Caddy container.
3. Persistent socket units if BSL-level socket activation is ever needed.

---

## Caddy proxy (`src/system/caddy.rs`)

- Caddy is managed **out of band**: it is not tracked in `resource_instances`
  and does not go through the normal `Actuator` start/stop path. Seedling
  manages Caddy's container and transient unit directly at startup as
  infrastructure, distinct from user-declared BSL resources.
- Caddy is attached to the stable `seedling-proxy` network. Its IP on that
  network is **dynamic**: podman assigns it at container start time and seedling
  discovers it by inspecting the container. No fixed IP is pre-assigned.
- As pods start and stop, Caddy is also dynamically connected to and
  disconnected from each pod's network. The active pod network list is sourced
  from `ContainerRuntime::list_networks` filtered to the `seedling-` prefix,
  cross-referenced with running pod containers, at the time of the upgrade.
- Caddy listens on `::` (all interfaces) for the ports declared in
  `ProxyConfig.listeners`, making it reachable from every attached network.
- The admin API is accessed at `http://<current-caddy-ip>:2019`. `CaddyProxy`
  holds the current IP in an `Arc<RwLock<SocketAddr>>` that is updated
  whenever the active Caddy container changes. The admin API port is not
  exposed to the host; it is only reachable on the `seedling-proxy` network.
- Caddy requires a persistent named volume (e.g. `seedling-caddy-data`) mounted
  at `/data` inside the container. This stores ACME account keys and certificate
  cache. Without it, every Caddy restart triggers fresh ACME challenges and will
  hit Let's Encrypt rate limits in production.

Caddy's JSON config API supports adding new server blocks with new listener
addresses hot, without process restart. When an ingress is added on a new port,
`apply_config` sends the updated config (including the new listener), and Caddy
starts accepting connections on that port inside the container. The
corresponding DNAT rule is applied by `PortForwarder` in the same actuator
step.

`apply_config` sends the full config document to `POST /config/` using Caddy's
JSON config API. Caddy applies it atomically with no traffic drop.

### Version management and upgrades

Caddy's image reference (e.g. `docker.io/library/caddy:2.9`) is part of
seedling's own configuration, not the BSL script. It is versioned and
distributed alongside seedling itself — upgrading Caddy means upgrading
seedling's config or the seedling binary, not editing a BSL script.

Upgrades use a **blue/green strategy**: the new container is fully prepared and
configured before traffic is cut over, and the cutover is an atomic kernel-level
DNAT rule replacement with no gap for new connections.

**Upgrade sequence:**

1. Pull the new image while the old Caddy container continues serving traffic.
2. Start the new Caddy container on `seedling-proxy`; podman assigns it an IP.
3. Inspect the new container to discover its IP on `seedling-proxy`.
4. Poll `is_healthy` (with retries and a timeout) before proceeding — Caddy
   needs time to initialise before its admin API accepts connections.
5. Connect the new container to every currently active pod network.
   (Source of truth: `list_networks` filtered to pod networks, i.e. those
   not named `seedling-proxy`, cross-referenced with running containers.)
6. Apply the full `ProxyConfig` to the new container via its admin API.
   (The new container is reachable but not yet receiving external traffic.)
7. Atomically replace the DNAT ruleset in `seedling_ingress` so all rules
   point to the new container's IP. New connections are now routed to the new
   container. The kernel's conntrack table preserves established connections to
   the old container, allowing them to drain naturally.
8. Persist the active Caddy container name (`seedling-caddy-next` at this
   point) to the DB. This is the crash-recovery oracle.
9. Update `CaddyProxy`'s internal `SocketAddr` to the new container's admin
   API address. Subsequent config updates go to the new container.
10. Stop the old transient unit (`stop_unit` + `wait_unit_stopped` with
    timeout; force-stop on timeout). The old container is removed by `--rm`.
11. Record `seedling-caddy` as the canonical active name in the DB. The
    container may be renamed by stopping `seedling-caddy-next` and starting
    a fresh `seedling-caddy` on the next reconciliation, or left as-is until
    the next upgrade; the DB name is the authority, not the container name.

**Startup reconciliation:**

At startup, seedling runs a Caddy reconciliation pass before entering the main
loop:

1. Inspect the running Caddy container (if any) and read its image digest.
2. Compare against the configured digest.
3. If they match and Caddy is healthy: discover its current IP, update
   `CaddyProxy`, apply the current `ProxyConfig`, and proceed.
4. If they differ or Caddy is absent/unhealthy: run the upgrade sequence above.
   If no old container exists, steps 6 and 8 are skipped.

This same path handles crash recovery: if Caddy's container is found absent on
any reconciliation tick, seedling treats it as a missing-old-container upgrade
and re-converges via steps 2–5 of the upgrade sequence.

---

## Seedling restart and upgrade

Seedling restarts (clean or crash, including mid-upgrade binary replacement) are
largely transparent because the critical design decisions already compose well:

| Concern                     | Survives restart? | Reason                                              |
|-----------------------------|:-----------------:|-----------------------------------------------------|
| DNAT forwarding rules       | yes               | kernel-owned; process lifecycle irrelevant          |
| App containers              | yes               | systemd transient units; not tied to seedling's PID |
| Caddy container             | yes               | same — another transient unit                       |
| Caddy routing config        | yes               | lives in Caddy's process memory; Caddy keeps running|
| Pod networks                | yes               | podman networks persist independently               |
| Volumes                     | yes               | persistent by definition                           |
| DB state / operation replay | yes               | already persisted; existing replay infrastructure   |

**What seedling re-establishes on startup:**

The only in-memory state that must be reconstructed is `CaddyProxy`'s current
`SocketAddr`. Seedling inspects running containers for the active Caddy instance,
reads its IP on `seedling-proxy`, and re-initialises `CaddyProxy`. Everything
else is handled by the reconciliation loop's first tick, which observes actual
system state and converges from there — the same path used for normal operation.

**Crash mid-Caddy-upgrade:**

The DB is the oracle for which Caddy container is active. The upgrade sequence
writes the active container name to the DB at step 8 (after DNAT switch) and
again at step 11 (after cleanup). On startup, seedling reads the DB to determine
which container name was last recorded as active.

If both `seedling-caddy` and `seedling-caddy-next` exist:
- DB says `seedling-caddy` is active → crash occurred before step 8 (DNAT not
  yet switched). Stop and remove `seedling-caddy-next`; proceed with the
  recorded active container.
- DB says `seedling-caddy-next` is active → crash occurred between steps 8 and
  11 (DNAT switched, old not yet cleaned up). Stop and remove `seedling-caddy`;
  `CaddyProxy` is initialised from `seedling-caddy-next`.

If only `seedling-caddy-next` exists and DB says it is active: old container
was already cleaned up. `CaddyProxy` is initialised from `seedling-caddy-next`.

In all cases no container rename is needed — the DB name is the authority.

**DB schema migrations:**

If a new version of seedling requires a schema change, migrations run before
the reconciliation loop starts — the same as any other restart. No special
handling beyond what the existing migration infrastructure provides.

---

## nftables port forwarder (`src/system/nftables.rs`)

- Crate: `nftables` (v0.6+) with the `tokio` feature. Seedling calls the
  crate's typed Rust API; the crate internally drives the `nft` binary in JSON
  mode (`nft -j`). The `nft` binary must be present on the host — this is a
  runtime dependency of the crate, not of seedling's code directly.
- Manages a dedicated nftables table: `table inet seedling_ingress {}`.
- All DNAT rules live in a single chain within that table, making cleanup
  (`clear_rules`) a simple table flush.
- `apply_rules` builds a `nftables::schema::Nftables` ruleset value, flushes
  the chain, and appends the new rules in a single atomic transaction via
  `nftables::helper::apply_ruleset`.
- Each `ForwardingRule` with `ForwardProto::Both` produces two rules (one TCP,
  one UDP) sharing the same destination.

Example ruleset for two ingress ports:

```
table inet seedling_ingress {
    chain prerouting {
        type nat hook prerouting priority dstnat; policy accept;
        tcp dport 80  dnat to 10.88.0.2:80
        udp dport 80  dnat to 10.88.0.2:80
        tcp dport 443 dnat to 10.88.0.2:443
        udp dport 443 dnat to 10.88.0.2:443
    }
}
```

Note: UDP DNAT at port 443 is required for QUIC (HTTP/3) support when Caddy's
`quic` ingress option is set.

---

## Implementation order

1. **`src/system/types.rs`** — all shared data types; no logic, no deps.
2. **`src/system/mod.rs`** — `SystemDriver` struct and re-exports.
3. **`src/system/translate/`** — pure conversion functions; testable without
   any backend. Start with `container.rs` → `podman_args`, then `proxy.rs`.
4. **`src/system/podman.rs`** — `PodmanRuntime`; stub all methods with
   `todo!()`, then implement incrementally starting with `inspect` and `list`.
5. **`src/system/systemd.rs`** — `SystemdManager`; stub first, implement
   `start_transient` and `stop_unit` as the first two live methods.
6. **`src/system/caddy.rs`** — `CaddyProxy`; stub first, implement
   `apply_config` and `is_healthy`.
7. **`src/system/nftables.rs`** — `NftablesForwarder`; implement `apply_rules`
   and `clear_rules`. Test with a real nftables table in isolation.
8. **`src/system/observer.rs`** — `Observer`; implement `observe` per resource
   kind, one at a time, starting with `Deployment`.
9. **`src/system/actuator.rs`** — `Actuator`; implement `start` and `stop` for
   `Deployment` and `Volume` first, then `Ingress` (which coordinates proxy
   config and forwarding rules together).
10. Wire the reconciliation loop in `main` using the real `SystemDriver`.

---

## Open questions for implementation time

- **`async fn` in traits + `Send` bounds**: use `trait-variant` (idiomatic,
  minimal) or `async_trait` (heavier but widely understood). Decide when adding
  the tokio dependency.

- **ip_forward**: nftables DNAT requires `net.ipv4.ip_forward=1` (and the
  IPv6 equivalent). Rootful podman typically sets this already; confirm and
  document the assumption.

- **Localhost → ingress port access**: nftables `prerouting` DNAT does not
  apply to traffic originating on the host. Accessing ingress ports from
  localhost (seedling health checks, monitoring, etc.) requires either an
  additional `output` chain DNAT rule or `net.ipv4.conf.all.route_localnet=1`.
  Decide whether to add the output rule unconditionally or document the
  limitation.

- **Client IP visibility**: with DNAT, Caddy sees the original client IP as the
  connection source (conntrack handles reverse translation on replies). Verify
  this holds with the specific bridge/network setup and document whether Caddy's
  `trusted_proxies` config needs adjustment.