use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Instant,
};

use parking_lot::RwLock;

use crate::runtime::apps::AppRegistry;

use super::forwards::ForwardRegistry;

/// Shared state for all OI request handlers.
pub struct OiState {
    pub registry: Arc<RwLock<AppRegistry>>,
    /// Set once by the server after key generation; never changes after that.
    pub spki_fingerprint: OnceLock<String>,
    pub start_time: Instant,
    /// When the daemon started, on the wall clock.
    pub started_at: jiff::Timestamp,
    pub db: crate::runtime::db::DbHandle,
    pub scheduler: Arc<parking_lot::Mutex<crate::runtime::Scheduler>>,
    pub tick_notify: Arc<tokio::sync::Notify>,
    pub db_path: PathBuf,
    /// In-memory set of authorized client SPKI fingerprints, shared with the
    /// TLS client cert verifier so additions/removals take effect immediately.
    pub trusted_keys: Arc<parking_lot::RwLock<HashSet<String>>>,
    pub shells: Arc<crate::oi::shells::ShellRegistry>,
    pub forwards: Arc<parking_lot::Mutex<ForwardRegistry>>,
    /// Live offers to carry Canopy requests, and the settings governing them.
    // i[impl canopy.offer]
    pub canopy: Arc<crate::oi::canopy::CanopyState>,
    pub container_runtime: Arc<dyn crate::system::ContainerRuntime>,
    pub driver: Arc<crate::system::System>,
    /// Node-wide /48 IPv6 prefix, used to derive pod network addresses for
    /// shell session containers.
    pub node_prefix: ipnet::Ipv6Net,
    pub event_tx: seedling_protocol::events::EventSender,
    pub script_limits: crate::ScriptLimits,
    /// DNS servers injected into workload containers' /etc/resolv.conf.
    pub dns_servers: Vec<std::net::Ipv6Addr>,
    // r[impl secret.key]
    pub cipher: Arc<crate::runtime::secrets::Cipher>,
    /// TLS issuance coordinator. The OI uses it to run operator-driven
    /// `issue-acme-dns` calls and to enqueue retries with the persistent
    /// force-retry signal.
    // r[impl tls.cert.eager-issuance]
    pub tls_coordinator: Arc<crate::runtime::tls::issuance::Coordinator>,
    /// Host-filesystem path of the Caddy data volume. Resolved lazily on
    /// the first OI request that needs to inspect Caddy's certificate
    /// cache (i.e. the TLS hostname rollup).
    // r[impl tls.cert.hostname-view]
    pub caddy_data_path: tokio::sync::OnceCell<PathBuf>,
    /// Tailscale discovery provider. `None` when Tailscale is not in use
    /// at all (e.g. in test harnesses); the discovery handler returns an
    /// empty status in that case.
    // r[impl ingress.site.tailscale]
    pub tailscale_provider: Option<Arc<crate::runtime::tailscale::TailscaleProvider>>,
    /// Background resolver for site-service DNS endpoints. Surfaced through
    /// the OI so handlers can refresh on add/remove and serve cache
    /// snapshots to operators. `None` in test harnesses.
    // r[impl service.site.address]
    pub site_resolver: Option<Arc<crate::runtime::site_services::resolver::SiteServiceResolver>>,
    /// The OCI registry client definitions are fetched through.
    // i[impl definition.fetch]
    pub definition_registry: crate::runtime::definition::fetch::SharedRegistry,
}

/// The daemon's state as the source of the facts a TLS chain depends on.
///
/// Weak, because the state holds the coordinator that holds this.
pub struct OiEnvironment(std::sync::Weak<OiState>);

impl OiEnvironment {
    pub fn new(state: &Arc<OiState>) -> Self {
        Self(Arc::downgrade(state))
    }
}

