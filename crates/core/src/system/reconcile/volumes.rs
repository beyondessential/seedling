use futures_util::future::join_all;
use serde_json::json;
use tracing::error;

use crate::{
    defs::resource::{Resource, ResourceKind},
    runtime::{
        autonomous_ops,
        db::DbHandle,
        desired::{DesiredResource, DesiredState},
        identity::ResourceInstance,
        lifecycle::LifecycleState,
    },
    system::{actuator::Actuator, observer::Observer, types::ObservationFact},
};

struct VolumeInstanceResult {
    observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    observe_failure: Option<(ResourceInstance, String)>,
    observed: Option<ResourceInstance>,
    create_failure: Option<(ResourceInstance, String)>,
    remove_failure: Option<(ResourceInstance, String)>,
    /// The volume's storage is observed present or was created this tick.
    present: bool,
    /// The volume was removed or held this tick.
    removed: bool,
}

pub(super) struct VolumeActuationUpdate {
    pub observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    /// The lifecycle state the desired state gives each external volume.
    // spec: LCY#external-volumes
    pub external_volume_states: Vec<(ResourceInstance, LifecycleState)>,
    pub observe_failures: Vec<(ResourceInstance, String)>,
    /// Instances observed successfully this tick.
    // spec: OBS#failed-observation
    pub observed: Vec<ResourceInstance>,
    pub create_failures: Vec<(ResourceInstance, String)>,
    pub remove_failures: Vec<(ResourceInstance, String)>,
    /// Instances whose storage is observed present, or was created, this tick.
    // spec: FLT#volume-faults
    pub present: Vec<ResourceInstance>,
    /// Instances removed or held successfully this tick.
    // spec: FLT#volume-faults
    pub removed: Vec<ResourceInstance>,
}

impl VolumeActuationUpdate {
    fn empty() -> Self {
        Self {
            observations: Vec::new(),
            external_volume_states: Vec::new(),
            observe_failures: Vec::new(),
            observed: Vec::new(),
            create_failures: Vec::new(),
            remove_failures: Vec::new(),
            present: Vec::new(),
            removed: Vec::new(),
        }
    }

    pub(super) fn external_volumes_only(desired: &DesiredState) -> Self {
        Self {
            external_volume_states: external_volume_states(desired),
            ..Self::empty()
        }
    }
}

/// A volume instance the volumes phase acts on.
#[derive(Clone, Copy)]
enum VolumeTarget<'a> {
    /// One a definition declares, in the state the desired state gives it.
    Declared(&'a DesiredResource),
    /// One nothing declares, held `Unscheduled` and torn down from its
    /// recorded identity.
    // spec: TRD#teardown
    Undeclared(&'a ResourceInstance),
}

impl<'a> VolumeTarget<'a> {
    fn instance(self) -> &'a ResourceInstance {
        match self {
            Self::Declared(dr) => &dr.instance,
            Self::Undeclared(instance) => instance,
        }
    }

    fn desired(self) -> LifecycleState {
        match self {
            Self::Declared(dr) => dr.desired,
            Self::Undeclared(_) => LifecycleState::Unscheduled,
        }
    }

    fn definition(self) -> Option<&'a Resource> {
        match self {
            Self::Declared(dr) => Some(&dr.definition),
            Self::Undeclared(_) => None,
        }
    }
}

