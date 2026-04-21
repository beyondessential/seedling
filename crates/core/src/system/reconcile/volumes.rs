use futures_util::future::join_all;
use serde_json::json;
use tracing::error;

use crate::{
    defs::resource::Resource,
    runtime::{
        autonomous_ops, db::DbHandle,
        desired::{DesiredResource, DesiredState},
        identity::ResourceInstance,
        lifecycle::LifecycleState,
    },
    system::{actuator::Actuator, observer::Observer, types::ObservationFact},
};

struct VolumeInstanceResult {
    observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    observe_failure: Option<(ResourceInstance, String)>,
    create_failure: Option<(ResourceInstance, String)>,
    remove_failure: Option<(ResourceInstance, String)>,
}

pub(super) struct VolumeActuationUpdate {
    pub observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    pub observe_failures: Vec<(ResourceInstance, String)>,
    pub create_failures: Vec<(ResourceInstance, String)>,
    pub remove_failures: Vec<(ResourceInstance, String)>,
}

async fn process_one_volume(
    observer: &Observer,
    actuator: &Actuator,
    db: &DbHandle,
    dr: &DesiredResource,
) -> VolumeInstanceResult {
    let mut result = VolumeInstanceResult {
        observations: Vec::new(),
        observe_failure: None,
        create_failure: None,
        remove_failure: None,
    };

    // r[observe.volume]
    let facts = match observer.observe(&dr.instance, &dr.definition).await {
        Ok(f) => f,
        Err(e) => {
            error!(
                instance = %dr.instance.display_name,
                error = %e,
                "volumes: observe failed, skipping instance"
            );
            result.observe_failure = Some((dr.instance.clone(), e.to_string()));
            return result;
        }
    };

    for (fact, _ts) in &facts {
        for (kind, payload) in fact.to_obs_kinds() {
            result
                .observations
                .push((dr.instance.clone(), kind, payload));
        }
    }

    let volume_present = facts
        .iter()
        .any(|(f, _)| matches!(f, ObservationFact::VolumePresent));

    let backend_mismatch = facts
        .iter()
        .any(|(f, _)| matches!(f, ObservationFact::VolumeBackendMismatch));

    match dr.desired {
        // r[impl actuate.volume.hold]
        LifecycleState::Ready if backend_mismatch => {
            // Storage backend changed; hold old volume and recreate.
            let hold_op = autonomous_ops::record(
                db,
                &dr.instance,
                "volume_hold_for_migration",
                "Volume backend mismatch observed; holding existing volume before recreating with current storage backend",
            );
            let hold_outcome = match actuator
                .hold_volume(&dr.instance, &dr.definition, "storage backend changed")
                .await
            {
                Ok(()) => "ok".to_owned(),
                Err(e) => {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "volumes: hold for migration failed"
                    );
                    let msg = e.to_string();
                    result.remove_failure = Some((dr.instance.clone(), msg.clone()));
                    hold_op.complete(&format!("error: {msg}"));
                    return result;
                }
            };
            hold_op.complete(&hold_outcome);
            // Now create a fresh volume with the current backend.
            let create_op = autonomous_ops::record(
                db,
                &dr.instance,
                "volume_recreate_after_migration",
                "After holding mismatched volume, creating replacement with current backend",
            );
            let create_outcome = match actuator.start(&dr.instance, &dr.definition).await {
                Ok(_) => "ok".to_owned(),
                Err(e) => {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "volumes: recreate after migration failed"
                    );
                    let msg = e.to_string();
                    result.create_failure = Some((dr.instance.clone(), msg.clone()));
                    format!("error: {msg}")
                }
            };
            create_op.complete(&create_outcome);
        }
        LifecycleState::Ready if !volume_present => {
            // r[actuate.volume.start]
            let op = autonomous_ops::record(
                db,
                &dr.instance,
                "volume_create",
                "Volume desired=Ready but absent on disk; creating",
            );
            let outcome = match actuator.start(&dr.instance, &dr.definition).await {
                Ok(_) => "ok".to_owned(),
                Err(e) => {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "volumes: create failed"
                    );
                    let msg = e.to_string();
                    result.create_failure = Some((dr.instance.clone(), msg.clone()));
                    format!("error: {msg}")
                }
            };
            op.complete(&outcome);
        }
        LifecycleState::Unscheduled if volume_present || backend_mismatch => {
            result
                .observations
                .push((dr.instance.clone(), "stop_sent", json!({})));
            // r[actuate.volume.stop]
            let op = autonomous_ops::record(
                db,
                &dr.instance,
                "volume_remove",
                "Volume desired=Unscheduled but still present; removing",
            );
            let outcome = match actuator.stop(&dr.instance, &dr.definition).await {
                Ok(()) => "ok".to_owned(),
                Err(e) => {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "volumes: remove failed"
                    );
                    let msg = e.to_string();
                    result.remove_failure = Some((dr.instance.clone(), msg.clone()));
                    format!("error: {msg}")
                }
            };
            op.complete(&outcome);
        }
        _ => {}
    }

    result
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
        .map(|dr| process_one_volume(observer, actuator, db, dr))
        .collect();

    let results = join_all(futures).await;

    let mut update = VolumeActuationUpdate {
        observations: Vec::new(),
        observe_failures: Vec::new(),
        create_failures: Vec::new(),
        remove_failures: Vec::new(),
    };

    for result in results {
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

    // r[impl lifecycle.external-volume]
    // External volume resources are declarations only — nothing to create or
    // destroy. Drive them directly to the appropriate lifecycle state.
    for dr in desired
        .resources
        .iter()
        .filter(|dr| matches!(&dr.definition, Resource::ExternalVolume(_)))
    {
        let obs_kind = match dr.desired {
            LifecycleState::Ready => "volume_ready",
            LifecycleState::Unscheduled => "volume_cleaned_up",
            _ => continue,
        };
        update
            .observations
            .push((dr.instance.clone(), obs_kind, json!({})));
    }

    update
}
