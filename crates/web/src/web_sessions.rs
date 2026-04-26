use std::{collections::HashMap, sync::Arc, time::Duration};

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

pub struct WebSessionEntry {
    pub id: Uuid,
    pub connected_at: Timestamp,
    pub last_seen: Timestamp,
    pub actor: Arc<Actor>,
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
    /// Update `last_seen` to `now` for a session. Returns true if the session
    /// existed; false if it had already been removed (e.g. reaped between the
    /// browser's last heartbeat being sent and arriving).
    pub fn touch(&self, id: &Uuid, now: Timestamp) -> bool {
        let mut guard = self.sessions.lock();
        if let Some(entry) = guard.get_mut(id) {
            entry.last_seen = now;
            true
        } else {
            false
        }
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
                })
            })
            .collect()
    }
}
