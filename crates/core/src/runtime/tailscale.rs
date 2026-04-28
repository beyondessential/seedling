//! Tailscale discovery provider for site ingresses.
//!
//! Polls the local `tailscaled` Unix-socket API for the host's identity and
//! reconciles a single discovered site ingress against the runtime
//! database. The site ingress's hostname is the host's tailnet DNS name;
//! its `discovered_key` is the stable Tailscale node id, so renames update
//! the row in place and attachments survive them.
//
// r[impl ingress.site.tailscale]

use std::{path::PathBuf, sync::Arc, time::Duration};

use jiff::Timestamp;
use seedling_protocol::names::SiteIngressName;
use serde::Deserialize;
use tokio::{sync::Notify, task::JoinHandle};
use tracing::{debug, info, warn};

use crate::runtime::{
    db::DbHandle,
    site_ingresses::{self, DiscoveryProvider, SiteIngressDef, SiteIngressSource, TlsProvider},
};

/// Default Tailscale local API socket path. Operators on non-standard
/// installations can configure this via [`TailscaleConfig::socket_path`].
pub const DEFAULT_SOCKET_PATH: &str = "/var/run/tailscale/tailscaled.sock";

/// Operator-visible name we use for the discovered Tailscale site ingress.
/// Matches the provider name so listings read naturally.
const TAILSCALE_INGRESS_NAME: &str = "tailscale";

/// How often the provider polls tailscaled for the current identity.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// How many consecutive transient API errors are tolerated before the
/// provider files a `tailscale_unreachable` system fault.
const FAULT_AFTER_FAILURES: u32 = 5;

#[derive(Clone)]
pub struct TailscaleConfig {
    pub socket_path: PathBuf,
    pub poll_interval: Duration,
}

impl Default for TailscaleConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from(DEFAULT_SOCKET_PATH),
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

/// Identity material extracted from `/localapi/v0/status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredIdentity {
    /// Tailnet DNS name (e.g. `host.tailnet-name.ts.net`). Trailing dots
    /// are stripped.
    pub hostname: String,
    /// Stable per-node identifier (`Self.ID` from the local API).
    pub node_id: String,
    /// Whether the backend is currently logged in. When false, the
    /// provider marks the existing row stale instead of deleting it.
    pub backend_running: bool,
}

#[derive(Debug, Clone)]
pub enum TailscaleError {
    /// Socket file doesn't exist or refused the connection. Treated as
    /// "Tailscale not installed / not running" — not a fault.
    Unreachable(String),
    /// API returned a non-success HTTP status.
    Api { status: u16, body: String },
    /// Response body couldn't be deserialised.
    Decode(String),
}

impl std::fmt::Display for TailscaleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(s) => write!(f, "tailscaled unreachable: {s}"),
            Self::Api { status, body } => write!(f, "tailscaled API {status}: {body}"),
            Self::Decode(s) => write!(f, "tailscaled response decode failed: {s}"),
        }
    }
}

impl std::error::Error for TailscaleError {}

/// Public-shape of the discovery snapshot the OI handler exposes.
#[derive(Debug, Clone)]
pub struct DiscoveryStatusSnapshot {
    pub healthy: bool,
    pub last_poll_at: Option<Timestamp>,
    pub identity: Option<DiscoveredIdentity>,
    pub last_error: Option<String>,
}

/// Status surface the OI/CLI can read. Updated by the polling task.
pub struct DiscoveryStatus {
    inner: parking_lot::RwLock<DiscoveryStatusSnapshot>,
}

impl DiscoveryStatus {
    fn new() -> Self {
        Self {
            inner: parking_lot::RwLock::new(DiscoveryStatusSnapshot {
                healthy: false,
                last_poll_at: None,
                identity: None,
                last_error: None,
            }),
        }
    }

    pub fn snapshot(&self) -> DiscoveryStatusSnapshot {
        self.inner.read().clone()
    }

    fn record(&self, snap: DiscoveryStatusSnapshot) {
        *self.inner.write() = snap;
    }
}

pub struct TailscaleProvider {
    db: DbHandle,
    config: TailscaleConfig,
    kick: Notify,
    status: Arc<DiscoveryStatus>,
}

impl TailscaleProvider {
    pub fn new(db: DbHandle, config: TailscaleConfig) -> Arc<Self> {
        Arc::new(Self {
            db,
            config,
            kick: Notify::new(),
            status: Arc::new(DiscoveryStatus::new()),
        })
    }

