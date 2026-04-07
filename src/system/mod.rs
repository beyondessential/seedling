use std::sync::Arc;
use std::time::Duration;

use ipnet::Ipv6Net;

use crate::system::types::{
    ContainerFilter, ContainerState, ContainerSummary, DataPlaneRules, ExecHandle, ExecSpec,
    ProxyConfig, ServiceRoute, TransientUnitSpec, UnitState, UnitSummary,
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

    // Networks — one IPv6 /64 per pod instance.
    // The host bridge is assigned ::1 (gateway) and ::2 (mount endpoint).
    fn network_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>>;
    fn create_network<'a>(
        &'a self,
        name: &'a str,
        prefix: Ipv6Net,
    ) -> BoxFuture<'a, Result<(), BoxError>>;
    fn remove_network<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;

    // Volumes
    fn volume_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>>;
    fn create_volume<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;
    fn remove_volume<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>>;

    // Forced cleanup (e.g. seedling crashed while container was running)
    fn remove_container<'a>(
        &'a self,
        name: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>>;

    // Interactive exec (for BSL shell sessions)
    fn exec<'a>(
        &'a self,
        name: &'a str,
        spec: ExecSpec,
    ) -> BoxFuture<'a, Result<ExecHandle, BoxError>>;
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
