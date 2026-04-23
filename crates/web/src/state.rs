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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tokens() -> WtTokens {
        Mutex::new(HashMap::new())
    }

    fn test_actor() -> Arc<Actor> {
        Arc::new(Actor {
            kind: Some("password".to_owned()),
            id: Some("admin".to_owned()),
            display: Some("admin".to_owned()),
            session: Some("session".to_owned()),
        })
    }

    // w[verify wt.token]
    #[test]
    fn issue_and_consume_round_trip() {
        let t = tokens();
        let token = issue_wt_token(&t, test_actor());
        let got = consume_wt_token(&t, &token).expect("valid token should consume");
        assert_eq!(got.id.as_deref(), Some("admin"));
    }

    // w[verify wt.token]
    #[test]
    fn tokens_are_single_use() {
        let t = tokens();
        let token = issue_wt_token(&t, test_actor());
        assert!(consume_wt_token(&t, &token).is_some(), "first consume ok");
        assert!(
            consume_wt_token(&t, &token).is_none(),
            "second consume must fail — one-shot token",
        );
    }

    // w[verify wt.token]
    #[test]
    fn unknown_token_returns_none() {
        let t = tokens();
        assert!(consume_wt_token(&t, "no-such-token").is_none());
    }

    // w[verify wt.token]
    #[test]
    fn expired_token_does_not_consume() {
        let t = tokens();
        let token = issue_wt_token(&t, test_actor());
        // Force expire the entry by rewriting the stored deadline.
        {
            let mut map = t.lock();
            let entry = map.get_mut(&token).unwrap();
            entry.expires = Instant::now() - Duration::from_secs(1);
        }
        assert!(consume_wt_token(&t, &token).is_none());
    }

    // w[verify wt.token]
    #[test]
    fn issuing_prunes_expired_and_used_entries() {
        let t = tokens();
        // Issue two tokens: one we'll force-expire, one we'll consume.
        // Populate both without pruning in between by bypassing the public
        // issue fn for the second entry.
        let stale = issue_wt_token(&t, test_actor());
        let used = "used-token".to_owned();
        {
            let mut map = t.lock();
            // Force the stale token into the past.
            map.get_mut(&stale).unwrap().expires = Instant::now() - Duration::from_secs(1);
            // Inject a used-flagged entry directly.
            map.insert(
                used.clone(),
                WtTokenEntry {
                    actor: test_actor(),
                    expires: Instant::now() + Duration::from_secs(60),
                    used: true,
                },
            );
        }
        assert_eq!(
            t.lock().len(),
            2,
            "stale + used entries both present before next issue"
        );

        let _fresh = issue_wt_token(&t, test_actor());
        let map = t.lock();
        assert!(!map.contains_key(&stale), "expired entry pruned on issue");
        assert!(!map.contains_key(&used), "used entry pruned on issue");
        assert_eq!(map.len(), 1);
    }

    // w[verify wt.token]
    #[test]
    fn issued_tokens_are_unique() {
        let t = tokens();
        let a = issue_wt_token(&t, test_actor());
        let b = issue_wt_token(&t, test_actor());
        assert_ne!(a, b);
    }
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
