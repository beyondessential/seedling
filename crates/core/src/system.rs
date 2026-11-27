use std::{collections::HashMap, path::Path, sync::Arc};

use ipnet::{Ipv4Net, Ipv6Net};
use sha2::{Digest, Sha256};

use crate::system::{
    priority::AppStandings,
    types::{
        ContainerFilter, ContainerSpec, ContainerState, ContainerSummary, DataPlaneRules,
        ExecHandle, ExecOutput, ImageSummary, NetworkSummary, ProxyConfig, ServiceRoute,
        TransientUnitSpec, UnitState, UnitSummary,
    },
};

pub mod actuator;
pub mod backends;
pub mod host_probes;
pub mod isolation;
pub mod nat64;
pub mod observer;
pub mod priority;
pub mod reconcile;
pub mod translate;
pub mod types;

pub mod breadcrumb;
pub(crate) mod caddy;
pub mod journal;
pub mod resolver;
pub mod startup_cleanup;
pub mod stub;
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
    first_host_v6(&caddy::proxy_network_prefix(node_prefix)).expect("the proxy network is a /64")
}

/// The gateway of a bridge network the runtime creates: the first host
/// address of its prefix. Network creation assigns it, and the host binds
/// services on it, so both sides use this one derivation.
///
/// Follows from the prefix rather than from where an allocator happens to put
/// the host part. `None` for a prefix too small to hold a gateway and a host.
// spec: INFRA#pod-networks
pub fn first_host_v6(prefix: &Ipv6Net) -> Option<std::net::Ipv6Addr> {
    (prefix.prefix_len() <= 126).then(|| std::net::Ipv6Addr::from(u128::from(prefix.network()) + 1))
}

/// The start of every pod network's bridge name. Packet filter rules match pod
/// bridges by it.
pub const POD_BRIDGE_PREFIX: &str = "sdp-";

/// The bridge name of the proxy network.
pub const PROXY_BRIDGE: &str = "sdl-proxy";

/// The bridge name of the resolver network.
pub const RESOLVER_BRIDGE: &str = "sdl-resolver";

/// The bridge name of the pod network holding `prefix`: [`POD_BRIDGE_PREFIX`]
/// and the /64's subnet ID, the bits between the host's /48 and the /64, as
/// four hex digits.
///
/// Unique because the prefix is allocated and recorded, and within Linux's
/// 15-character limit on interface names.
pub fn pod_bridge_name(prefix: &Ipv6Net) -> String {
    format!("{POD_BRIDGE_PREFIX}{:04x}", prefix.network().segments()[3])
}

// spec: INFRA#pod-networks
pub fn first_host_v4(prefix: &ipnet::Ipv4Net) -> Option<std::net::Ipv4Addr> {
    (prefix.prefix_len() <= 30).then(|| std::net::Ipv4Addr::from(u32::from(prefix.network()) + 1))
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
pub use futures_util::future::BoxFuture;

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
    /// Create the network with its bridge interface named `bridge_name`, and
    /// return the bridge name the runtime reports for it.
    fn create_network<'a>(
        &'a self,
        name: &'a str,
        bridge_name: &'a str,
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

    // spec: RT#signal-delivery
    /// Send a POSIX signal to the running container's PID 1.
    /// Returns `Ok(true)` when the signal was delivered, `Ok(false)` when the
    /// container did not exist (already terminated). Other errors propagate.
    fn signal_container<'a>(
        &'a self,
        name: &'a str,
        signal: &'a str,
    ) -> BoxFuture<'a, Result<bool, BoxError>>;

    // spec: RT#command-execution
    /// Run a command (`argv`) inside the running container `name` and wait
    /// for it to exit. `extra_env` is layered on top of the container's
    /// environment. Returns what the command wrote and its exit code; the
    /// caller records the output with the container's logs.
    fn exec_command<'a>(
        &'a self,
        name: &'a str,
        argv: &'a [String],
        extra_env: &'a [(String, String)],
    ) -> BoxFuture<'a, Result<ExecOutput, BoxError>>;
}

