use std::{
    collections::{BTreeMap, HashMap},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    time::{Duration, SystemTime},
};

use ipnet::Ipv6Net;
use seedling_protocol::env::EnvVar;

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
    /// The `seedling.spec-hash` label value from the running container, if present.
    /// Used to detect config drift without re-running the spec computation.
    pub spec_hash: Option<String>,
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

#[derive(Debug, Clone, Copy, Default)]
pub struct ContainerFilter<'a> {
    pub label: Option<(&'a str, &'a str)>,
    /// Match containers that have this label key, regardless of value.
    pub label_key: Option<&'a str>,
    pub name_prefix: Option<&'a str>,
}

/// Returned by `ContainerRuntime::list_networks`; carries the bridge
/// interface name needed by the startup bridge-address check.
#[derive(Debug, Clone)]
pub struct NetworkSummary {
    pub name: String,
    pub bridge_name: String,
}

/// Returned by `ContainerRuntime::list_images`. Drives the `/images/list`
/// OI endpoint and the autonomous GC's size/age book-keeping.
#[derive(Debug, Clone)]
pub struct ImageSummary {
    /// Content-addressable digest, e.g. `"sha256:..."`.
    pub image_id: String,
    /// Named tag references (e.g. `"docker.io/library/node:latest"`).
    pub tags: Vec<String>,
    /// Pinned-digest references (e.g. `"docker.io/library/node@sha256:..."`).
    /// May contain both the image-manifest digest and the manifest-list
    /// digest of the multi-arch tag the image was pulled from; use
    /// [`Self::manifest_digest`] to tell them apart.
    pub digests: Vec<String>,
    /// The image's own manifest digest (e.g. `"sha256:..."`), when the
    /// container runtime reports one. A digest reference in `digests`
    /// whose hash matches this is the image manifest; any others are
    /// manifest-list digests for the multi-arch tag the image came from.
    pub manifest_digest: Option<String>,
    pub size_bytes: i64,
    /// Image creation time as reported by the runtime, Unix seconds.
    pub created_at_secs: i64,
}

impl ImageSummary {
    /// Flat list of every reference (tags + digests) that currently
    /// resolves to this image. Used for DB bookkeeping.
    pub fn all_references(&self) -> impl Iterator<Item = &str> {
        self.tags
            .iter()
            .chain(self.digests.iter())
            .map(String::as_str)
    }
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
    pub env: Vec<EnvVar>,
    pub mounts: Vec<Mount>,
    /// Pod network name.
    pub network: String,
    pub labels: BTreeMap<String, String>,
    pub health: Option<HealthCheckSpec>,
    /// Entries injected into /etc/hosts inside the container.
    /// Used to map `localmount` to the pod's ::2 mount endpoint address.
    pub hosts: Vec<(String, IpAddr)>,
    /// DNS servers to inject into the container's /etc/resolv.conf.
    pub dns_servers: Vec<Ipv6Addr>,
    pub memory: Option<String>,
    pub cpus: Option<f64>,
    pub extra_caps: Vec<String>,
    pub writable_rootfs: bool,
    pub pids_limit: u32,
    pub workdir: Option<String>,
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
pub struct ResolvedExternalMount {
    pub source: MountSource,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct HealthCheckSpec {
    pub command: Vec<String>,
    pub interval: Duration,
    pub timeout: Duration,
    pub retries: u32,
    pub start_period: Duration,
    pub on_failure: HealthCheckOnFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthCheckOnFailure {
    None,
    Kill,
    Restart,
    Stop,
}

impl HealthCheckOnFailure {
    pub fn podman_arg(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Kill => "kill",
            Self::Restart => "restart",
            Self::Stop => "stop",
        }
    }
}

// ---------------------------------------------------------------------------
// Exec handle
// ---------------------------------------------------------------------------

/// Handle returned by `ContainerRuntime::exec` for an interactive PTY session.
///
/// `stdin` writes to the PTY master (input to the container process).
/// `stdout` reads from the PTY master (output from the container process; stderr
/// is merged into stdout by the PTY).
/// `pty_master_fd` is the raw fd of the PTY master for `TIOCSWINSZ` resize ioctls.
/// It remains valid for as long as `stdin`/`stdout` are alive.
/// `child` is the subprocess running `podman run`.
///
/// The I/O halves are backed by `AsyncFd` with `O_NONBLOCK` so that reads and
/// writes integrate with the tokio event loop via epoll rather than blocking
/// thread-pool tasks.  Using `tokio::fs::File` for a PTY fd is incorrect: it
/// uses `spawn_blocking` and a single-operation state machine, which prevents
/// concurrent reads and writes — causing stdin to stall while waiting for
/// stdout data.
pub struct ExecHandle {
    pub stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
    pub stdout: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
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
    /// Structured journal fields attached to every log entry from this unit.
    /// Each pair is `(KEY, VALUE)`, emitted as `KEY=VALUE`.
    pub log_extra_fields: Vec<(String, String)>,
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
    /// Hostnames whose TLS certificates should be pre-provisioned without
    /// adding routes (`rt.warm_certs`). Translated to
    /// `tls.certificates.automate` plus matching policy subjects so that Caddy
    /// acquires the certs eagerly, not lazily on first request.
    // r[impl actuate.ingress.warm-certs]
    pub warm_cert_hostnames: std::collections::BTreeSet<String>,
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
    /// HTTP/3 over QUIC (UDP).
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
    /// The spec hash label read from the running container.
    ContainerSpecHash(String),

