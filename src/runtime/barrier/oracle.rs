use std::collections::HashMap;

use parking_lot::Mutex;

use crate::defs::resource::ResourceKind;
use crate::runtime::history::WorldObservation;
use crate::runtime::{LifecycleState, ResourceInstance};

// r[impl lifecycle.derivation]
// r[impl history.world.state-derivation]
pub trait WorldStateOracle: Send + Sync {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> LifecycleState;
}

/// A simple in-memory oracle. Useful in tests, but also as the initial
/// implementation before a real observation-history-backed oracle exists.
pub struct TestWorldOracle {
    states: Mutex<HashMap<ResourceInstance, LifecycleState>>,
}

impl TestWorldOracle {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }

    pub fn set(&self, resource: ResourceInstance, state: LifecycleState) {
        self.states.lock().insert(resource, state);
    }
}

impl Default for TestWorldOracle {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldStateOracle for TestWorldOracle {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> LifecycleState {
        let states = self.states.lock();

        // Try exact match first.
        if let Some(&s) = states.get(resource) {
            return s;
        }

        // Fallback: match by kind + name + ordinal, ignoring the app field.
        // This allows callers that key the oracle with one app name to match
        // resources the runtime extracted with a different (or empty) app name.
        for (k, &v) in states.iter() {
            if k.kind == resource.kind && k.name == resource.name && k.ordinal == resource.ordinal {
                return v;
            }
        }

