use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use seedling_protocol::actor::Actor;

use crate::daemon::DaemonConn;
use crate::event_broker::EventBroker;
use crate::web_sessions::WebSessionRegistry;
use crate::wt_cert::CertStore;

pub struct WtTokenEntry {
    pub actor: Arc<Actor>,
    pub expires: Instant,
    pub used: bool,
}

pub type WtTokens = Mutex<std::collections::HashMap<String, WtTokenEntry>>;

/// Single-use WT tokens are valid for 30 seconds to bridge POST /connect → WT handshake.
pub const WT_TOKEN_LIFETIME: Duration = Duration::from_secs(30);

// w[wt.token]
pub fn issue_wt_token(tokens: &WtTokens, actor: Arc<Actor>) -> String {
    let token = uuid::Uuid::new_v4().to_string();
    let expires = Instant::now() + WT_TOKEN_LIFETIME;

    let mut map = tokens.lock();
    map.retain(|_, v| !v.used && v.expires > Instant::now());
    map.insert(
        token.clone(),
        WtTokenEntry {
            actor,
            expires,
            used: false,
        },
    );
    token
}

/// Validates and consumes a single-use WT token. Returns the associated actor or None.
// w[wt.token]
pub fn consume_wt_token(tokens: &WtTokens, token: &str) -> Option<Arc<Actor>> {
    let mut map = tokens.lock();
    let entry = map.get_mut(token)?;
    if entry.used || entry.expires <= Instant::now() {
        return None;
    }
    entry.used = true;
    Some(Arc::clone(&entry.actor))
}

#[derive(Clone)]
pub struct AppState {
    pub trust_tailscale: bool,
    pub dev_no_auth: bool,
    pub cert_store: Arc<parking_lot::RwLock<CertStore>>,
    pub sessions: Arc<crate::auth::password::Sessions>,
    pub wt_tokens: Arc<WtTokens>,
    pub session_lifetime: Duration,
    pub password_hash: Option<String>,
    pub wt_port: u16,
    /// When set, the SPA fallback proxies to a Vite dev server instead of serving embedded assets.
    pub vite_port: Option<u16>,
    pub daemon: Arc<DaemonConn>,
    pub event_broker: Arc<EventBroker>,
    pub web_sessions: Arc<WebSessionRegistry>,
}
