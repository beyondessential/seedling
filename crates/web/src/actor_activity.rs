use std::collections::HashMap;

use jiff::{SignedDuration, Timestamp};
use parking_lot::Mutex;
use serde_json::Value;

// w[sessions.actor-activity]
/// Activity entries older than this are dropped at list time. Matches the
/// web-session stale cutoff so the connected-clients view has a consistent
/// "operator is here" window.
pub const ACTIVITY_WINDOW: SignedDuration = SignedDuration::from_secs(600);

/// One observed activity entry per `(actor_kind, actor_id)`. The display name
/// is whatever the most recent event carried.
struct ActivityEntry {
    last_seen: Timestamp,
    actor_display: Option<String>,
    last_action: String,
}

#[derive(Default)]
pub struct ActorActivityRegistry {
    inner: Mutex<HashMap<(String, String), ActivityEntry>>,
}

impl ActorActivityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    // w[impl sessions.actor-activity]
    /// Inspect a published event line and, if it carries an actor identity,
    /// update the corresponding entry. Events with no actor (autonomous
    /// reconciler activity such as boot/schedule fires) are ignored.
    pub fn record_from_event_line(&self, line: &str) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let Some(obj) = value.as_object() else { return };
        let Some(actor) = obj.get("actor").and_then(|a| a.as_object()) else {
            return;
        };
        let kind = match actor.get("kind").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return,
        };
        let id = match actor.get("id").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return,
        };
        let display = actor
            .get("display")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let timestamp = obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Timestamp>().ok())
            .unwrap_or_else(Timestamp::now);
        let event_type = obj
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("event")
            .to_string();
        let last_action = render_action(&event_type, obj);

        let mut guard = self.inner.lock();
        let entry = guard
            .entry((kind, id))
            .or_insert_with(|| ActivityEntry {
                last_seen: timestamp,
                actor_display: display.clone(),
                last_action: last_action.clone(),
            });
        if timestamp >= entry.last_seen {
            entry.last_seen = timestamp;
            entry.last_action = last_action;
            if display.is_some() {
                entry.actor_display = display;
            }
        }
    }

    // w[impl sessions.actor-activity]
    /// Snapshot the entries whose last_seen is within the activity window.
    /// Older entries are dropped from the underlying map at the same time so
    /// the registry does not grow unboundedly across long-lived processes.
    pub fn list_recent(&self) -> Vec<Value> {
        let now = Timestamp::now();
        let cutoff = now.checked_sub(ACTIVITY_WINDOW).unwrap_or(now);
        let mut guard = self.inner.lock();
        guard.retain(|_, e| e.last_seen >= cutoff);
        guard
            .iter()
            .map(|((kind, id), entry)| {
                serde_json::json!({
                    "actor_kind": kind,
                    "actor_id": id,
                    "actor_display": entry.actor_display,
                    "last_seen": entry.last_seen.to_string(),
                    "last_action": entry.last_action,
                })
            })
            .collect()
    }
}

/// Build a human-readable summary of an event for display in the
/// connected-clients view. Falls back to the event type when no useful
/// context fields are present.
fn render_action(event_type: &str, obj: &serde_json::Map<String, Value>) -> String {
    let app = obj.get("app").and_then(|v| v.as_str());
    let action_name = obj.get("action_name").and_then(|v| v.as_str());
    let param_name = obj.get("name").and_then(|v| v.as_str());
    let deployment = obj.get("deployment").and_then(|v| v.as_str());
    let scale = obj.get("scale").and_then(|v| v.as_i64());

    match (event_type, app) {
        ("OperationStarted", Some(app)) => match action_name {
            Some(name) => format!("started {name} on {app}"),
            None => format!("started operation on {app}"),
        },
        ("OperationCompleted", Some(app)) => match action_name {
            Some(name) => format!("completed {name} on {app}"),
            None => format!("completed operation on {app}"),
        },
        ("OperationFailed", Some(app)) => match action_name {
            Some(name) => format!("failed {name} on {app}"),
            None => format!("failed operation on {app}"),
        },
        ("ParamSet", Some(app)) => match param_name {
            Some(p) => format!("set param {p} on {app}"),
            None => format!("set param on {app}"),
        },
        ("ParamUnset", Some(app)) => match param_name {
            Some(p) => format!("unset param {p} on {app}"),
            None => format!("unset param on {app}"),
        },
        ("AppRegistered", Some(app)) => format!("registered {app}"),
        ("AppUpdated", Some(app)) => format!("updated {app}"),
        ("AppDeregistered", Some(app)) => format!("deregistered {app}"),
        ("ScaleChanged", Some(app)) => match (deployment, scale) {
            (Some(d), Some(n)) => format!("scaled {d} to {n} on {app}"),
            (Some(d), None) => format!("scaled {d} on {app}"),
            _ => format!("scale change on {app}"),
        },
        ("ShellStarted", Some(app)) => format!("opened shell on {app}"),
        ("ShellExited", Some(app)) => format!("closed shell on {app}"),
        ("ForwardStarted", Some(app)) => format!("opened forward on {app}"),
        ("ForwardStopped", Some(app)) => format!("closed forward on {app}"),
        ("WebSessionStarted", _) => "opened web session".to_string(),
        ("WebSessionStopped", _) => "closed web session".to_string(),
        (other, Some(app)) => format!("{other} on {app}"),
        (other, None) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(v: Value) -> String {
        v.to_string()
    }

    #[test]
    fn ignores_event_without_actor() {
        let reg = ActorActivityRegistry::new();
        reg.record_from_event_line(&line(json!({
            "type": "ResourceStateChanged",
            "timestamp": "2026-04-26T12:00:00Z",
            "app": "demo",
        })));
        assert!(reg.list_recent().is_empty());
    }

    #[test]
    fn records_operation_started_with_actor() {
        let reg = ActorActivityRegistry::new();
        reg.record_from_event_line(&line(json!({
            "type": "OperationStarted",
            "timestamp": Timestamp::now().to_string(),
            "app": "postgres",
            "action_name": "dump",
            "actor": {
                "kind": "ctl",
                "id": "fingerprint-abc",
                "display": "felix"
            },
        })));
        let entries = reg.list_recent();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["actor_kind"], "ctl");
        assert_eq!(entries[0]["actor_display"], "felix");
        assert_eq!(entries[0]["last_action"], "started dump on postgres");
    }

    #[test]
    fn drops_entries_outside_window() {
        let reg = ActorActivityRegistry::new();
        reg.record_from_event_line(&line(json!({
            "type": "AppRegistered",
            "timestamp": "2020-01-01T00:00:00Z",
            "app": "ancient",
            "actor": { "kind": "ctl", "id": "anyone" },
        })));
        assert!(reg.list_recent().is_empty());
    }

    #[test]
    fn deduplicates_by_kind_and_id() {
        let reg = ActorActivityRegistry::new();
        let now = Timestamp::now();
        let earlier = now.checked_sub(SignedDuration::from_secs(30)).unwrap();
        reg.record_from_event_line(&line(json!({
            "type": "OperationStarted",
            "timestamp": earlier.to_string(),
            "app": "a",
            "action_name": "first",
            "actor": { "kind": "ctl", "id": "u1", "display": "old name" },
        })));
        reg.record_from_event_line(&line(json!({
            "type": "OperationStarted",
            "timestamp": now.to_string(),
            "app": "a",
            "action_name": "second",
            "actor": { "kind": "ctl", "id": "u1", "display": "new name" },
        })));
        let entries = reg.list_recent();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["last_action"], "started second on a");
        assert_eq!(entries[0]["actor_display"], "new name");
    }
}
