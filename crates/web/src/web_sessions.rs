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