    pub fn status(&self) -> Arc<DiscoveryStatus> {
        Arc::clone(&self.status)
    }

    /// Force the provider's poll loop to run now instead of waiting for the
    /// next tick. Used by the `/ingresses/site/discovery/refresh` OI route.
    pub fn refresh_now(&self) {
        self.kick.notify_one();
    }

    /// Spawn the provider's background task. Returns a join handle so the
    /// daemon can shut it down cleanly during teardown.
    pub fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        let provider = Arc::clone(&self);
        tokio::spawn(async move {
            provider.run().await;
        })
    }

    async fn run(self: Arc<Self>) {
        let mut consecutive_failures: u32 = 0;
        loop {
            match self.poll_once().await {
                Ok(identity) => {
                    consecutive_failures = 0;
                    self.reconcile_db(identity);
                }
                Err(TailscaleError::Unreachable(msg)) => {
                    debug!("tailscale: {msg}; skipping cycle");
                    consecutive_failures = 0;
                    self.status.record(DiscoveryStatusSnapshot {
                        healthy: false,
                        last_poll_at: Some(Timestamp::now()),
                        identity: None,
                        last_error: Some(msg),
                    });
                }
                Err(e) => {
                    consecutive_failures += 1;
                    warn!(
                        attempt = consecutive_failures,
                        "tailscale: poll failed: {e}"
                    );
                    self.status.record(DiscoveryStatusSnapshot {
                        healthy: false,
                        last_poll_at: Some(Timestamp::now()),
                        identity: None,
                        last_error: Some(e.to_string()),
                    });
                    if consecutive_failures >= FAULT_AFTER_FAILURES {
                        // Mark any existing discovered row stale so the
                        // reconciler stops emitting routes for it.
                        self.mark_existing_stale(true);
                    }
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(self.config.poll_interval) => {}
                _ = self.kick.notified() => {
                    debug!("tailscale: refresh_now triggered");
                }
            }
        }
    }

    /// Single poll attempt. Returns `Ok(None)` when the backend is reachable
    /// but reports no identity (e.g. backend not yet running, no Self
    /// section), `Ok(Some(identity))` on success, and `Err` on transport /
    /// API errors.
    pub async fn poll_once(&self) -> Result<Option<DiscoveredIdentity>, TailscaleError> {
        let client = build_client(&self.config.socket_path)?;
        let raw = match client.get("http://local/localapi/v0/status").send().await {
            Ok(resp) => resp,
            Err(e) => {
                if let Some(io_err) = io_error(&e)
                    && matches!(
                        io_err.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    )
                {
                    return Err(TailscaleError::Unreachable(io_err.to_string()));
                }
                return Err(TailscaleError::Unreachable(e.to_string()));
            }
        };
        let status = raw.status();
        if !status.is_success() {
            let body = raw.text().await.unwrap_or_default();
            return Err(TailscaleError::Api {
                status: status.as_u16(),
                body,
            });
        }
        let payload: StatusPayload = raw
            .json()
            .await
            .map_err(|e| TailscaleError::Decode(e.to_string()))?;
        Ok(parse_identity(&payload))
    }

    /// Reconcile the DB to match the latest poll result. `None` means the
    /// API was reachable but reported no identity (e.g. the user hasn't
    /// logged in yet); the existing discovered row is marked stale.
    pub fn reconcile_db(&self, identity: Option<DiscoveredIdentity>) {
        match &identity {
            Some(id) if id.backend_running => {
                let now = Timestamp::now();
                let id_for_db = id.clone();
                self.db.call(move |db| {
                    if let Err(e) = upsert_discovered_row(db, &id_for_db, &now) {
                        warn!(error = %e, "tailscale: db upsert failed");
                    }
                });
                self.status.record(DiscoveryStatusSnapshot {
                    healthy: true,
                    last_poll_at: Some(now),
                    identity: Some(id.clone()),
                    last_error: None,
                });
            }
            _ => {
                // Either Self section was absent, or the backend is not
                // running (logged out). Mark stale; don't delete — the
                // operator's attachments survive the outage.
                self.mark_existing_stale(true);
                self.status.record(DiscoveryStatusSnapshot {
                    healthy: false,
                    last_poll_at: Some(Timestamp::now()),
                    identity: identity.clone(),
                    last_error: Some(if identity.is_none() {
                        "no Self identity reported by tailscaled".to_owned()
                    } else {
                        "tailscale backend not running (logged out)".to_owned()
                    }),
                });
            }
        }
    }

    fn mark_existing_stale(&self, stale: bool) {
        self.db.call(move |db| {
            let name = SiteIngressName::new_unchecked(TAILSCALE_INGRESS_NAME);
            if let Ok(Some(_)) = site_ingresses::get(db, &name)
                && let Err(e) = site_ingresses::set_stale(db, &name, stale)
            {
                warn!(error = %e, "tailscale: failed to update stale flag");
            }
        });
    }
}

