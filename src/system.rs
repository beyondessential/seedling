use std::{path::Path, sync::Arc};

use ipnet::{Ipv4Net, Ipv6Net};
use sha2::{Digest, Sha256};

use crate::system::types::{
    ContainerFilter, ContainerSpec, ContainerState, ContainerSummary, DataPlaneRules, ExecHandle,
    NetworkSummary, ProxyConfig, ServiceRoute, TransientUnitSpec, UnitState, UnitSummary,
};

pub mod actuator;
pub mod nat64;
pub mod observer;
pub mod reconcile;
pub mod translate;
pub mod types;

pub(crate) mod caddy;
pub(crate) mod confined_write;
pub(crate) mod data_plane;
pub mod jool;
pub(crate) mod journal;
pub(crate) mod podman;
pub mod resolver;
pub(crate) mod systemd;
pub mod volume_store;

pub use actuator::{ActuateError, Actuator};
pub use observer::{ObserveError, Observer};
pub use types::{
    ActiveState, ContainerHealth, ContainerStatus, DataPlaneRules as SystemDataPlaneRules,
    ExecHandle as SystemExecHandle, ForwardProto, HealthCheckSpec, HttpRedirect, IngressRule,
    Mount, MountRule, MountSource, ObservationFact, ProxyConfig as SystemProxyConfig,
    ProxyListener, ProxyListenerProto, ProxyRoute, ResolvedExternalMount,
    ServiceRoute as SystemServiceRoute, TransientRestart, VirtualHost,
};

// ---------------------------------------------------------------------------
// Shared error / future aliases
// ---------------------------------------------------------------------------

/// Boxed, type-erased error returned by all backend trait methods.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Heap-allocated, Send future returned by dyn-compatible async trait methods.
pub(crate) use futures_util::future::BoxFuture;

// ---------------------------------------------------------------------------
// Backend traits
// ---------------------------------------------------------------------------

pub trait ContainerRuntime: Send + Sync + 'static {
    fn inspect<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<ContainerState>, BoxError>>;
    fn list<'a>(
        &'a self,
        filter: ContainerFilter<'a>,
    ) -> BoxFuture<'a, Result<Vec<ContainerSummary>, BoxError>>;

    // Images
    fn image_exists<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<bool, BoxError>>;
    fn pull_image<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
    /// Returns the image ID (config digest, e.g. `sha256:…`) for a locally-stored image.
    /// Returns `None` if the image is not present locally.
    fn local_image_id<'a>(
        &'a self,
        reference: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, BoxError>>;

    // Networks — one IPv6 /64 per pod instance.
    // The host bridge is assigned ::1 (gateway) and ::2 (mount endpoint).
    fn network_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>>;
    /// Returns the Linux bridge interface name assigned to the network.
    fn create_network<'a>(
        &'a self,
        name: &'a str,
        prefix: Ipv6Net,
        ipv4: Option<Ipv4Net>,
    ) -> BoxFuture<'a, Result<String, BoxError>>;
    fn remove_network<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
    /// List all networks whose name starts with `prefix`.
    /// Returns the name and bridge interface name for each.
    fn list_networks<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<NetworkSummary>, BoxError>>;

    // Volumes
    fn volume_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>>;
    fn create_volume<'a>(
        &'a self,
        name: &'a str,
        tmpfs: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>>;
    fn remove_volume<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
    /// List volume names matching a given prefix.
    fn list_volumes_by_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, BoxError>>;
    /// Returns the host filesystem path where the named volume is mounted.
    fn volume_mountpoint<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<std::path::PathBuf, BoxError>>;

    // Forced cleanup (e.g. seedling crashed while container was running)
    fn remove_container<'a>(
        &'a self,
        name: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>>;

    // Interactive shell sessions — runs a fresh ephemeral container with a PTY.
    fn exec<'a>(&'a self, spec: ContainerSpec) -> BoxFuture<'a, Result<ExecHandle, BoxError>>;
}

pub trait ProcessManager: Send + Sync + 'static {
    // Transient units — container lifecycle; no unit file written to disk.
    fn start_transient<'a>(
        &'a self,
        spec: TransientUnitSpec,
    ) -> BoxFuture<'a, Result<(), BoxError>>;
    /// Sends the stop signal; returns immediately without waiting.
    fn stop_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
    /// Clears the failed state of a unit (equivalent to `systemctl reset-failed`).
    /// Required before re-starting a unit that hit its start rate limit.
    fn reset_failed_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
    fn unit_state<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<UnitState>, BoxError>>;
    fn list_units<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<UnitSummary>, BoxError>>;

    // Persistent units — written to the unit drop-in path.
    fn write_unit<'a>(
        &'a self,
        name: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), BoxError>>;
    fn remove_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
    fn daemon_reload<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>>;
    fn start_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
}

