use std::{path::Path, sync::Arc};

use ipnet::{Ipv4Net, Ipv6Net};
use sha2::{Digest, Sha256};

use crate::system::types::{
    ContainerFilter, ContainerSpec, ContainerState, ContainerSummary, DataPlaneRules, ExecHandle,
    ImageSummary, NetworkSummary, ProxyConfig, ServiceRoute, TransientUnitSpec, UnitState,
    UnitSummary,
};

pub mod actuator;
pub mod nat64;
pub mod netinfo;
pub mod observer;
pub mod reconcile;
pub mod translate;
pub mod types;

pub mod breadcrumb;
pub(crate) mod caddy;
pub(crate) mod confined_write;
pub(crate) mod data_plane;
pub mod jool;
pub(crate) mod journal;
pub(crate) mod podman;
pub mod resolver;
pub mod stub;
pub(crate) mod systemd;
pub(crate) mod unavailable;
pub mod volume_store;

pub use actuator::{ActuateError, Actuator, TMPFS_VOLUMES_DIR};
pub use observer::{ObserveError, Observer};

/// Returns whether `network_name` is one of the infrastructure networks
/// managed by the daemon's own ensure_*_running paths (Caddy proxy,
/// CoreDNS resolver, mount-namespace nets) rather than a per-app pod
/// network. The daemon's startup orphan-network sweep uses this to skip
/// infra networks: their containers may legitimately be down at the
/// moment the sweep runs (we're about to recreate them on the first
/// reconciler tick), so name-prefix correlation against live containers
/// would mistake them for orphans and tear them down.
pub fn is_infra_network(network_name: &str) -> bool {
    network_name == caddy::PROXY_NETWORK
        || network_name == resolver::RESOLVER_NETWORK
        || network_name.starts_with("seedling-mount-")
}

/// Returns the host-side bridge gateway IP for the seedling-proxy network,
/// derived from the node's /48 prefix. The daemon binds local services that
/// only Caddy + host processes should reach (e.g. the TLS cert endpoint) to
/// this address so that workload pods on other /64s cannot route to them.
pub fn proxy_bridge_gateway(node_prefix: &Ipv6Net) -> std::net::Ipv6Addr {
    let proxy_net = caddy::proxy_network_prefix(node_prefix);
    let mut bytes = proxy_net.network().octets();
    bytes[15] = 1;
    std::net::Ipv6Addr::from(bytes)
}
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
    /// Enumerate every image in local storage, with size and references.
    fn list_images<'a>(&'a self) -> BoxFuture<'a, Result<Vec<ImageSummary>, BoxError>>;
    /// Remove an image by reference (tag, digest ref, or image ID).
    /// Returns `true` when the image was removed, `false` when it was not
    /// present locally.
    fn remove_image<'a>(
        &'a self,
        reference: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<bool, BoxError>>;

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

    // l[impl rt.signal]
    /// Send a POSIX signal to the running container's PID 1.
    /// Returns `Ok(true)` when the signal was delivered, `Ok(false)` when the
    /// container did not exist (already terminated). Other errors propagate.
    fn signal_container<'a>(
        &'a self,
        name: &'a str,
        signal: &'a str,
    ) -> BoxFuture<'a, Result<bool, BoxError>>;

    // l[impl rt.exec]
    /// Run a command (`argv`) inside the running container `name` and wait
    /// for it to exit. `extra_env` is layered on top of the container's
    /// environment. Stdout and stderr are forwarded to the container's log
    /// sink, not captured. Returns the command's exit code.
    fn exec_command<'a>(
        &'a self,
        name: &'a str,
        argv: &'a [String],
        extra_env: &'a [(String, String)],
    ) -> BoxFuture<'a, Result<i32, BoxError>>;
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
    /// `Some(reason)` when the container runtime (podman) or process manager
    /// (systemd) could not be reached at startup and the daemon came up in a
    /// degraded mode. In this state the backends are erroring stubs (see
    /// [`unavailable`]): the OI, database, and reconciler run so the operator
    /// can see the fault, but no workloads are actuated. `None` in normal
    /// operation.
    pub degraded: Option<String>,
}