async fn process_one_volume(
    observer: &Observer,
    actuator: &Actuator,
    db: &DbHandle,
    target: VolumeTarget<'_>,
) -> VolumeInstanceResult {
    let instance = target.instance();
    let mut result = VolumeInstanceResult {
        observations: Vec::new(),
        observe_failure: None,
        observed: None,
        create_failure: None,
        remove_failure: None,
        present: false,
        removed: false,
    };

    // spec: OBS#volumes
    let observed = match target {
        VolumeTarget::Declared(dr) => observer.observe(&dr.instance, &dr.definition).await,
        VolumeTarget::Undeclared(instance) => observer.observe_undeclared_volume(instance),
    };
    let facts = match observed {
        Ok(f) => f,
        Err(e) => {
            error!(
                instance = %instance.display_name,
                error = %e,
                "volumes: observe failed, skipping instance"
            );
            result.observe_failure = Some((instance.clone(), e.to_string()));
            return result;
        }
    };

    result.observed = Some(instance.clone());
    for (fact, _ts) in &facts {
        for (kind, payload) in fact.to_obs_kinds() {
            result.observations.push((instance.clone(), kind, payload));
        }
    }

    let volume_present = facts
        .iter()
        .any(|(f, _)| matches!(f, ObservationFact::VolumePresent));

    let backend_mismatch = facts
        .iter()
        .any(|(f, _)| matches!(f, ObservationFact::VolumeBackendMismatch));
    result.present = volume_present && !backend_mismatch;

    match (target.desired(), target.definition()) {
        // spec: ACTU#held-volumes
        (LifecycleState::Ready, Some(definition)) if backend_mismatch => {
            // Storage backend changed; hold old volume and recreate.
            let Some(hold_op) = autonomous_ops::record(
                db,
                instance,
                "volume_hold_for_migration",
                "Volume backend mismatch observed; holding existing volume before recreating with current storage backend",
            ) else {
                return result;
            };
            let hold_outcome = match actuator
                .hold_volume(instance, definition, "storage backend changed")
                .await
            {
                Ok(()) => {
                    result.removed = true;
                    autonomous_ops::OK.to_owned()
                }
                Err(e) => {
                    error!(
                        instance = %instance.display_name,
                        error = %e,
                        "volumes: hold for migration failed"
                    );
                    let msg = e.to_string();
                    result.remove_failure = Some((instance.clone(), msg.clone()));
                    hold_op.complete(&format!("error: {msg}"));
                    return result;
                }
            };
            hold_op.complete(&hold_outcome);
            // Now create a fresh volume with the current backend.
            let Some(create_op) = autonomous_ops::record(
                db,
                instance,
                "volume_recreate_after_migration",
                "After holding mismatched volume, creating replacement with current backend",
            ) else {
                return result;
            };
            let create_outcome = match actuator.start_volume(instance, definition).await {
                Ok(_) => {
                    result.present = true;
                    autonomous_ops::OK.to_owned()
                }
                Err(e) => {
                    error!(
                        instance = %instance.display_name,
                        error = %e,
                        "volumes: recreate after migration failed"
                    );
                    let msg = e.to_string();
                    result.create_failure = Some((instance.clone(), msg.clone()));
                    format!("error: {msg}")
                }
            };
            create_op.complete(&create_outcome);
        }
        (LifecycleState::Ready, Some(definition)) if !volume_present => {
            // spec: ACTU#volume-creation-and-seeding
            let Some(op) = autonomous_ops::record(
                db,
                instance,
                "volume_create",
                "Volume desired=Ready but absent on disk; creating",
            ) else {
                return result;
            };
            let outcome = match actuator.start_volume(instance, definition).await {
                Ok(_) => {
                    result.present = true;
                    autonomous_ops::OK.to_owned()
                }
                Err(e) => {
                    error!(
                        instance = %instance.display_name,
                        error = %e,
                        "volumes: create failed"
                    );
                    let msg = e.to_string();
                    result.create_failure = Some((instance.clone(), msg.clone()));
                    format!("error: {msg}")
                }
            };
            op.complete(&outcome);
        }
        (LifecycleState::Unscheduled, definition) if volume_present || backend_mismatch => {
            result
                .observations
                .push((instance.clone(), "stop_sent", json!({})));
            // spec: ACTU#volume-removal
            let Some(op) = autonomous_ops::record(
                db,
                instance,
                "volume_remove",
                "Volume desired=Unscheduled but still present; removing",
            ) else {
                return result;
            };
            let stopped = match definition {
                Some(definition) => actuator.stop(instance, definition).await,
                None => actuator.stop_undeclared_volume(instance).await,
            };
            let outcome = match stopped {
                Ok(()) => {
                    result.removed = true;
                    autonomous_ops::OK.to_owned()
                }
                Err(e) => {
                    error!(
                        instance = %instance.display_name,
                        error = %e,
                        "volumes: remove failed"
                    );
                    let msg = e.to_string();
                    result.remove_failure = Some((instance.clone(), msg.clone()));
                    format!("error: {msg}")
                }
            };
            op.complete(&outcome);
        }
        _ => {}
    }

    result
}

/// The lifecycle state the desired state gives each external volume.
///
/// External volume resources are declarations only: nothing to create or
/// destroy, and nothing to observe.
// spec: LCY#external-volumes
pub(super) fn external_volume_states(
    desired: &DesiredState,
) -> Vec<(ResourceInstance, LifecycleState)> {
    desired
        .resources
        .iter()
        .filter(|dr| matches!(&dr.definition, Resource::ExternalVolume(_)))
        .filter(|dr| {
            matches!(
                dr.desired,
                LifecycleState::Ready | LifecycleState::Unscheduled
            )
        })
        .map(|dr| (dr.instance.clone(), dr.desired))
        // spec: TRD#teardown
        .chain(
            desired
                .undeclared
                .iter()
                .filter(|instance| instance.kind == ResourceKind::ExternalVolume)
                .map(|instance| (instance.clone(), LifecycleState::Unscheduled)),
        )
        .collect()
}

pub(super) async fn observe_and_actuate(
    observer: &Observer,
    actuator: &Actuator,
    db: &DbHandle,
    desired: &DesiredState,
) -> VolumeActuationUpdate {
    let futures: Vec<_> = desired
        .resources
        .iter()
        .filter(|dr| matches!(&dr.definition, Resource::Volume(_)))
        .map(VolumeTarget::Declared)
        .chain(
            desired
                .undeclared
                .iter()
                .filter(|instance| instance.kind == ResourceKind::Volume)
                .map(VolumeTarget::Undeclared),
        )
        .map(|target| process_one_volume(observer, actuator, db, target))
        .collect();

    let results = join_all(futures).await;

    let mut update = VolumeActuationUpdate::empty();

    for result in results {
        if result.present
            && let Some(instance) = &result.observed
        {
            update.present.push(instance.clone());
        }
        if result.removed
            && let Some(instance) = &result.observed
        {
            update.removed.push(instance.clone());
        }
        if let Some(instance) = result.observed {
            update.observed.push(instance);
        }
        update.observations.extend(result.observations);
        if let Some(f) = result.observe_failure {
            update.observe_failures.push(f);
        }
        if let Some(f) = result.create_failure {
            update.create_failures.push(f);
        }
        if let Some(f) = result.remove_failure {
            update.remove_failures.push(f);
        }
    }

    update.external_volume_states = external_volume_states(desired);

    update
}
