use futures_util::future::join_all;
use serde_json::json;
use tracing::error;

use crate::{
    defs::resource::Resource,
    runtime::{
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
            if let Err(e) = actuator
                .hold_volume(&dr.instance, &dr.definition, "storage backend changed")
                .await
            {
                error!(
                    instance = %dr.instance.display_name,
                    error = %e,
                    "volumes: hold for migration failed"
                );
                result.remove_failure = Some((dr.instance.clone(), e.to_string()));
                return result;
            }
            // Now create a fresh volume with the current backend.
            if let Err(e) = actuator.start(&dr.instance, &dr.definition).await {
                error!(
                    instance = %dr.instance.display_name,
                    error = %e,
                    "volumes: recreate after migration failed"
                );
                result.create_failure = Some((dr.instance.clone(), e.to_string()));
            }
        }
        LifecycleState::Ready if !volume_present => {
            // r[actuate.volume.start]
            if let Err(e) = actuator.start(&dr.instance, &dr.definition).await {
                error!(
                    instance = %dr.instance.display_name,
                    error = %e,
                    "volumes: create failed"
                );
                result.create_failure = Some((dr.instance.clone(), e.to_string()));
            }
        }
        LifecycleState::Unscheduled if volume_present || backend_mismatch => {
            result
                .observations
                .push((dr.instance.clone(), "stop_sent", json!({})));
            // r[actuate.volume.stop]
            if let Err(e) = actuator.stop(&dr.instance, &dr.definition).await {
                error!(
                    instance = %dr.instance.display_name,
                    error = %e,
                    "volumes: remove failed"
                );
                result.remove_failure = Some((dr.instance.clone(), e.to_string()));
            }
        }
        _ => {}
    }

    result
}

pub(super) async fn observe_and_actuate(
    observer: &Observer,
    actuator: &Actuator,
    desired: &DesiredState,
) -> VolumeActuationUpdate {
    let futures: Vec<_> = desired
        .resources
        .iter()
        .filter(|dr| matches!(&dr.definition, Resource::Volume(_)))
        .map(|dr| process_one_volume(observer, actuator, dr))
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

    update
}
