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
    pub db: Arc<parking_lot::Mutex<crate::runtime::db::Db>>,
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
    pub event_tx: crate::oi::events::EventSender,
    pub script_limits: crate::ScriptLimits,
    /// DNS servers injected into workload containers' /etc/resolv.conf.
    pub dns_servers: Vec<std::net::Ipv6Addr>,
}