fn build_client(socket: &std::path::Path) -> Result<reqwest::Client, TailscaleError> {
    reqwest::Client::builder()
        .unix_socket(socket)
        .build()
        .map_err(|e| TailscaleError::Unreachable(format!("client build failed: {e}")))
}

fn io_error(err: &reqwest::Error) -> Option<&std::io::Error> {
    use std::error::Error;
    let mut src: Option<&dyn Error> = err.source();
    while let Some(s) = src {
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            return Some(io);
        }
        src = s.source();
    }
    None
}

/// Subset of tailscaled's `/localapi/v0/status` we care about. Field names
/// match the wire protocol; everything else is dropped on the floor.
#[derive(Debug, Deserialize)]
struct StatusPayload {
    #[serde(rename = "Self")]
    self_: Option<SelfStatus>,
    #[serde(rename = "BackendState")]
    backend_state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SelfStatus {
    #[serde(rename = "ID")]
    id: Option<String>,
    #[serde(rename = "DNSName")]
    dns_name: Option<String>,
}

fn parse_identity(payload: &StatusPayload) -> Option<DiscoveredIdentity> {
    let self_ = payload.self_.as_ref()?;
    let id = self_.id.as_ref()?.clone();
    let dns_name = self_.dns_name.as_ref()?;
    if id.is_empty() || dns_name.is_empty() {
        return None;
    }
    let backend_running = payload
        .backend_state
        .as_deref()
        .is_some_and(|s| s == "Running");
    Some(DiscoveredIdentity {
        hostname: dns_name.trim_end_matches('.').to_owned(),
        node_id: id,
        backend_running,
    })
}

fn upsert_discovered_row(
    db: &crate::runtime::db::Db,
    identity: &DiscoveredIdentity,
    now: &Timestamp,
) -> rusqlite::Result<()> {
    // First, see if there's already a discovered Tailscale row keyed by
    // this node id. If so, update its hostname in place (rename case) and
    // clear the stale flag.
    if let Some(existing) =
        site_ingresses::find_discovered(db, DiscoveryProvider::Tailscale, &identity.node_id)?
    {
        if existing.hostname != identity.hostname {
            site_ingresses::update_hostname_for_discovery(
                db,
                DiscoveryProvider::Tailscale,
                &identity.node_id,
                &identity.hostname,
            )?;
            info!(
                old = %existing.hostname,
                new = %identity.hostname,
                "tailscale: hostname renamed in place"
            );
        }
        if existing.stale {
            site_ingresses::set_stale(db, &existing.name, false)?;
        }
        return Ok(());
    }

    // No row for this node id. If a row already exists under the operator-
    // visible name (from a prior node identity that's since been replaced),
    // remove it before inserting the new one so the unique constraint and
    // PK both hold. Operator attachments on that row are dropped in this
    // case, which matches the design intent: a node-id change is "the
    // host identity changed" and attachments shouldn't silently follow.
    let name = SiteIngressName::new_unchecked(TAILSCALE_INGRESS_NAME);
    if let Some(existing) = site_ingresses::get(db, &name)?
        && existing.source.is_discovered()
    {
        site_ingresses::delete(db, &name)?;
        info!(
            stale_node_id = ?match &existing.source {
                SiteIngressSource::Discovered { key, .. } => key.as_str(),
                SiteIngressSource::Manual => "",
            },
            new_node_id = %identity.node_id,
            "tailscale: node identity changed; replacing discovered ingress"
        );
    }

    let def = SiteIngressDef {
        name,
        hostname: identity.hostname.clone(),
        description: Some("Auto-discovered Tailscale identity".to_owned()),
        source: SiteIngressSource::Discovered {
            provider: DiscoveryProvider::Tailscale,
            key: identity.node_id.clone(),
        },
        tls_provider: TlsProvider::Tailscale,
        stale: false,
        created_at: now.to_string(),
    };
    site_ingresses::create(db, &def)?;
    info!(
        hostname = %def.hostname,
        node_id = %identity.node_id,
        "tailscale: discovered ingress created"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::db::Db;

    fn mkdb() -> Db {
        Db::open_in_memory().expect("open in-memory db")
    }

    fn fake_identity(host: &str, node_id: &str) -> DiscoveredIdentity {
        DiscoveredIdentity {
            hostname: host.to_owned(),
            node_id: node_id.to_owned(),
            backend_running: true,
        }
    }

    #[test]
    fn parses_status_payload() {
        let body = serde_json::json!({
            "Self": {
                "ID": "n-abc123",
                "DNSName": "host.tailnet.ts.net.",
            },
            "BackendState": "Running",
        });
        let payload: StatusPayload = serde_json::from_value(body).unwrap();
        let id = parse_identity(&payload).expect("identity present");
        assert_eq!(id.hostname, "host.tailnet.ts.net");
        assert_eq!(id.node_id, "n-abc123");
        assert!(id.backend_running);
    }

    #[test]
    fn parse_returns_none_when_self_missing() {
        let body = serde_json::json!({ "BackendState": "Running" });
        let payload: StatusPayload = serde_json::from_value(body).unwrap();
        assert!(parse_identity(&payload).is_none());
    }

    #[test]
    fn parse_marks_logged_out_backend() {
        let body = serde_json::json!({
            "Self": { "ID": "n-1", "DNSName": "host.ts.net" },
            "BackendState": "NeedsLogin",
        });
        let payload: StatusPayload = serde_json::from_value(body).unwrap();
        let id = parse_identity(&payload).unwrap();
        assert!(!id.backend_running);
    }

    #[test]
    fn upsert_inserts_first_time() {
        let db = mkdb();
        let id = fake_identity("host.tailnet.ts.net", "n-1");
        upsert_discovered_row(&db, &id, &Timestamp::now()).unwrap();
        let name = SiteIngressName::new(TAILSCALE_INGRESS_NAME).unwrap();
        let row = site_ingresses::get(&db, &name).unwrap().unwrap();
        assert_eq!(row.hostname, "host.tailnet.ts.net");
        assert_eq!(row.tls_provider, TlsProvider::Tailscale);
        assert!(matches!(
            row.source,
            SiteIngressSource::Discovered {
                provider: DiscoveryProvider::Tailscale,
                ..
            }
        ));
        assert!(!row.stale);
    }

    #[test]
    fn upsert_renames_in_place_on_dns_change() {
        let db = mkdb();
        let id1 = fake_identity("old.tailnet.ts.net", "n-1");
        upsert_discovered_row(&db, &id1, &Timestamp::now()).unwrap();
        let id2 = fake_identity("new.tailnet.ts.net", "n-1");
        upsert_discovered_row(&db, &id2, &Timestamp::now()).unwrap();
        let name = SiteIngressName::new(TAILSCALE_INGRESS_NAME).unwrap();
        let row = site_ingresses::get(&db, &name).unwrap().unwrap();
        assert_eq!(row.hostname, "new.tailnet.ts.net");
        match row.source {
            SiteIngressSource::Discovered { key, .. } => assert_eq!(key, "n-1"),
            _ => panic!("source should remain discovered"),
        }
    }

    #[test]
    fn upsert_replaces_row_on_node_id_change() {
        let db = mkdb();
        upsert_discovered_row(&db, &fake_identity("a.ts.net", "n-1"), &Timestamp::now()).unwrap();
        upsert_discovered_row(&db, &fake_identity("b.ts.net", "n-2"), &Timestamp::now()).unwrap();
        let name = SiteIngressName::new(TAILSCALE_INGRESS_NAME).unwrap();
        let row = site_ingresses::get(&db, &name).unwrap().unwrap();
        assert_eq!(row.hostname, "b.ts.net");
        match row.source {
            SiteIngressSource::Discovered { key, .. } => assert_eq!(key, "n-2"),
            _ => panic!("source should be discovered"),
        }
    }

    #[test]
    fn upsert_clears_stale_when_seen_again() {
        let db = mkdb();
        let id = fake_identity("host.ts.net", "n-1");
        upsert_discovered_row(&db, &id, &Timestamp::now()).unwrap();
        let name = SiteIngressName::new(TAILSCALE_INGRESS_NAME).unwrap();
        site_ingresses::set_stale(&db, &name, true).unwrap();
        upsert_discovered_row(&db, &id, &Timestamp::now()).unwrap();
        let row = site_ingresses::get(&db, &name).unwrap().unwrap();
        assert!(!row.stale);
    }
}
