use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    time::{Duration, SystemTime},
};

use ipnet::Ipv6Net;

// ---------------------------------------------------------------------------
// Container observation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ContainerState {
    pub status: ContainerStatus,
    pub health: ContainerHealth,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub started_at: Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
    /// The container's IPv6 address on its pod network, if known.
    pub pod_addr: Option<Ipv6Addr>,
    /// The container's IPv4 address on its pod network, if known.
    pub pod_addr_v4: Option<Ipv4Addr>,
    /// The image ID (config digest) the container was started from, if known.
    pub image_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerStatus {
    Created,
    Running,
    Paused,
    Exited,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerHealth {
    None,
    Starting,
    Healthy,
    Unhealthy,
}

#[derive(Debug, Clone)]
pub struct ContainerSummary {
    pub name: String,
    pub status: ContainerStatus,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ContainerFilter<'a> {
    pub label: Option<(&'a str, &'a str)>,
    pub name_prefix: Option<&'a str>,
}

/// Returned by `ContainerRuntime::list_networks`; carries the bridge
/// interface name needed by the startup bridge-address check.
#[derive(Debug, Clone)]
pub struct NetworkSummary {
    pub name: String,
    pub bridge_name: String,
}

// ---------------------------------------------------------------------------
// Container spec
// ---------------------------------------------------------------------------

/// Intermediate representation produced by the translate layer.
/// `podman_args` turns it into the ExecStart argv for the transient systemd unit.
#[derive(Debug, Clone)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub entrypoint: Vec<String>,
    pub env: Vec<(String, String)>,
    pub mounts: Vec<Mount>,
    /// Pod network name.
    pub network: String,
    pub labels: HashMap<String, String>,
    pub health: Option<HealthCheckSpec>,
    /// Entries injected into /etc/hosts inside the container.
    /// Used to map `localmount` to the pod's ::2 mount endpoint address.
    pub hosts: Vec<(String, IpAddr)>,
}

#[derive(Debug, Clone)]
pub struct Mount {
    pub source: MountSource,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub enum MountSource {
    Volume(String),
    Bind(PathBuf),
    Tmpfs,
}

#[derive(Debug, Clone)]
pub struct HealthCheckSpec {
    pub command: Vec<String>,
    pub interval: Duration,
    pub timeout: Duration,
    pub retries: u32,
    pub start_period: Duration,
}

// ---------------------------------------------------------------------------
// Exec spec and handle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ExecSpec {
    pub command: Vec<String>,
    pub env: Vec<(String, String)>,
    pub tty: bool,
    pub user: Option<String>,
}

/// Handle returned by `ContainerRuntime::exec` for an interactive PTY session.
///
/// `stdin` writes to the PTY master (input to the container process).
/// `stdout` reads from the PTY master (output from the container process; stderr
/// is merged into stdout by the PTY).
/// `pty_master_fd` is the raw fd of the PTY master for `TIOCSWINSZ` resize ioctls.
/// It remains valid for as long as `stdin`/`stdout` are alive.
/// `child` is the subprocess running `podman exec`.
pub struct ExecHandle {
    pub stdin: tokio::io::WriteHalf<tokio::fs::File>,
    pub stdout: tokio::io::ReadHalf<tokio::fs::File>,
    pub pty_master_fd: std::os::unix::io::RawFd,
    pub child: tokio::process::Child,
}

