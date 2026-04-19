use std::{collections::HashMap, sync::Arc};

use parking_lot::Mutex;
use seedling_protocol::actor::Actor;
use uuid::Uuid;

pub struct WebSessionEntry {
    pub id: Uuid,
    pub connected_at: jiff::Timestamp,
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

    // w[impl routes.sessions]
    pub fn list(&self) -> Vec<serde_json::Value> {
        self.sessions
            .lock()
            .values()
            .map(|e| {
                serde_json::json!({
                    "id": e.id.to_string(),
                    "connected_at": e.connected_at.to_string(),
                    "actor_kind": e.actor.kind,
                    "actor_id": e.actor.id,
                    "actor_display": e.actor.display,
                })
            })
            .collect()
    }
}