    // Network
    NetworkPresent,
    NetworkMissing,

    // Volume
    VolumePresent,
    VolumeMissing,
    VolumeBackendMismatch,

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
            // r[impl lifecycle.container.unhealthy-transition]
            ObservationFact::ContainerUnhealthy => vec![("health_check_fail", json!({}))],
            ObservationFact::ContainerMissing => vec![("container_removed", json!({}))],
            // A present volume is both created and ready.
            ObservationFact::VolumePresent => {
                vec![("volume_created", json!({})), ("volume_ready", json!({}))]
            }
            ObservationFact::VolumeMissing => vec![("volume_cleaned_up", json!({}))],
            ObservationFact::VolumeBackendMismatch => {
                vec![("volume_backend_mismatch", json!({}))]
            }
            // l[impl rt.termination.ensure-success]
            // UnitFailed is persisted because termination_success needs to
            // distinguish "container exited but we didn't capture the exit
            // code (e.g. --rm removed it before we inspected)" from "the
            // systemd unit actually failed". The ContainerExited fact is the
            // primary signal when we catch it; unit_failed is a reliable
            // secondary signal from systemd for crashes/OOMs/signals that
            // podman --rm cleans up before we can observe them.
            ObservationFact::UnitFailed => vec![("unit_failed", json!({}))],
            // Ingress observations are emitted by proxy::apply, not here.
            // Network observations are used only for pod actuation decisions.
            // The remaining unit facts are consumed only within a single tick
            // for actuation decisions and have no oracle mapping.
            ObservationFact::ContainerSpecHash(_)
            | ObservationFact::NetworkPresent
            | ObservationFact::NetworkMissing
            | ObservationFact::ProxyReachable
            | ObservationFact::ProxyUnreachable
            | ObservationFact::RoutePresent { .. }
            | ObservationFact::RouteAbsent { .. }
            | ObservationFact::UnitActive
            | ObservationFact::UnitInactive
            | ObservationFact::UnitGone => vec![],
        }
    }
}

#[cfg(test)]
mod observation_fact_tests {
    use super::*;

    // r[verify lifecycle.container.unhealthy-transition]
    #[test]
    fn container_unhealthy_maps_to_health_check_fail() {
        let kinds = ObservationFact::ContainerUnhealthy.to_obs_kinds();
        assert_eq!(kinds.len(), 1);
        assert_eq!(kinds[0].0, "health_check_fail");
    }

    // r[verify lifecycle.container]
    #[test]
    fn container_healthy_maps_to_health_check_pass() {
        let kinds = ObservationFact::ContainerHealthy.to_obs_kinds();
        assert_eq!(kinds.len(), 1);
        assert_eq!(kinds[0].0, "health_check_pass");
    }

    // r[verify healthcheck.on-failure]
    #[test]
    fn on_failure_podman_strings_cover_all_variants() {
        assert_eq!(HealthCheckOnFailure::None.podman_arg(), "none");
        assert_eq!(HealthCheckOnFailure::Kill.podman_arg(), "kill");
        assert_eq!(HealthCheckOnFailure::Restart.podman_arg(), "restart");
        assert_eq!(HealthCheckOnFailure::Stop.podman_arg(), "stop");
    }
}
