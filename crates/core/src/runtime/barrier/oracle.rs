use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

use crate::defs::resource::ResourceKind;
use crate::runtime::history::WorldObservation;
use crate::runtime::{LifecycleState, ResourceInstance};

// r[impl lifecycle.derivation]
// r[impl history.world.state-derivation]
pub trait WorldStateOracle: Send + Sync {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> LifecycleState;

    /// Returns true if a TLS certificate has been observed valid for the given
    /// ingress resource. Used by `rt.warm_certs(...).ready()` to wait on cert
    /// validity without coupling to the standard ingress `Ready` lifecycle
    /// (which also requires routing).
    // r[impl observe.ingress.certs]
    fn cert_valid_for(&self, resource: &ResourceInstance) -> bool {
        let _ = resource;
        false
    }
}

/// A simple in-memory oracle for tests.
///
/// Keyed by `(ResourceKind, Option<name>)` so that test code setting state via
/// a helper `dep("web")` and runtime code querying a freshly-created instance
/// of the same resource (with a different UUID) still match correctly.
pub struct TestWorldOracle {
    states: Mutex<HashMap<(ResourceKind, Option<String>), LifecycleState>>,
    valid_certs: Mutex<std::collections::HashSet<(ResourceKind, Option<String>)>>,
}

impl TestWorldOracle {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
            valid_certs: Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub fn set(&self, resource: ResourceInstance, state: LifecycleState) {
        self.states
            .lock()
            .insert((resource.kind, resource.name), state);
    }

    /// Mark the given ingress as having a valid cert for `cert_valid_for`.
    pub fn set_cert_valid(&self, resource: ResourceInstance) {
        self.valid_certs
            .lock()
            .insert((resource.kind, resource.name));
    }
}

impl Default for TestWorldOracle {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldStateOracle for TestWorldOracle {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> LifecycleState {
        self.states
            .lock()
            .get(&(resource.kind, resource.name.clone()))
            .copied()
            .unwrap_or(LifecycleState::Pending)
    }

    fn cert_valid_for(&self, resource: &ResourceInstance) -> bool {
        self.valid_certs
            .lock()
            .contains(&(resource.kind, resource.name.clone()))
    }
}

// ---------------------------------------------------------------------------
// DbWorldOracle
// ---------------------------------------------------------------------------

pub struct DbWorldOracle {
    db: crate::runtime::db::DbHandle,
}

impl DbWorldOracle {
    pub fn new(db: crate::runtime::db::DbHandle) -> Self {
        Self { db }
    }
}

impl WorldStateOracle for DbWorldOracle {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> LifecycleState {
        let resource = resource.clone();
        self.db.call(move |db| {
            let observations = match crate::runtime::history::query_observations(db, &resource) {
                Ok(obs) => obs,
                Err(_) => return LifecycleState::Pending,
            };
            derive_lifecycle_state(&resource, &observations)
        })
    }

    fn cert_valid_for(&self, resource: &ResourceInstance) -> bool {
        if resource.kind != ResourceKind::Ingress {
            return false;
        }
        let resource = resource.clone();
        self.db.call(move |db| {
            let observations = match crate::runtime::history::query_observations(db, &resource) {
                Ok(obs) => obs,
                Err(_) => return false,
            };
            // Most recent cert observation determines current validity.
            // `cert_valid` overrides any earlier `cert_acquisition_failed` and
            // vice-versa.
            let mut valid = false;
            for obs in &observations {
                match obs.obs_kind.as_str() {
                    "cert_valid" => valid = true,
                    "cert_acquisition_failed" => valid = false,
                    _ => {}
                }
            }
            valid
        })
    }
}

// ---------------------------------------------------------------------------
// Lifecycle derivation — shared helper
// ---------------------------------------------------------------------------

/// Walk `observations` in order, advancing state via `obs_to_state`.
///
/// Returns the final `LifecycleState` and the `recorded_at` timestamp (ms) of
/// the observation that caused the last state transition, or `None` if no
/// transition ever occurred (i.e. state stayed `Pending`).
fn drive_observations(
    observations: &[WorldObservation],
    obs_to_state: impl Fn(&str) -> Option<LifecycleState>,
) -> (LifecycleState, Option<i64>) {
    let mut state = LifecycleState::Pending;
    let mut transition_ms: Option<i64> = None;

    for obs in observations {
        let Some(next) = obs_to_state(&obs.obs_kind) else {
            continue;
        };

        // Terminated and Unscheduled are end-of-cycle states. Reset to
        // Pending so that subsequent observations (from a container
        // restart or a reinstall) start a fresh cycle.
        if state == LifecycleState::Terminated || state == LifecycleState::Unscheduled {
            state = LifecycleState::Pending;
            transition_ms = None;
        }

        // Already at or past this state — idempotent, skip.
        if state.has_reached(next) {
            continue;
        }

        // Advance if the transition is valid; skip anomalous observations.
        if state.can_transition_to(next) {
            state = next;
            transition_ms = Some(obs.recorded_at);
        }
    }

    (state, transition_ms)
}

// ---------------------------------------------------------------------------
// Lifecycle derivation — public API
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
    derive_lifecycle_with_ms(resource, observations).0
}

