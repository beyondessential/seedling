use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use ipnet::{Ipv4Net, Ipv6Net};

use crate::system::types::{
    ContainerFilter, ContainerSpec, ContainerState, ContainerSummary, DataPlaneRules, ExecHandle,
    NetworkSummary, ProxyConfig, ServiceRoute, TransientUnitSpec, UnitState, UnitSummary,
};

pub mod actuator;
pub mod observer;
pub mod reconcile;
pub mod translate;
pub mod types;

pub(crate) mod caddy;
pub(crate) mod data_plane;
pub(crate) mod podman;
pub(crate) mod systemd;

pub use actuator::{ActuateError, Actuator};
pub use observer::{ObserveError, Observer};
pub use types::{
    ActiveState, ContainerHealth, ContainerStatus, DataPlaneRules as SystemDataPlaneRules,
    ExecHandle as SystemExecHandle, ForwardProto, HealthCheckSpec, HttpRedirect, IngressRule,
    Mount, MountRule, MountSource, ObservationFact, ProxyConfig as SystemProxyConfig,
    ProxyListener, ProxyListenerProto, ProxyRoute, ServiceRoute as SystemServiceRoute,
    TransientRestart, VirtualHost,
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
    fn create_volume<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
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
    /// Use `wait_unit_stopped` to block until the unit has fully stopped.
    fn stop_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
    /// Clears the failed state of a unit (equivalent to `systemctl reset-failed`).
    /// Required before re-starting a unit that hit its start rate limit.
    fn reset_failed_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
    /// Polls until the unit reaches an inactive or failed state, or the
    /// timeout elapses. Required before removing pod networks or volumes.
    fn wait_unit_stopped<'a>(
        &'a self,
        name: &'a str,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<(), BoxError>>;
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
}

impl System {
    // r[infra.proxy.startup]
    /// Initialize all system backends, ensure Caddy is running, and return the
    /// assembled `System` handle alongside the Caddy admin API address handle.
    ///
    /// Errors from individual backends are returned as boxed trait objects so
    /// callers do not need to depend on internal error types.
    pub async fn setup(
        node_prefix: Ipv6Net,
        data_dir: &Path,
    ) -> Result<
        (Arc<Self>, Arc<tokio::sync::RwLock<SocketAddr>>),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let container: Arc<dyn ContainerRuntime> = Arc::new(podman::PodmanRuntime::new().await?);
        let process: Arc<dyn ProcessManager> = Arc::new(systemd::SystemdManager::connect().await?);
        let data_plane_arc: Arc<dyn DataPlane> = Arc::new(data_plane::NftablesDataPlane::new()?);

        let initial =
            caddy::ensure_caddy_running(&*container, &*process, &node_prefix, data_dir).await?;
        let caddy_proxy = Arc::new(caddy::CaddyProxy::new(initial.v6));
        let caddy_admin_addr = caddy_proxy.admin_addr_handle();
        let proxy: Arc<dyn NetworkProxy> = caddy_proxy;

        let system = Arc::new(Self {
            container,
            process,
            proxy,
            data_plane: data_plane_arc,
        });
        Ok((system, caddy_admin_addr))
    }
}