        LifecycleState::Pending
    }
}

// ---------------------------------------------------------------------------
// DbWorldOracle
// ---------------------------------------------------------------------------

pub struct DbWorldOracle {
    db: Mutex<crate::runtime::db::Db>,
}

impl DbWorldOracle {
    pub fn new(db: crate::runtime::db::Db) -> Self {
        Self { db: Mutex::new(db) }
    }
}

impl WorldStateOracle for DbWorldOracle {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> LifecycleState {
        let db = self.db.lock();
        let observations = match crate::runtime::history::query_observations(&db, resource) {
            Ok(obs) => obs,
            Err(_) => return LifecycleState::Pending,
        };
        derive_lifecycle_state(resource, &observations)
    }
}

// ---------------------------------------------------------------------------
// Lifecycle derivation
// ---------------------------------------------------------------------------

// r[impl lifecycle.derivation]
/// Derive the current lifecycle state for a resource from its observation history.
///
/// Observations must be provided in chronological order (ascending `recorded_at`).
/// This is a pure function with no DB access; callers fetch the observations and
/// pass them in.
pub fn derive_lifecycle_state(
    resource: &ResourceInstance,
    observations: &[WorldObservation],
) -> LifecycleState {
    match resource.kind {
        ResourceKind::Deployment | ResourceKind::Job => {
            derive_container_lifecycle_state(observations)
        }
        // Service, Ingress, Volume observation kinds are not yet defined.
        _ => LifecycleState::Pending,
    }
}

// r[impl lifecycle.container]
fn derive_container_lifecycle_state(observations: &[WorldObservation]) -> LifecycleState {
    let mut state = LifecycleState::Pending;

    for obs in observations {
        let next = match obs.obs_kind.as_str() {
            "container_created" | "image_pull_started" => LifecycleState::Scheduled,
            "container_running" => LifecycleState::Running,
            "health_check_pass" => LifecycleState::Ready,
            "stop_sent" => LifecycleState::Terminating,
            "container_exited" => LifecycleState::Terminated,
            "container_removed" => LifecycleState::Unscheduled,
            // Unknown observation kind — skip.
            _ => continue,
        };

        // Already at or past this state — skip (idempotent observations).
        if state.has_reached(next) {
            continue;
        }

        // Advance if the transition is valid; skip anomalous observations.
        if state.can_transition_to(next) {
            state = next;
        }
    }

    state
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::resource::ResourceKind;
    use crate::runtime::db::Db;
    use crate::runtime::history::{WorldObservation, insert_observation, query_observations};

    fn dep(app: &str, name: &str) -> ResourceInstance {
        ResourceInstance::named(app, ResourceKind::Deployment, name)
    }

    /// Build a minimal `WorldObservation` for testing the pure derivation function.
    fn obs(obs_kind: &str) -> WorldObservation {
        WorldObservation {
            id: 0,
            recorded_at: 0,
            resource: dep("app", "web"),
            obs_kind: obs_kind.into(),
            payload: serde_json::Value::Null,
        }
    }

    // --- derive_lifecycle_state (pure) ---

    // r[verify lifecycle.derivation]
    // r[verify lifecycle.states]
    #[test]
    fn empty_observations_gives_pending() {
        let resource = dep("app", "web");
        let state = derive_lifecycle_state(&resource, &[]);
        assert_eq!(state, LifecycleState::Pending);
    }

    // r[verify lifecycle.container]
    // r[verify lifecycle.derivation]
    #[test]
    fn container_created_gives_scheduled() {
        let resource = dep("app", "web");
        let state = derive_lifecycle_state(&resource, &[obs("container_created")]);
        assert_eq!(state, LifecycleState::Scheduled);
    }

    // r[verify lifecycle.container]
    #[test]
    fn image_pull_started_gives_scheduled() {
        let resource = dep("app", "web");
        let state = derive_lifecycle_state(&resource, &[obs("image_pull_started")]);
        assert_eq!(state, LifecycleState::Scheduled);
    }

    // r[verify lifecycle.container]
    // r[verify lifecycle.derivation]
    #[test]
    fn container_running_gives_running() {
        let resource = dep("app", "web");
        let state = derive_lifecycle_state(&resource, &[obs("container_running")]);
        assert_eq!(state, LifecycleState::Running);
    }

    // r[verify lifecycle.container]
    #[test]
    fn health_check_pass_gives_ready() {
        let resource = dep("app", "web");
        let state = derive_lifecycle_state(
            &resource,
            &[obs("container_running"), obs("health_check_pass")],
        );
        assert_eq!(state, LifecycleState::Ready);
    }

    // r[verify lifecycle.container]
    // r[verify lifecycle.transitions]
    #[test]
    fn container_exited_gives_terminated_skipping_terminating() {
        let resource = dep("app", "web");
        let state = derive_lifecycle_state(
            &resource,
            &[obs("container_running"), obs("container_exited")],
        );
        assert_eq!(state, LifecycleState::Terminated);
    }

    // r[verify lifecycle.container]
    // r[verify lifecycle.transitions]
    #[test]
    fn stop_sent_then_exited_gives_terminated_via_terminating() {
        let resource = dep("app", "web");
        let state = derive_lifecycle_state(
            &resource,
            &[
                obs("container_running"),
                obs("stop_sent"),
                obs("container_exited"),
            ],
        );
        assert_eq!(state, LifecycleState::Terminated);
    }

    // r[verify lifecycle.container]
    // r[verify lifecycle.transitions]
    #[test]
    fn container_removed_gives_unscheduled() {
        let resource = dep("app", "web");
        let state = derive_lifecycle_state(
            &resource,
            &[
                obs("container_running"),
                obs("container_exited"),
                obs("container_removed"),
            ],
        );
        assert_eq!(state, LifecycleState::Unscheduled);
    }

    // r[verify lifecycle.derivation]
    // r[verify reconciliation.idempotency]
    #[test]
    fn duplicate_running_observations_do_not_regress() {
        let resource = dep("app", "web");
        // Duplicate container_running is idempotent
        let state = derive_lifecycle_state(
            &resource,
            &[obs("container_running"), obs("container_running")],
        );
        assert_eq!(state, LifecycleState::Running);
    }

    // r[verify lifecycle.derivation]
    #[test]
    fn unknown_obs_kind_is_ignored() {
        let resource = dep("app", "web");
        let state = derive_lifecycle_state(
            &resource,
            &[obs("container_running"), obs("some_unknown_event")],
        );
        assert_eq!(state, LifecycleState::Running);
    }

    // r[verify lifecycle.service]
    #[test]
    fn service_kind_returns_pending() {
        let resource = ResourceInstance::named("app", ResourceKind::Service, "web");
        let state = derive_lifecycle_state(&resource, &[obs("container_running")]);
        assert_eq!(state, LifecycleState::Pending);
    }

    // --- DbWorldOracle (DB-backed) ---

    // r[verify history.world.state-derivation]
    #[test]
    fn db_oracle_empty_gives_pending() {
        let db = Db::open_in_memory().expect("open");
        let oracle = DbWorldOracle::new(db);
        let resource = dep("app", "web");
        assert_eq!(oracle.lifecycle_state(&resource), LifecycleState::Pending);
    }

    // r[verify history.world.state-derivation]
    // r[verify lifecycle.derivation]
    #[test]
    fn db_oracle_derives_from_observations() {
        let db = Db::open_in_memory().expect("open");
        let resource = dep("app", "web");

        insert_observation(&db, &resource, "container_created", &serde_json::json!({}))
            .expect("insert");
        insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
            .expect("insert");

        let oracle = DbWorldOracle::new(db);
        assert_eq!(oracle.lifecycle_state(&resource), LifecycleState::Running);
    }

    // r[verify history.world.state-derivation]
    // r[verify lifecycle.container]
    // r[verify lifecycle.transitions]
    #[test]
    fn db_oracle_full_sequence_to_terminated() {
        let db = Db::open_in_memory().expect("open");
        let resource = dep("app", "web");

        for kind in &["container_running", "stop_sent", "container_exited"] {
            insert_observation(&db, &resource, kind, &serde_json::json!({})).expect("insert");
        }

        let oracle = DbWorldOracle::new(db);
        assert_eq!(
            oracle.lifecycle_state(&resource),
            LifecycleState::Terminated
        );
    }

    // r[verify history.world.state-derivation]
    // r[verify lifecycle.derivation]
    #[test]
    fn db_oracle_uses_query_observations_from_history() {
        // Ensure the DB oracle is consistent with direct query_observations calls.
        let db = Db::open_in_memory().expect("open");
        let resource = dep("app", "api");

        insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
            .expect("insert");
        insert_observation(&db, &resource, "health_check_pass", &serde_json::json!({}))
            .expect("insert");

        // Verify via direct query
        let obs_vec = query_observations(&db, &resource).expect("query");
        let direct_state = derive_lifecycle_state(&resource, &obs_vec);
        assert_eq!(direct_state, LifecycleState::Ready);

        // Verify via oracle
        let oracle = DbWorldOracle::new(db);
        assert_eq!(oracle.lifecycle_state(&resource), LifecycleState::Ready);
    }
}