impl System {
    /// The reason the daemon is running in degraded mode, if it is. See
    /// [`System::degraded`].
    pub fn degraded_reason(&self) -> Option<&str> {
        self.degraded.as_deref()
    }
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

/// Result of `System::setup` / `System::setup_stubbed`: the assembled system
/// handle alongside the live Caddy admin client (or a placeholder under stub
/// mode).
pub type SystemSetup = (Arc<System>, Arc<tokio::sync::RwLock<reqwest::Client>>);

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
    ) -> Result<SystemSetup, BoxError> {
        // r[impl infra.node.degraded] — bring up the container runtime (podman)
        // and process manager (systemd) best-effort. If either connection fails
        // the daemon must NOT crash-loop: podman/systemd being down is a
        // recoverable host-configuration problem, and a crash-loop hides it
        // behind restart churn with no operator-visible signal. Instead we swap
        // in erroring backends, record the reason, and let the OI, database, and
        // reconciler come up so the operator sees a system-wide fault explaining
        // what is broken.
        let podman = podman::PodmanRuntime::new().await;
        let systemd = systemd::SystemdManager::connect().await;
        let (container, process, degraded): (
            Arc<dyn ContainerRuntime>,
            Arc<dyn ProcessManager>,
            Option<String>,
        ) = match (podman, systemd) {
            (Ok(c), Ok(p)) => (Arc::new(c), Arc::new(p), None),
            (podman, systemd) => {
                let reason = match (&podman, &systemd) {
                    (Err(e), _) => format!("container runtime (podman) unavailable: {e}"),
                    (_, Err(e)) => format!("process manager (systemd) unavailable: {e}"),
                    (Ok(_), Ok(_)) => unreachable!("matched the all-Ok arm above"),
                };
                tracing::error!(
                    "{reason} — starting in degraded mode; the operator interface will \
                     report a system fault and no workloads will be actuated until the \
                     daemon is restarted with a working podman/systemd"
                );
                (
                    Arc::new(unavailable::UnavailableContainerRuntime),
                    Arc::new(unavailable::UnavailableProcessManager),
                    Some(reason),
                )
            }
        };
        let data_plane_arc: Arc<dyn DataPlane> =
            Arc::new(data_plane::NftablesDataPlane::new(node_prefix)?);

        // r[impl infra.proxy.startup] — bring Caddy up best-effort. A failure
        // here (image build, container start, health check) must NOT take down
        // the daemon: Caddy is workload-ingress infrastructure, and the
        // reconciler re-runs ensure_caddy_running every tick — filing a
        // `caddy_failed` fault and swapping the admin client to the live socket
        // once it succeeds. So on failure we start with a placeholder client
        // and let the reconciler heal it; the OI, DB, and scheduler come up
        // regardless and the operator sees the fault.
        let caddy_proxy = if degraded.is_some() {
            // No point trying to build/run Caddy through the unavailable
            // container runtime — it would only error. Start with a placeholder;
            // the reconciler's degraded guard files the overarching fault.
            Arc::new(caddy::CaddyProxy::placeholder()?)
        } else {
            match caddy::ensure_caddy_running(&*container, &*process, &node_prefix, data_dir).await
            {
                Ok(initial) => Arc::new(caddy::CaddyProxy::new(&initial.admin_socket)?),
                Err(e) => {
                    tracing::error!(
                        "initial Caddy bring-up failed: {e} — starting without ingress; \
                     the reconciler will retry and file a caddy_failed fault"
                    );
                    Arc::new(caddy::CaddyProxy::placeholder()?)
                }
            }
        };
        let caddy_admin_client = caddy_proxy.admin_client_handle();
        let proxy: Arc<dyn NetworkProxy> = caddy_proxy;

        let volume_store = volume_store::VolumeStore::new(data_dir, use_btrfs)?;

        let system = Arc::new(Self {
            container,
            process,
            proxy,
            data_plane: data_plane_arc,
            volume_store,
            degraded,
        });
        Ok((system, caddy_admin_client))
    }

    /// Setup variant that uses in-memory stubs for every backend. Lets the
    /// daemon boot without podman / systemd / nftables / Caddy. The
    /// `caddy_admin_client` is a vestigial reqwest::Client that points at
    /// nowhere — fine because the stub `NetworkProxy` doesn't use it. Every
    /// host-system effect is faked; OI handlers, reconciliation, DB, and
    /// event emission remain real. Intended for end-to-end UI tests
    /// (Playwright) and as a foundation for future integration tests of
    /// individual subsystems.
    pub fn setup_stubbed(data_dir: &Path, use_btrfs: bool) -> Result<SystemSetup, BoxError> {
        let volumes_root = data_dir.join("stub-volumes");
        std::fs::create_dir_all(&volumes_root)?;

        let container = Arc::new(stub::StubContainerRuntime::new(volumes_root));
        let process = Arc::new(stub::StubProcessManager::new(Arc::clone(&container)));
        let data_plane_arc: Arc<dyn DataPlane> = Arc::new(stub::StubDataPlane);
        let proxy: Arc<dyn NetworkProxy> = Arc::new(stub::StubNetworkProxy);

        let volume_store = volume_store::VolumeStore::new(data_dir, use_btrfs)?;

        // The caddy_admin_client field on Reconciler holds the live HTTP
        // client used during blue/green Caddy upgrades. Stub mode never
        // upgrades Caddy, so a placeholder client suffices.
        let placeholder_client = reqwest::Client::builder()
            .build()
            .expect("default reqwest client builds");
        let caddy_admin_client = Arc::new(tokio::sync::RwLock::new(placeholder_client));

        let system = Arc::new(Self {
            container: container as Arc<dyn ContainerRuntime>,
            process: process as Arc<dyn ProcessManager>,
            proxy,
            data_plane: data_plane_arc,
            volume_store,
            degraded: None,
        });
        Ok((system, caddy_admin_client))
    }
}

#[cfg(test)]
mod is_infra_network_tests {
    use super::*;

    #[test]
    fn excludes_proxy_and_resolver_networks_by_exact_name() {
        // Both networks are bare ("seedling-proxy", "seedling-resolver");
        // earlier filters using "seedling-caddy-" / "seedling-resolver-"
        // prefixes did not match these names — letting the orphan sweep
        // delete them on every startup.
        assert!(is_infra_network("seedling-proxy"));
        assert!(is_infra_network("seedling-resolver"));
    }

    #[test]
    fn excludes_mount_networks_by_prefix() {
        assert!(is_infra_network("seedling-mount-foo"));
        assert!(is_infra_network("seedling-mount-"));
    }

    #[test]
    fn accepts_app_pod_networks_as_non_infra() {
        assert!(!is_infra_network("seedling-myapp"));
        assert!(!is_infra_network(
            "seedling-postgres-tamanu-postgres-0746d008"
        ));
    }

    #[test]
    fn does_not_match_infra_container_names() {
        // The container slot names used to be the basis of the prefix
        // filter; they must not be confused with the network names.
        assert!(!is_infra_network("seedling-resolver-blue"));
        assert!(!is_infra_network("seedling-caddy-green"));
    }
}
