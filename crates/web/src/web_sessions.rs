use std::{collections::HashMap, fmt, sync::Arc, time::Duration};

use jiff::{SignedDuration, Timestamp};
use parking_lot::Mutex;
use seedling_protocol::actor::Actor;
use uuid::Uuid;

// w[sessions.stale-cutoff]
/// A session is considered stale when its `last_seen` is older than this. The
/// reaper task drops stale sessions and emits `WebSessionStopped` events.
pub const STALE_CUTOFF: SignedDuration = SignedDuration::from_secs(600);

/// How often the reaper task runs.
pub const REAPER_TICK: Duration = Duration::from_secs(60);

// w[sessions.safety-mode]
/// The advisory safety tier reported by a browser session. The server records
/// and broadcasts whatever the client reports without enforcing it on any OI
/// request.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SafetyMode {
    #[default]
    Read,
    Write,
    Dangerous,
}

impl SafetyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Dangerous => "dangerous",
        }
    }

    /// Parse a client-supplied mode string. Unknown or absent values fall back
    /// to `read` — the safest default if a peer ever reports something we
    /// don't understand.
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("write") => Self::Write,
            Some("dangerous") => Self::Dangerous,
            _ => Self::Read,
        }
    }
}

impl fmt::Display for SafetyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub struct WebSessionEntry {
    pub id: Uuid,
    pub connected_at: Timestamp,
    pub last_seen: Timestamp,
    pub actor: Arc<Actor>,
    // w[impl sessions.safety-mode]
    pub safety_mode: SafetyMode,
}

#[derive(Default)]
pub struct WebSessionRegistry {
    sessions: Mutex<HashMap<Uuid, WebSessionEntry>>,
}

impl WebSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, entry: WebSessionEntry) {
        self.sessions.lock().insert(entry.id, entry);
    }

    pub fn remove(&self, id: &Uuid) {
        self.sessions.lock().remove(id);
    }

    // w[impl sessions.heartbeat]
    /// Update `last_seen` to `now` for a session, and apply the reported
    /// safety mode if one was supplied. Returns the heartbeat outcome so the
    /// caller can decide whether to publish a `WebSessionModeChanged` event.
    pub fn touch(&self, id: &Uuid, now: Timestamp, mode: Option<SafetyMode>) -> HeartbeatOutcome {
        let mut guard = self.sessions.lock();
        let Some(entry) = guard.get_mut(id) else {
            return HeartbeatOutcome::Missing;
        };
        entry.last_seen = now;
        // w[impl sessions.safety-mode]
        let mode_change = mode.and_then(|new_mode| {
            (entry.safety_mode != new_mode).then(|| {
                entry.safety_mode = new_mode;
                new_mode
            })
        });
        HeartbeatOutcome::Alive { mode_change }
    }

    // w[impl sessions.stale-cutoff]
    /// Remove and return every session whose `last_seen` is older than `now -
    /// STALE_CUTOFF`. The caller is expected to publish a `WebSessionStopped`
    /// event for each returned id.
    pub fn reap_stale(&self, now: Timestamp) -> Vec<Uuid> {
        let cutoff = now.checked_sub(STALE_CUTOFF).unwrap_or(now);
        let mut guard = self.sessions.lock();
        let stale: Vec<Uuid> = guard
            .iter()
            .filter(|(_, e)| e.last_seen < cutoff)
            .map(|(id, _)| *id)
            .collect();
        for id in &stale {
            guard.remove(id);
        }
        stale
    }

    // w[impl routes.sessions]
    pub fn list(&self) -> Vec<serde_json::Value> {
        self.sessions
            .lock()
            .values()
            .map(|e| {
                serde_json::json!({
                    "id": e.id.to_string(),
                    "connected_at": e.connected_at.to_string(),
                    "last_seen": e.last_seen.to_string(),
                    "actor_kind": e.actor.kind,
                    "actor_id": e.actor.id,
                    "actor_display": e.actor.display,
                    // w[impl sessions.safety-mode]
                    "safety_mode": e.safety_mode.as_str(),
                })
            })
            .collect()
    }
}

/// Result of [`WebSessionRegistry::touch`]. `mode_change` is `Some(new_mode)`
/// when the heartbeat reported a mode that differs from what was recorded —
/// the caller publishes a `WebSessionModeChanged` event in that case.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HeartbeatOutcome {
    Missing,
    Alive { mode_change: Option<SafetyMode> },
}

impl HeartbeatOutcome {
    pub fn alive(self) -> bool {
        matches!(self, Self::Alive { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> Arc<Actor> {
        Arc::new(Actor {
            kind: Some("password".to_owned()),
            id: Some("admin".to_owned()),
            display: Some("admin".to_owned()),
            session: Some("session".to_owned()),
        })
    }

    fn entry(id: Uuid, now: Timestamp) -> WebSessionEntry {
        WebSessionEntry {
            id,
            connected_at: now,
            last_seen: now,
            actor: actor(),
            safety_mode: SafetyMode::Read,
        }
    }

    // w[verify sessions.safety-mode]
    #[test]
    fn safety_mode_parse_falls_back_to_read() {
        assert_eq!(SafetyMode::parse(Some("write")), SafetyMode::Write);
        assert_eq!(SafetyMode::parse(Some("dangerous")), SafetyMode::Dangerous);
        assert_eq!(SafetyMode::parse(Some("read")), SafetyMode::Read);
        // Unknown / absent / hostile values must never silently elevate.
        assert_eq!(SafetyMode::parse(Some("DANGEROUS")), SafetyMode::Read);
        assert_eq!(SafetyMode::parse(Some("")), SafetyMode::Read);
        assert_eq!(SafetyMode::parse(None), SafetyMode::Read);
    }

    // w[verify sessions.safety-mode]
    #[test]
    fn touch_returns_mode_change_only_when_different() {
        let registry = WebSessionRegistry::new();
        let id = Uuid::new_v4();
        let now = Timestamp::now();
        registry.insert(entry(id, now));

        // Same mode: no change reported.
        let outcome = registry.touch(&id, now, Some(SafetyMode::Read));
        assert_eq!(outcome, HeartbeatOutcome::Alive { mode_change: None });

        // Different mode: change carries the new value.
        let outcome = registry.touch(&id, now, Some(SafetyMode::Write));
        assert_eq!(
            outcome,
            HeartbeatOutcome::Alive {
                mode_change: Some(SafetyMode::Write)
            }
        );

        // Heartbeat without a reported mode never overwrites the stored one.
        let outcome = registry.touch(&id, now, None);
        assert_eq!(outcome, HeartbeatOutcome::Alive { mode_change: None });
        let listed = registry.list();
        assert_eq!(listed[0]["safety_mode"], "write");
    }

    // w[verify sessions.heartbeat]
    #[test]
    fn touch_on_missing_session_returns_missing() {
        let registry = WebSessionRegistry::new();
        let outcome = registry.touch(&Uuid::new_v4(), Timestamp::now(), Some(SafetyMode::Write));
        assert_eq!(outcome, HeartbeatOutcome::Missing);
        assert!(!outcome.alive());
    }
}