pub trait ProcessManager: Send + Sync + 'static {
    // Transient units — container lifecycle; no unit file written to disk.
    /// Start a transient unit. The process manager paces the unit's automatic
    /// restarts, whatever its standing.
    // spec: AUTO#restart-pacing
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

    // Resource-control groups — the grouping the runtime owns.
    /// Make the resource-control groups this runtime owns provide for `apps`
    /// and for the runtime's infrastructure, creating, reweighting and
    /// forgetting as needed.
    ///
    /// Takes the whole set rather than one app at a time because the
    /// supervisor may re-read its configuration as global work: deciding once
    /// for the batch costs a single re-read instead of one per group. The
    /// implementation compares against what already exists, so a call that
    /// changes nothing costs nothing.
    ///
    /// `prune` is false when the caller could not account for every app this
    /// tick, so a short `apps` set never forgets a live app's group.
    // spec: PRIO#resource-control-groups
    fn sync_groups<'a>(
        &'a self,
        apps: Vec<AppStandings>,
        prune: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>>;

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

    /// When the proxy last refused a session of each DTLS-terminating ingress,
    /// by ingress name, for want of a flow to its service. An error is "could
    /// not be read", which is not "none refused".
    // spec: PRX#dtls-terminating-ingresses
    fn dtls_refusals<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<std::collections::BTreeMap<String, jiff::Timestamp>, BoxError>>;
}

pub trait DataPlane: Send + Sync + 'static {
    /// Atomically replace the packet rules the runtime holds as absolute
    /// state. Idempotent. Covers ingress redirection, service translation and
    /// mount translation.
    fn apply_rules<'a>(&'a self, rules: &'a DataPlaneRules) -> BoxFuture<'a, Result<(), BoxError>>;

    /// Replace the complete set of IPv6 service routes in the routing table.
    /// Each route maps a service IP to one or more pod instance IPs (ECMP).
    fn apply_routes<'a>(
        &'a self,
        routes: &'a [ServiceRoute],
    ) -> BoxFuture<'a, Result<(), BoxError>>;

    /// Install the isolation rules, which keep pods apart from one another
    /// and from the host, replacing them atomically if they are installed.
    /// Idempotent. Nothing removes them: workload containers outlive the
    /// runtime.
    fn ensure_isolation<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>>;

    /// Remove the rules held as absolute state, and the routes. Called on
    /// shutdown.
    fn clear_all<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>>;
}

/// The host's NAT64 translator, which the runtime installs while workloads
/// need it and removes when the host goes idle.
// spec: NAT64#translator-lifecycle
pub trait AddressTranslator: Send + Sync + 'static {
    /// Install the translator. Setting up one already installed is not an
    /// error.
    fn set_up(&self) -> BoxFuture<'_, Result<(), BoxError>>;

    /// Remove the translator. Removing one that is not installed is not an
    /// error.
    fn tear_down(&self) -> BoxFuture<'_, Result<(), BoxError>>;
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
    /// Where log streams are read from.
    pub journal: Arc<dyn journal::JournalSource>,
    /// The host's NAT64 translator.
    pub translator: Arc<dyn AddressTranslator>,
    /// Whether the container runtime and process manager are reachable.
    /// While either is not, the daemon runs degraded: the
    /// backend is an erroring stub (see [`unavailable`]), the OI, database and
    /// reconciler run so the operator can see the fault, and no workloads are
    /// actuated. The reconciler probes the backends again, and
    /// [`Self::container`] and [`Self::process`] become the working ones when
    /// a probe reaches them.
    pub backends: backends::Backends,
    /// Whether the isolation rules are installed. While they are not, no
    /// workload container and no shell session starts.
    pub isolation: isolation::Isolation,
}

impl System {
    /// Install the isolation rules unless they are installed, or the delay
    /// since the last failed install has yet to pass.
    pub async fn ensure_isolation(&self) -> Result<(), isolation::IsolationMissing> {
        self.isolation.ensure(&*self.data_plane).await
    }

    /// The reason the daemon is running in degraded mode, if it is.
    pub fn degraded_reason(&self) -> Option<String> {
        self.backends.unavailable_reason()
    }

    /// Probe an unavailable backend again, if the delay since the last probe
    /// has passed, and say why one is still unavailable.
    pub async fn probe_backends(&self) -> Option<String> {
        self.backends.probe_unavailable().await
    }
}

