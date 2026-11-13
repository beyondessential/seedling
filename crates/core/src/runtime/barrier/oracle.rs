use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

use crate::defs::resource::ResourceKind;
use crate::runtime::history::WorldObservation;
use crate::runtime::{LifecycleState, ResourceInstance};

/// An instance's lifecycle state could not be derived, because its
/// observation history could not be read. Not an empty history, and not any
/// state.
// spec: LCY#derivation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Undetermined;

pub trait WorldStateOracle: Send + Sync {
    /// The instance's lifecycle state, or [`Undetermined`] while it cannot be
    /// read. A barrier is not decided on an undetermined instance.
    fn lifecycle_state(&self, resource: &ResourceInstance) -> Result<LifecycleState, Undetermined>;

    /// Returns true if a TLS certificate has been observed valid for the given
    /// ingress resource. Used by `rt.warm_certs(...).ready()` to wait on cert
    /// validity without coupling to the standard ingress `Ready` lifecycle
    /// (which also requires routing).
    // spec: OBS#ingresses
    fn cert_valid_for(&self, resource: &ResourceInstance) -> bool {
        let _ = resource;
        false
    }

    /// Returns true if a container image matching `reference` is currently
    /// present in local storage. Used by `rt.warm_images(...).ready()`.
    // spec: IMG#image-warming
    fn image_present(&self, reference: &str) -> bool {
        let _ = reference;
        false
    }

    /// Returns `Some(true)` when the resource has terminated successfully,
    /// `Some(false)` when it has terminated unsuccessfully, and `None` when
    /// the resource has not yet terminated or when success is not meaningful
    /// for this resource kind.
    ///
    /// For container-backed resources (Deployment, Job), success means the
    /// last `container_exited` observation recorded `exit_code == 0`. For
    /// Service/Ingress/Volume, termination is always success (the resource
    /// has no process exit code to inspect).
    // spec: RT#termination
    fn termination_success(&self, resource: &ResourceInstance) -> Option<bool> {
        let _ = resource;
        None
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
    exit_codes: Mutex<HashMap<(ResourceKind, Option<String>), i32>>,
    present_images: Mutex<std::collections::HashSet<String>>,
}

impl TestWorldOracle {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
            valid_certs: Mutex::new(std::collections::HashSet::new()),
            exit_codes: Mutex::new(HashMap::new()),
            present_images: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Mark `reference` as present locally for `image_present` queries.
    pub fn set_image_present(&self, reference: impl Into<String>) {
        self.present_images.lock().insert(reference.into());
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

    /// Record an exit code for a container-backed resource. Used by
    /// `termination_success()` to report success/failure.
    pub fn set_exit_code(&self, resource: ResourceInstance, exit_code: i32) {
        self.exit_codes
            .lock()
            .insert((resource.kind, resource.name), exit_code);
    }
}

impl Default for TestWorldOracle {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldStateOracle for TestWorldOracle {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> Result<LifecycleState, Undetermined> {
        Ok(self
            .states
            .lock()
            .get(&(resource.kind, resource.name.clone()))
            .copied()
            .unwrap_or(LifecycleState::Pending))
    }

    fn cert_valid_for(&self, resource: &ResourceInstance) -> bool {
        self.valid_certs
            .lock()
            .contains(&(resource.kind, resource.name.clone()))
    }

    fn image_present(&self, reference: &str) -> bool {
        self.present_images.lock().contains(reference)
    }

