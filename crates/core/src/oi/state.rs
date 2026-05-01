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
    pub db: crate::runtime::db::DbHandle,
    pub scheduler: Arc<parking_lot::Mutex<crate::runtime::Scheduler>>,
    pub tick_notify: Arc<tokio::sync::Notify>,
    pub db_path: PathBuf,
    /// In-memory set of authorized client SPKI fingerprints, shared with the
    /// TLS client cert verifier so additions/removals take effect immediately.
    pub trusted_keys: Arc<parking_lot::RwLock<HashSet<String>>>,
    pub shells: Arc<crate::oi::shells::ShellRegistry>,
    pub forwards: Arc<parking_lot::Mutex<ForwardRegistry>>,
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
}