// spec: INFRA#host-prefix-derivation
/// Derive the node's /48 ULA prefix from the host's machine identifier, as
/// the platform reads it.
///
/// The identifier (whitespace-trimmed) is hashed with SHA-256; the first four
/// bytes of the digest fill octets 2–5 of the prefix:
///
/// ```text
/// fd5e : <hash[0]><hash[1]> : <hash[2]><hash[3]> :: /48
/// ```
///
/// Hashing instead of direct interpretation means the derivation is
/// agnostic to the identifier's format (plain hex, UUID with dashes, etc.).
pub fn node_prefix_from_machine_id(machine_id: &str) -> std::io::Result<Ipv6Net> {
    let trimmed = machine_id.trim();
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

/// What the host platform provides the runtime with, beyond what the runtime
/// builds itself.
pub struct Platform {
    pub connector: Arc<dyn backends::Connector>,
    pub data_plane: Arc<dyn DataPlane>,
    pub journal: Arc<dyn journal::JournalSource>,
    pub storage: Arc<dyn volume_store::VolumeStorage>,
    pub translator: Arc<dyn AddressTranslator>,
}

/// Result of `System::setup` / `System::setup_stubbed`: the assembled system
/// handle alongside the live Caddy admin client (or a placeholder under stub
/// mode).
pub type SystemSetup = (Arc<System>, Arc<tokio::sync::RwLock<reqwest::Client>>);

impl System {
    // spec: PRX#proxy-lifecycle
    /// Initialize all system backends, ensure Caddy is running, and return the
    /// assembled `System` handle alongside the Caddy admin client handle.
    ///
    /// `recorded_pod_prefixes` is the prefix recorded for each pod network, or
    /// `None` when the records could not be read; see
    /// [`startup_cleanup::replace_misnamed_networks`].
    ///
    /// Errors from individual backends are returned as boxed trait objects so
    /// callers do not need to depend on internal error types.
    pub async fn setup(
        platform: Platform,
        node_prefix: Ipv6Net,
        data_dir: &Path,
        recorded_pod_prefixes: Option<HashMap<String, Ipv6Net>>,
    ) -> Result<SystemSetup, BoxError> {
        // spec: INFRA#pod-network-sources
        // spec: INFRA#forwarding-policy
        // spec: INFRA#pod-traffic-to-the-host
        // Before anything that might start a container on a pod network. A
        // failure is retried from the reconciliation loop, which starts no
        // workload until an install succeeds.
        let isolation = isolation::Isolation::default();
        let _ = isolation.ensure(&*platform.data_plane).await;

        // spec: INFRA#unavailable-backends — bring up the container runtime
        // and process manager best-effort. If either connection fails
        // the daemon must NOT crash-loop: either being down is a
        // recoverable host-configuration problem, and a crash-loop hides it
        // behind restart churn with no operator-visible signal. Instead we swap
        // in erroring backends, record the reason, and let the OI, database, and
        // reconciler come up so the operator sees a system-wide fault explaining
        // what is broken.
        let connector = platform.connector;
        let backends = backends::Backends::new(
            Arc::clone(&connector),
            connector.container().await,
            connector.process().await,
        );
        if let Some(reason) = backends.unavailable_reason() {
            tracing::error!(
                "{reason} — starting in degraded mode; the operator interface will \
                 report a system fault and no workloads will be actuated until the \
                 backend is reachable again"
            );
        }
        let (container, process) = (backends.container(), backends.process());

        // Before Caddy comes up, so that a proxy network it would otherwise
        // run on is replaced first.
        if backends.unavailable_reason().is_none() {
            startup_cleanup::replace_misnamed_networks(&*container, recorded_pod_prefixes.as_ref())
                .await;
        }

        // spec: PRX#proxy-lifecycle — bring Caddy up best-effort. A failure
        // here (image build, container start, health check) must NOT take down
        // the daemon: Caddy is workload-ingress infrastructure, and the
        // reconciler re-runs ensure_caddy_running every tick — filing a
        // `caddy_failed` fault and swapping the admin client to the live socket
        // once it succeeds. So on failure we start with a placeholder client
        // and let the reconciler heal it; the OI, DB, and scheduler come up
        // regardless and the operator sees the fault.
        let caddy_proxy = if backends.unavailable_reason().is_some() {
            // No point trying to build/run Caddy through the unavailable
            // container runtime — it would only error. Start with a placeholder;
            // the reconciler's degraded guard files the overarching fault.
            Arc::new(caddy::CaddyProxy::placeholder()?)
        } else {
            match caddy::ensure_caddy_running(&*container, &*process, &node_prefix, data_dir).await
            {
                Ok(initial) => Arc::new(caddy::CaddyProxy::new(&initial.addrs.admin_socket)?),
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

        let volume_store = volume_store::VolumeStore::new(data_dir, platform.storage)?;

        let system = Arc::new(Self {
            container,
            process,
            proxy,
            data_plane: platform.data_plane,
            volume_store,
            journal: platform.journal,
            translator: platform.translator,
            backends,
            isolation,
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
    /// individual subsystems. Volumes are kept in `storage` and logs are read
    /// from `journal`, so the daemon can still keep volumes as the host does
    /// and show the breadcrumbs it writes.
    pub fn setup_stubbed(
        data_dir: &Path,
        storage: Arc<dyn volume_store::VolumeStorage>,
        journal: Arc<dyn journal::JournalSource>,
    ) -> Result<SystemSetup, BoxError> {
        let volumes_root = data_dir.join("stub-volumes");
        std::fs::create_dir_all(&volumes_root)?;

        let container = Arc::new(stub::StubContainerRuntime::new(volumes_root));
        let process = Arc::new(stub::StubProcessManager::new(Arc::clone(&container)));
        let data_plane_arc: Arc<dyn DataPlane> = Arc::new(stub::StubDataPlane::default());
        let proxy: Arc<dyn NetworkProxy> = Arc::new(stub::StubNetworkProxy);

        let volume_store = volume_store::VolumeStore::new(data_dir, storage)?;

        // The caddy_admin_client field on Reconciler holds the live HTTP
        // client used during blue/green Caddy upgrades. Stub mode never
        // upgrades Caddy, so a placeholder client suffices.
        let placeholder_client = reqwest::Client::builder()
            .build()
            .expect("default reqwest client builds");
        let caddy_admin_client = Arc::new(tokio::sync::RwLock::new(placeholder_client));

        let backends = backends::Backends::working(
            container as Arc<dyn ContainerRuntime>,
            process as Arc<dyn ProcessManager>,
        );
        let system = Arc::new(Self {
            container: backends.container(),
            process: backends.process(),
            proxy,
            data_plane: data_plane_arc,
            volume_store,
            journal,
            translator: Arc::new(stub::StubTranslator),
            backends,
            isolation: isolation::Isolation::default(),
        });
        Ok((system, caddy_admin_client))
    }

    /// A `System` whose container runtime and process manager fail every call,
    /// as they do when neither could be reached at startup and the daemon came
    /// up in degraded mode.
    ///
    /// The counterpart to [`Self::setup_stubbed`], which fakes success: this
    /// one lets a test exercise what the daemon does when the world will not
    /// answer, rather than only what it does when the world agrees.
    pub fn setup_unavailable(data_dir: &Path) -> Result<SystemSetup, BoxError> {
        let volume_store =
            volume_store::VolumeStore::new(data_dir, Arc::new(stub::StubVolumeStorage))?;
        let placeholder_client = reqwest::Client::builder()
            .build()
            .expect("default reqwest client builds");
        let caddy_admin_client = Arc::new(tokio::sync::RwLock::new(placeholder_client));

        let backends = backends::Backends::unreachable("backends unavailable");
        let system = Arc::new(Self {
            container: backends.container(),
            process: backends.process(),
            proxy: Arc::new(stub::StubNetworkProxy),
            data_plane: Arc::new(stub::StubDataPlane::default()),
            volume_store,
            journal: Arc::new(stub::StubJournal),
            translator: Arc::new(stub::StubTranslator),
            backends,
            isolation: isolation::Isolation::default(),
        });
        Ok((system, caddy_admin_client))
    }

    /// A `System` on `backends` and `proxy`, with stubs for everything else,
    /// for tests that need a backend of their own.
    #[cfg(test)]
    pub(crate) fn for_tests(
        backends: backends::Backends,
        proxy: Arc<dyn NetworkProxy>,
        data_dir: &Path,
    ) -> Self {
        Self {
            container: backends.container(),
            process: backends.process(),
            proxy,
            data_plane: Arc::new(stub::StubDataPlane::default()),
            volume_store: volume_store::VolumeStore::new(
                data_dir,
                Arc::new(stub::StubVolumeStorage),
            )
            .expect("the volume store opens"),
            journal: Arc::new(stub::StubJournal),
            translator: Arc::new(stub::StubTranslator),
            backends,
            isolation: isolation::Isolation::default(),
        }
    }
}

#[cfg(test)]
mod gateway_tests {
    use super::{first_host_v4, first_host_v6};

    // spec: INFRA#pod-networks
    #[test]
    fn the_gateway_is_the_first_host_address_of_the_prefix() {
        let gw = |p: &str| first_host_v6(&p.parse().unwrap());
        assert_eq!(
            gw("fd5e:ed12:3456:4a07::/64"),
            Some("fd5e:ed12:3456:4a07::1".parse().unwrap())
        );
        // Carved from one /64, two networks do not share a gateway.
        assert_ne!(
            gw("fd5e:ed12:3456:4a07::/80"),
            gw("fd5e:ed12:3456:4a07:1::/80")
        );
        // The gateway lies inside the subnet, off its network address.
        let p: ipnet::Ipv6Net = "fd5e:ed12:3456:4a07:1::/80".parse().unwrap();
        let g = first_host_v6(&p).unwrap();
        assert!(p.contains(&g) && g != p.network());
        assert_eq!(
            first_host_v4(&"10.89.4.0/24".parse().unwrap()),
            Some("10.89.4.1".parse().unwrap())
        );
    }

    // spec: INFRA#pod-networks
    #[test]
    fn a_prefix_with_no_room_for_a_gateway_is_refused() {
        assert_eq!(first_host_v6(&"fd5e::1/128".parse().unwrap()), None);
        assert_eq!(first_host_v6(&"fd5e::/127".parse().unwrap()), None);
        assert_eq!(first_host_v4(&"10.0.0.0/31".parse().unwrap()), None);
    }

    // The proxy and resolver bridges are created like any pod network, so the
    // address the host binds on is the gateway that creation assigns.
    #[test]
    fn infra_bridge_gateways_are_their_networks_first_host() {
        let node: ipnet::Ipv6Net = "fd5e:ed12:3456::/48".parse().unwrap();
        assert_eq!(
            super::proxy_bridge_gateway(&node),
            "fd5e:ed12:3456:ff00::1"
                .parse::<std::net::Ipv6Addr>()
                .unwrap()
        );
        assert_eq!(
            super::resolver::resolver_gateway_addr(&node),
            "fd5e:ed12:3456:fd00::1"
                .parse::<std::net::Ipv6Addr>()
                .unwrap()
        );
    }
}

#[cfg(test)]
mod bridge_name_tests {
    use super::{POD_BRIDGE_PREFIX, PROXY_BRIDGE, RESOLVER_BRIDGE, pod_bridge_name};

    /// Linux refuses interface names longer than this.
    const IFNAMSIZ_CHARS: usize = 15;

    #[test]
    fn a_pod_bridge_is_named_after_its_subnet_id() {
        let name = |p: &str| pod_bridge_name(&p.parse().unwrap());
        assert_eq!(name("fd5e:1234:5678:0a1b::/64"), "sdp-0a1b");
        assert_eq!(name("fd5e:1234:5678:4::/64"), "sdp-0004");
        assert_eq!(name("fd5e:1234:5678:ffff::/64"), "sdp-ffff");
        assert_ne!(
            name("fd5e:1234:5678:0400::/64"),
            name("fd5e:1234:5678:0401::/64")
        );
    }

    #[test]
    fn every_bridge_name_fits_an_interface_name() {
        let widest = pod_bridge_name(&"fd5e:1234:5678:ffff::/64".parse().unwrap());
        for name in [widest.as_str(), PROXY_BRIDGE, RESOLVER_BRIDGE] {
            assert!(name.len() <= IFNAMSIZ_CHARS, "{name} is too long");
        }
        assert!(widest.starts_with(POD_BRIDGE_PREFIX));
        assert!(!PROXY_BRIDGE.starts_with(POD_BRIDGE_PREFIX));
        assert!(!RESOLVER_BRIDGE.starts_with(POD_BRIDGE_PREFIX));
    }
}

#[cfg(test)]
mod node_prefix_tests {
    use super::node_prefix_from_machine_id;

    // spec: INFRA#host-prefix-derivation
    #[test]
    fn the_prefix_is_a_ula_48_derived_from_the_trimmed_identifier() {
        let prefix = node_prefix_from_machine_id("0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(prefix.prefix_len(), 48);
        assert_eq!(prefix.network().segments()[0], 0xfd5e);
        assert_eq!(
            node_prefix_from_machine_id("  0123456789abcdef0123456789abcdef\n").unwrap(),
            prefix
        );
        assert_ne!(
            node_prefix_from_machine_id("fedcba9876543210fedcba9876543210").unwrap(),
            prefix
        );
    }

    // spec: INFRA#host-prefix-derivation
    #[test]
    fn an_empty_identifier_is_refused() {
        assert!(node_prefix_from_machine_id(" \n").is_err());
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
