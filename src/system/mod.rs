// The system module is production infrastructure not yet wired to the
// reconciliation loop. Suppress dead-code noise until it is.
#![allow(dead_code, unused_imports)]

use std::time::Duration;

use ipnet::Ipv6Net;

use crate::system::types::{
    ContainerFilter, ContainerState, ContainerSummary, DataPlaneRules, ExecHandle, ExecSpec,
    ProxyConfig, ServiceRoute, TransientUnitSpec, UnitState, UnitSummary,
};

pub mod actuator;
pub mod observer;
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
// Backend traits
// ---------------------------------------------------------------------------

#[trait_variant::make(ContainerRuntime: Send)]
pub trait LocalContainerRuntime: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    // Observation
    async fn inspect(&self, name: &str) -> Result<Option<ContainerState>, Self::Error>;
    async fn list(&self, filter: ContainerFilter<'_>)
    -> Result<Vec<ContainerSummary>, Self::Error>;

    // Images
    async fn image_exists(&self, reference: &str) -> Result<bool, Self::Error>;
    async fn pull_image(&self, reference: &str) -> Result<(), Self::Error>;

    // Networks — one IPv6 /64 per pod instance.
    // The host bridge is assigned ::1 (gateway) and ::2 (mount endpoint).
    async fn network_exists(&self, name: &str) -> Result<bool, Self::Error>;
    async fn create_network(&self, name: &str, prefix: Ipv6Net) -> Result<(), Self::Error>;
    async fn remove_network(&self, name: &str) -> Result<(), Self::Error>;

    // Volumes
    async fn volume_exists(&self, name: &str) -> Result<bool, Self::Error>;
    async fn create_volume(&self, name: &str) -> Result<(), Self::Error>;
    async fn remove_volume(&self, name: &str) -> Result<(), Self::Error>;

    // Forced cleanup (e.g. seedling crashed while container was running)
    async fn remove_container(&self, name: &str, force: bool) -> Result<(), Self::Error>;

    // Interactive exec (for BSL shell sessions)
    async fn exec(&self, name: &str, spec: ExecSpec) -> Result<ExecHandle, Self::Error>;
}

#[trait_variant::make(ProcessManager: Send)]
pub trait LocalProcessManager: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    // Transient units — container lifecycle; no unit file written to disk.
    async fn start_transient(&self, spec: TransientUnitSpec) -> Result<(), Self::Error>;
    /// Sends the stop signal; returns immediately without waiting.
    /// Use `wait_unit_stopped` to block until the unit has fully stopped.
    async fn stop_unit(&self, name: &str) -> Result<(), Self::Error>;
    /// Polls until the unit reaches an inactive or failed state, or the
    /// timeout elapses. Required before removing pod networks or volumes.
    async fn wait_unit_stopped(&self, name: &str, timeout: Duration) -> Result<(), Self::Error>;
    async fn unit_state(&self, name: &str) -> Result<Option<UnitState>, Self::Error>;
    async fn list_units(&self, prefix: &str) -> Result<Vec<UnitSummary>, Self::Error>;

    // Persistent units — written to the unit drop-in path.
    async fn write_unit(&self, name: &str, content: &str) -> Result<(), Self::Error>;
    async fn remove_unit(&self, name: &str) -> Result<(), Self::Error>;
    async fn daemon_reload(&self) -> Result<(), Self::Error>;
    async fn start_unit(&self, name: &str) -> Result<(), Self::Error>;
}

#[trait_variant::make(NetworkProxy: Send)]
pub trait LocalNetworkProxy: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn is_healthy(&self) -> Result<bool, Self::Error>;
    async fn apply_config(&self, config: &ProxyConfig) -> Result<(), Self::Error>;
}

#[trait_variant::make(DataPlane: Send)]
pub trait LocalDataPlane: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Atomically replace the complete nftables rule set in `seedling_net`.
    /// Idempotent. Covers ingress DNAT, FORWARD policy, and mount DNAT6.
    async fn apply_rules(&self, rules: &DataPlaneRules) -> Result<(), Self::Error>;

    /// Replace the complete set of IPv6 service routes in the routing table.
    /// Each route maps a service IP to one or more pod instance IPs (ECMP).
    async fn apply_routes(&self, routes: &[ServiceRoute]) -> Result<(), Self::Error>;

    /// Remove all rules and routes owned by seedling. Called on shutdown.
    async fn clear_all(&self) -> Result<(), Self::Error>;
}

// ---------------------------------------------------------------------------
// SystemDriver
// ---------------------------------------------------------------------------

pub struct SystemDriver<C, P, N, D> {
    pub container: C,
    pub process: P,
    pub proxy: N,
    pub data_plane: D,
}