/// Like [`derive_lifecycle_state`], but also returns the time of the last state
/// transition. Returns `None` for the time if the resource is still `Pending`
/// (no transitions have occurred). Used for deadline and backoff calculations.
pub fn derive_state_with_transition_time(
    resource: &ResourceInstance,
    observations: &[WorldObservation],
) -> (LifecycleState, Option<SystemTime>) {
    let (state, ms) = derive_lifecycle_with_ms(resource, observations);
    let time = ms.map(|ms| UNIX_EPOCH + Duration::from_millis(ms as u64));
    (state, time)
}

fn derive_lifecycle_with_ms(
    resource: &ResourceInstance,
    observations: &[WorldObservation],
) -> (LifecycleState, Option<i64>) {
    match resource.kind {
        ResourceKind::Deployment | ResourceKind::Job => derive_container_lifecycle(observations),
        ResourceKind::Service | ResourceKind::HttpService => derive_service_lifecycle(observations),
        ResourceKind::Ingress => derive_ingress_lifecycle(observations),
        // r[impl lifecycle.volume]
        ResourceKind::Volume => derive_volume_lifecycle(observations),
        // r[impl lifecycle.external-volume]
        ResourceKind::ExternalVolume => derive_volume_lifecycle(observations),
        _ => (LifecycleState::Pending, None),
    }
}

// ---------------------------------------------------------------------------
// Per-kind derivation functions
// ---------------------------------------------------------------------------

// r[impl lifecycle.container]
fn derive_container_lifecycle(observations: &[WorldObservation]) -> (LifecycleState, Option<i64>) {
    drive_observations(observations, |kind| match kind {
        "container_created" | "image_pull_started" => Some(LifecycleState::Scheduled),
        "container_running" => Some(LifecycleState::Running),
        "health_check_pass" => Some(LifecycleState::Ready),
        "stop_sent" => Some(LifecycleState::Terminating),
        "container_exited" => Some(LifecycleState::Terminated),
        "container_removed" => Some(LifecycleState::Unscheduled),
        _ => None,
    })
}

// r[impl lifecycle.service]
fn derive_service_lifecycle(observations: &[WorldObservation]) -> (LifecycleState, Option<i64>) {
    drive_observations(observations, |kind| match kind {
        "network_created" => Some(LifecycleState::Scheduled),
        "backend_healthy" => Some(LifecycleState::Ready),
        "stop_sent" => Some(LifecycleState::Terminating),
        "network_removed" => Some(LifecycleState::Terminated),
        "network_cleaned_up" => Some(LifecycleState::Unscheduled),
        _ => None,
    })
}

// r[impl lifecycle.ingress]
fn derive_ingress_lifecycle(observations: &[WorldObservation]) -> (LifecycleState, Option<i64>) {
    drive_observations(observations, |kind| match kind {
        "ingress_configured" => Some(LifecycleState::Scheduled),
        "ingress_ready" => Some(LifecycleState::Ready),
        "stop_sent" => Some(LifecycleState::Terminating),
        "ingress_removed" => Some(LifecycleState::Terminated),
        "ingress_cleaned_up" => Some(LifecycleState::Unscheduled),
        _ => None,
    })
}

// r[impl lifecycle.volume]
fn derive_volume_lifecycle(observations: &[WorldObservation]) -> (LifecycleState, Option<i64>) {
    drive_observations(observations, |kind| match kind {
        "volume_created" => Some(LifecycleState::Scheduled),
        "volume_ready" => Some(LifecycleState::Ready),
        "stop_sent" => Some(LifecycleState::Terminating),
        "volume_removed" => Some(LifecycleState::Terminated),
        "volume_cleaned_up" => Some(LifecycleState::Unscheduled),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