impl crate::runtime::tls::env::Environment for OiEnvironment {
    fn gather(
        &self,
    ) -> Option<(
        Vec<crate::runtime::tls::state::ManagedIngress>,
        crate::runtime::tls::state::ChainEnv,
    )> {
        let state = self.0.upgrade()?;
        let managed = {
            let registry = state.registry.read();
            crate::runtime::tls::state::managed_ingresses(&registry)
        };
        match state.chain_env(&managed) {
            Ok(env) => Some((managed, env)),
            Err(e) => {
                tracing::warn!(error = %e, "cannot read the facts a TLS chain depends on");
                None
            }
        }
    }

    fn holding(&self, hostname: &str) -> crate::runtime::tls::state::ProxyHolding {
        let Some(state) = self.0.upgrade() else {
            return crate::runtime::tls::state::ProxyHolding::Unreadable;
        };
        crate::runtime::tls::env::proxy_holding(
            state.resolve_caddy_data_path().as_deref(),
            hostname,
        )
    }
}

impl OiState {
    /// Whether Canopy access is enabled and a client is offering to carry
    /// requests.
    pub fn canopy_offer_live(&self) -> bool {
        self.canopy.is_enabled() && self.canopy.current().is_some()
    }

    /// Resolve (and cache) the host filesystem path of the Caddy data volume.
    /// Returns `None` if the volume can't be located right now, such as before
    /// Caddy has started or when the container driver's mount-point query
    /// failed. The cell is one-shot success; transient failures retry on the
    /// next call.
    pub fn resolve_caddy_data_path(&self) -> Option<PathBuf> {
        if let Some(p) = self.caddy_data_path.get() {
            return Some(p.clone());
        }
        let runtime = Arc::clone(&self.container_runtime);
        let resolved: Result<PathBuf, _> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                runtime
                    .volume_mountpoint(crate::system::caddy::CADDY_DATA_VOLUME)
                    .await
            })
        });
        match resolved {
            Ok(p) => {
                // Best-effort fill; if another caller raced ahead and won we
                // use whatever they stored.
                let _ = self.caddy_data_path.set(p.clone());
                Some(p)
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "caddy data volume unresolved; the proxy's certificates cannot be read"
                );
                None
            }
        }
    }

    /// The facts a hostname's strategy chain depends on, for an operator's view
    /// of them: the database read afresh, and what the proxy holds taken from
    /// the reading the coordinator already has.
    ///
    /// The proxy's certificates are read off disk and parsed per hostname, which
    /// is too much for a page that polls; they change on the scale of a renewal,
    /// so the coordinator's is as good an answer. What is stored is read fresh,
    /// since that is what an operator watches change.
    pub fn chain_env_for_display(
        &self,
        managed: &[crate::runtime::tls::state::ManagedIngress],
    ) -> rusqlite::Result<crate::runtime::tls::state::ChainEnv> {
        match self.tls_coordinator.proxy_holdings() {
            Some(proxy) => {
                let offer_live = self.canopy_offer_live();
                let managed = managed.to_vec();
                self.db.call(move |db| {
                    crate::runtime::tls::env::collect(db, &managed, offer_live, proxy)
                })
            }
            // The coordinator has none to give, so they are gathered here as
            // the runtime's own callers do.
            None => self.chain_env(managed),
        }
    }

    /// The facts a hostname's strategy chain depends on, for the TLS-terminating
    /// ingresses `managed`.
    pub fn chain_env(
        &self,
        managed: &[crate::runtime::tls::state::ManagedIngress],
    ) -> rusqlite::Result<crate::runtime::tls::state::ChainEnv> {
        let offer_live = self.canopy_offer_live();
        // The proxy's cache is read here, outside the database thread: see
        // `env::proxy_holdings`.
        let proxy = crate::runtime::tls::env::proxy_holdings(
            managed,
            self.resolve_caddy_data_path().as_deref(),
        );
        let managed = managed.to_vec();
        self.db
            .call(move |db| crate::runtime::tls::env::collect(db, &managed, offer_live, proxy))
    }
}