// ---------------------------------------------------------------------------
// Transient unit spec
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TransientUnitSpec {
    /// App containers: `"seedling-{display_name}.service"`
    /// Caddy:          `"seedling-caddy.service"` / `"seedling-caddy-next.service"`
    pub name: String,
    pub description: String,
    /// Full `podman run [...]` argv — produced by `translate::container::podman_args`.
    pub exec_start: Vec<String>,
    pub restart: TransientRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientRestart {
    No,
    OnFailure,
    Always,
}

// ---------------------------------------------------------------------------
// systemd unit observation
// ---------------------------------------------------------------------------

/// `unit_state` returns `None` when the unit does not exist or is masked.
#[derive(Debug, Clone)]
pub struct UnitState {
    pub active: ActiveState,
    pub sub: String,
}

#[derive(Debug, Clone)]
pub struct UnitSummary {
    pub name: String,
    pub state: UnitState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveState {
    Active,
    Activating,
    Deactivating,
    Inactive,
    Failed,
}

// ---------------------------------------------------------------------------
// DataPlane types
// ---------------------------------------------------------------------------

/// The complete desired state for all nftables rules in `seedling_net`.
/// Applied atomically; replaces the previous rule set entirely.
#[derive(Debug, Clone, Default)]
pub struct DataPlaneRules {
    /// External ingress: host port → Caddy's IPv6 address.
    pub ingress: Vec<IngressRule>,
    /// Service mount DNAT6: per-pod localmount:port → backend pod_ip:pod_port.
    pub mounts: Vec<MountRule>,
    /// Service DNAT6: service_ip:service_port → backend pod_ip:pod_port.
    pub service_dnat: Vec<ServiceDnatRule>,
}

/// Redirects an external host port to Caddy's container address.
/// All ingress traffic (HTTP and L4) flows through Caddy.
/// Applied in both prerouting and output nftables chains.
#[derive(Debug, Clone)]
pub struct IngressRule {
    pub external_port: u16,
    pub proto: ForwardProto,
    /// Caddy's IPv6 address:port on the proxy network.
    pub caddy_v6: SocketAddr,
    /// Caddy's IPv4 address:port, if the proxy network is dual-stack.
    pub caddy_v4: Option<SocketAddr>,
}

/// DNAT6 rule translating a mount endpoint to a backing pod's address and
/// pod-side port. Scoped to traffic from a specific pod's /64 destined for
/// that pod's mount endpoint address (::1000).
// r[impl infra.dataplane.mount-dnat]
#[derive(Debug, Clone)]
pub struct MountRule {
    /// The mounting pod's /64 prefix.
    pub pod_prefix: Ipv6Net,
    /// pod-prefix::1000 — the mount endpoint address on the bridge.
    pub mount_addr: Ipv6Addr,
    /// Port declared in `.mount()`.
    pub mount_port: u16,
    /// Backend pod addresses and pod-side ports.
    pub backends: Vec<(Ipv6Addr, u16)>,
    pub proto: ForwardProto,
}

/// DNAT6 rule translating service_ip:service_port to a backing pod's
/// address and pod-side port. When multiple backends exist, connections are
/// distributed round-robin via nftables `numgen`.
///
/// Applied in the nftables prerouting chain.
// r[impl infra.dataplane.service-dnat]
#[derive(Debug, Clone)]
pub struct ServiceDnatRule {
    /// The service's stable IPv6 address.
    pub service_ip: Ipv6Addr,
    /// The endpoint-side (service) port.
    pub service_port: u16,
    /// Backend pod addresses and pod-side ports.
    pub backends: Vec<(Ipv6Addr, u16)>,
    pub proto: ForwardProto,
}

/// Maps a service IP to one or more backing pod instance addresses via ECMP.
/// Applied to the host routing table via rtnetlink.
#[derive(Debug, Clone)]
pub struct ServiceRoute {
    pub service_ip: Ipv6Addr,
    /// Pod instance addresses. Empty = blackhole route.
    pub backends: Vec<Ipv6Addr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ForwardProto {
    Tcp,
    Udp,
    /// Emits one TCP rule and one UDP rule.
    Both,
}

// ---------------------------------------------------------------------------
// Proxy config
// ---------------------------------------------------------------------------

/// Full-replacement config document sent to Caddy's admin API.
/// Upstreams reference service IPv6 addresses; Caddy reaches them through
/// the host routing table. Caddy does not join pod networks.
#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    /// Ports Caddy should bind inside its container.
    pub listeners: Vec<ProxyListener>,
    pub virtual_hosts: Vec<VirtualHost>,
    /// Layer-4 (TCP/UDP) routes for non-HTTP ingresses proxied via Caddy L4.
    pub l4_routes: Vec<L4Route>,
}

/// A layer-4 (TCP/UDP) proxy route for non-HTTP ingresses.
/// Caddy L4 listens on the port and forwards to service upstreams.
#[derive(Debug, Clone)]
pub struct L4Route {
    pub port: u16,
    pub proto: L4Proto,
    /// Upstream addresses: `"[fd5e:...]:port"` format.
    pub upstreams: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L4Proto {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProxyListener {
    pub port: u16,
    pub proto: ProxyListenerProto,
}

/// Protocol type for a Caddy listener.
/// Distinct from `ForwardProto` (nftables); Caddy's model is HTTP/HTTPS/QUIC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProxyListenerProto {
    Http,
    Https,
    /// HTTP/3 over QUIC (UDP). Requires TLS.
    Quic,
}

#[derive(Debug, Clone)]
pub struct VirtualHost {
    pub hostname: String,
    pub tls_acme: bool,
    /// If present, add an HTTP→HTTPS redirect server block.
    pub redirect: Option<HttpRedirect>,
    pub routes: Vec<ProxyRoute>,
}

#[derive(Debug, Clone)]
pub struct HttpRedirect {
    pub from_port: u16,
    pub code: u16,
}

#[derive(Debug, Clone)]
pub struct ProxyRoute {
    pub prefix: String,
    /// One upstream per service: `"http://[fd5e:ed...]:3000"`.
    /// ECMP at the kernel distributes connections across instances.
    pub upstreams: Vec<String>,
}

// ---------------------------------------------------------------------------
// Observation facts
// ---------------------------------------------------------------------------

/// Bridge between the system layer and the runtime history.
/// `Observer` produces these; the reconciler loop persists them to
/// `world_observations`. Each call to `observe` is scoped to one
/// `ResourceInstance`; facts are subject-identified by the call site.
#[derive(Debug, Clone)]
pub enum ObservationFact {
    // Container
    ContainerMissing,
    ContainerCreated,
    ContainerRunning {
        pid: u32,
    },
    ContainerExited {
        exit_code: i32,
    },
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
    /// The unit is not loaded by systemd at all (unit_state returned None).
    UnitGone,

    // Proxy
    ProxyReachable,
    ProxyUnreachable,
    RoutePresent {
        hostname: String,
    },
    RouteAbsent {
        hostname: String,
    },
}

impl ObservationFact {
    // r[impl observe.persist]
    /// Maps this observation to the `(obs_kind, payload)` pairs written to
    /// `world_observations`. Returns multiple entries when one fact advances
    /// several oracle states simultaneously (e.g. VolumePresent → created +
    /// ready). Returns an empty slice when this fact has no oracle mapping.
    pub fn to_obs_kinds(&self) -> Vec<(&'static str, serde_json::Value)> {
        use serde_json::json;
        match self {
            ObservationFact::ContainerCreated => vec![("container_created", json!({}))],
            ObservationFact::ContainerRunning { pid } => {
                vec![("container_running", json!({ "pid": pid }))]
            }
            ObservationFact::ContainerExited { exit_code } => {
                vec![("container_exited", json!({ "exit_code": exit_code }))]
            }
            ObservationFact::ContainerHealthy => vec![("health_check_pass", json!({}))],
            ObservationFact::ContainerMissing => vec![("container_removed", json!({}))],
            // A present volume is both created and ready.
            ObservationFact::VolumePresent => {
                vec![("volume_created", json!({})), ("volume_ready", json!({}))]
            }
            ObservationFact::VolumeMissing => vec![("volume_cleaned_up", json!({}))],
            // Ingress observations are emitted by proxy::apply, not here.
            // Network observations are used only for pod actuation decisions.
            // Unit and health-failure facts have no direct oracle mapping.
            ObservationFact::ContainerUnhealthy
            | ObservationFact::NetworkPresent
            | ObservationFact::NetworkMissing
            | ObservationFact::ProxyReachable
            | ObservationFact::ProxyUnreachable
            | ObservationFact::RoutePresent { .. }
            | ObservationFact::RouteAbsent { .. }
            | ObservationFact::UnitActive
            | ObservationFact::UnitInactive
            | ObservationFact::UnitFailed
            | ObservationFact::UnitGone => vec![],
        }
    }
}