pub trait NetworkProxy: Send + Sync + 'static {
    fn is_healthy<'a>(&'a self) -> BoxFuture<'a, Result<bool, BoxError>>;
    fn apply_config<'a>(&'a self, config: &'a ProxyConfig) -> BoxFuture<'a, Result<(), BoxError>>;
}

pub trait DataPlane: Send + Sync + 'static {
    /// Atomically replace the complete nftables rule set in `seedling_net`.
    /// Idempotent. Covers ingress DNAT, FORWARD policy, and mount DNAT6.
    fn apply_rules<'a>(&'a self, rules: &'a DataPlaneRules) -> BoxFuture<'a, Result<(), BoxError>>;

    /// Replace the complete set of IPv6 service routes in the routing table.
    /// Each route maps a service IP to one or more pod instance IPs (ECMP).
    fn apply_routes<'a>(
        &'a self,
        routes: &'a [ServiceRoute],
    ) -> BoxFuture<'a, Result<(), BoxError>>;

    /// Remove all rules and routes owned by seedling. Called on shutdown.
    fn clear_all<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>>;
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

pub struct System {
    pub container: Arc<dyn ContainerRuntime>,
    pub process: Arc<dyn ProcessManager>,
    pub proxy: Arc<dyn NetworkProxy>,
    pub data_plane: Arc<dyn DataPlane>,
    pub volume_store: volume_store::VolumeStore,
}

// r[infra.node.prefix]
/// Derive the node's /48 ULA prefix from `/etc/machine-id`.
///
/// The raw machine-id content (whitespace-trimmed) is hashed with SHA-256;
/// the first four bytes of the digest fill octets 2–5 of the prefix:
///
/// ```text
/// fd5e : <hash[0]><hash[1]> : <hash[2]><hash[3]> :: /48
/// ```
///
/// Hashing instead of direct interpretation means the derivation is
/// agnostic to the machine-id format (plain hex, UUID with dashes, etc.).
pub fn node_prefix_from_machine_id() -> std::io::Result<Ipv6Net> {
    let raw = std::fs::read_to_string("/etc/machine-id")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "machine-id is empty",
        ));
    }

    let digest = Sha256::digest(trimmed.as_bytes());

    let mut octets = [0u8; 16];
    octets[0] = 0xfd;
    octets[1] = 0x5e;
    octets[2] = digest[0];
    octets[3] = digest[1];
    octets[4] = digest[2];
    octets[5] = digest[3];

    Ok(Ipv6Net::new(std::net::Ipv6Addr::from(octets), 48)
        .expect("48 is a valid IPv6 prefix length"))
}

impl System {
    // r[infra.proxy.startup]
    /// Initialize all system backends, ensure Caddy is running, and return the
    /// assembled `System` handle alongside the Caddy admin client handle.
    ///
    /// Errors from individual backends are returned as boxed trait objects so
    /// callers do not need to depend on internal error types.
    pub async fn setup(
        node_prefix: Ipv6Net,
        data_dir: &Path,
        use_btrfs: bool,
    ) -> Result<
        (Arc<Self>, Arc<tokio::sync::RwLock<reqwest::Client>>),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let container: Arc<dyn ContainerRuntime> = Arc::new(podman::PodmanRuntime::new().await?);
        let process: Arc<dyn ProcessManager> = Arc::new(systemd::SystemdManager::connect().await?);
        let data_plane_arc: Arc<dyn DataPlane> =
            Arc::new(data_plane::NftablesDataPlane::new(node_prefix)?);

        let initial =
            caddy::ensure_caddy_running(&*container, &*process, &node_prefix, data_dir).await?;
        let caddy_proxy = Arc::new(caddy::CaddyProxy::new(&initial.admin_socket)?);
        let caddy_admin_client = caddy_proxy.admin_client_handle();
        let proxy: Arc<dyn NetworkProxy> = caddy_proxy;

        let volume_store = volume_store::VolumeStore::new(data_dir, use_btrfs)?;

        let system = Arc::new(Self {
            container,
            process,
            proxy,
            data_plane: data_plane_arc,
            volume_store,
        });
        Ok((system, caddy_admin_client))
    }
}