    // spec: RT#termination
    fn termination_success(&self, resource: &ResourceInstance) -> Option<bool> {
        match resource.kind {
            ResourceKind::Deployment | ResourceKind::Job => self
                .exit_codes
                .lock()
                .get(&(resource.kind, resource.name.clone()))
                .map(|&code| code == 0),
            ResourceKind::Service
            | ResourceKind::Ingress
            | ResourceKind::Volume
            | ResourceKind::ExternalVolume => {
                // Non-container resources: termination itself is success.
                // Only claim success once the resource is actually terminated.
                if matches!(
                    self.lifecycle_state(resource),
                    Ok(LifecycleState::Terminated | LifecycleState::Unscheduled)
                ) {
                    Some(true)
                } else {
                    None
                }
            }
            _ => None,
        }
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
    fn lifecycle_state(&self, resource: &ResourceInstance) -> Result<LifecycleState, Undetermined> {
        let resource = resource.clone();
        self.db.call(move |db| {
            match crate::runtime::history::query_observations(db, &resource) {
                Ok(observations) => Ok(derive_lifecycle_state(&resource, &observations)),
                Err(e) => {
                    tracing::warn!(
                        instance = %resource.display_name,
                        "cannot read an observation history; its lifecycle state is undetermined: {e}"
                    );
                    Err(Undetermined)
                }
            }
        })
    }

    // spec: IMG#image-warming
    fn image_present(&self, reference: &str) -> bool {
        let reference = reference.to_owned();
        self.db.call(move |db| {
            crate::runtime::images::reference_present(db, &reference).unwrap_or(false)
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

    // spec: RT#termination
    fn termination_success(&self, resource: &ResourceInstance) -> Option<bool> {
        let kind = resource.kind;
        let resource_clone = resource.clone();
        match kind {
            ResourceKind::Deployment | ResourceKind::Job => self.db.call(move |db| {
                let observations =
                    crate::runtime::history::query_observations(db, &resource_clone).ok()?;
                container_run_succeeded(&observations)
            }),
            ResourceKind::Service
            | ResourceKind::Ingress
            | ResourceKind::Volume
            | ResourceKind::ExternalVolume => {
                // Non-container resources: once terminated, treat as success.
                let resource_for_state = resource.clone();
                let state = self.db.call(move |db| {
                    crate::runtime::history::query_observations(db, &resource_for_state)
                        .map(|observations| {
                            derive_lifecycle_state(&resource_for_state, &observations)
                        })
                        .ok()
                });
                match state {
                    Some(LifecycleState::Terminated | LifecycleState::Unscheduled) => Some(true),
                    _ => None,
                }
            }
            _ => None,
        }
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
    drive_observations_with(observations, obs_to_state, |_| None)
}

/// As [`drive_observations`], for a kind of resource whose state can also go
/// back: `regression` names, for an observation kind, the state it takes a
/// resource out of and the one it returns it to.
fn drive_observations_with(
    observations: &[WorldObservation],
    obs_to_state: impl Fn(&str) -> Option<LifecycleState>,
    regression: impl Fn(&str) -> Option<(LifecycleState, LifecycleState)>,
) -> (LifecycleState, Option<i64>) {
    let mut state = LifecycleState::Pending;
    let mut transition_ms: Option<i64> = None;

    for obs in observations {
        if let Some((from, to)) = regression(&obs.obs_kind)
            && state == from
        {
            state = to;
            transition_ms = Some(obs.recorded_at);
            continue;
        }
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

/// How a container's latest run ended: `Some(true)` when it succeeded,
/// `Some(false)` when it failed, and `None` while it has not ended.
// spec: RT#termination
pub fn container_run_succeeded(observations: &[WorldObservation]) -> Option<bool> {
    // Find the terminal marker for the current run: the most
    // recent container_exited or container_removed. Anything
    // after the most recent container_running is what we care
    // about — an earlier failed run followed by a successful
    // retry must report success.
    let last_run_start = observations
        .iter()
        .rposition(|obs| obs.obs_kind == "container_running")
        .unwrap_or(0);
    let tail = &observations[last_run_start..];

    // Primary signal: if we captured container_exited with a
    // known exit code, trust it.
    if let Some(exit) = tail.iter().rev().find(|o| o.obs_kind == "container_exited") {
        let code = exit
            .payload
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        if code >= 0 {
            return Some(code == 0);
        }
        // exit_code = -1 means podman couldn't tell us. Fall
        // through to the unit-level signal.
    }

    // Secondary signal: if systemd observed the unit as failed,
    // the container process crashed/OOMed/got signalled, even
    // if --rm cleaned it up before we could inspect.
    if tail.iter().any(|o| o.obs_kind == "unit_failed") {
        return Some(false);
    }

    // Fallback: the exit status was never captured, as for a job removed
    // before it could be inspected. The run succeeded only if the supervisor
    // reported it so once it ended; until it reports either way the outcome
    // is not known.
    // spec: LINUX#exit-status
    let ended = tail.iter().rposition(|o| {
        matches!(
            o.obs_kind.as_str(),
            "container_exited" | "container_removed"
        )
    })?;
    tail[ended..]
        .iter()
        .any(|o| o.obs_kind == "unit_inactive")
        .then_some(true)
}

/// Whether instances of `kind` have a lifecycle state derived from what is
/// observed of them. Every other kind reads as `Pending` forever.
pub fn has_lifecycle(kind: ResourceKind) -> bool {
    match kind {
        ResourceKind::Deployment
        | ResourceKind::Job
        | ResourceKind::Service
        | ResourceKind::Ingress
        | ResourceKind::Volume
        | ResourceKind::ExternalVolume => true,
        // An external service is provided from outside the runtime, which
        // never observes it.
        ResourceKind::ExternalService | ResourceKind::Parameter | ResourceKind::Action => false,
    }
}

// spec: LCY#derivation
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
        ResourceKind::Service => derive_service_lifecycle(observations),
        ResourceKind::Ingress => derive_ingress_lifecycle(observations),
        // spec: LCY#volumes
        ResourceKind::Volume => derive_volume_lifecycle(observations),
        // spec: LCY#external-volumes
        ResourceKind::ExternalVolume => derive_volume_lifecycle(observations),
        _ => (LifecycleState::Pending, None),
    }
}

// ---------------------------------------------------------------------------
// Per-kind derivation functions
// ---------------------------------------------------------------------------

// spec: LCY#containers
fn derive_container_lifecycle(observations: &[WorldObservation]) -> (LifecycleState, Option<i64>) {
    let mut state = LifecycleState::Pending;
    let mut transition_ms: Option<i64> = None;

    for obs in observations {
        if state == LifecycleState::Terminated || state == LifecycleState::Unscheduled {
            state = LifecycleState::Pending;
            transition_ms = None;
        }

        // spec: LCY#containers
        // Health demotion: Ready containers observed as unhealthy drop back to
        // Running, and only to Running — other states are unaffected.
        if obs.obs_kind == "health_check_fail" {
            if state == LifecycleState::Ready {
                state = LifecycleState::Running;
                transition_ms = Some(obs.recorded_at);
            }
            continue;
        }

        let next = match obs.obs_kind.as_str() {
            "container_created" | "image_pull_started" => LifecycleState::Scheduled,
            "container_running" => LifecycleState::Running,
            "health_check_pass" => LifecycleState::Ready,
            "stop_sent" => LifecycleState::Terminating,
            "container_exited" => LifecycleState::Terminated,
            "container_removed" => LifecycleState::Unscheduled,
            _ => continue,
        };

        if state.has_reached(next) {
            continue;
        }
        if state.can_transition_to(next) {
            state = next;
            transition_ms = Some(obs.recorded_at);
        }
    }

    (state, transition_ms)
}

// spec: LCY#services
fn derive_service_lifecycle(observations: &[WorldObservation]) -> (LifecycleState, Option<i64>) {
    drive_observations_with(
        observations,
        |kind| match kind {
            "network_created" => Some(LifecycleState::Scheduled),
            "backend_healthy" => Some(LifecycleState::Ready),
            "stop_sent" => Some(LifecycleState::Terminating),
            "network_removed" => Some(LifecycleState::Terminated),
            "network_cleaned_up" => Some(LifecycleState::Unscheduled),
            _ => None,
        },
        // A pool that has emptied takes a Ready service back to Scheduled.
        |kind| match kind {
            "backend_lost" => Some((LifecycleState::Ready, LifecycleState::Scheduled)),
            _ => None,
        },
    )
}

// spec: LCY#ingresses
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

// spec: LCY#volumes
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
