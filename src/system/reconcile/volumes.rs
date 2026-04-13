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

async fn process_one_volume(
    observer: &Observer,
    actuator: &Actuator,
    dr: &DesiredResource,
) -> Option<Vec<(ResourceInstance, &'static str, serde_json::Value)>> {
    let mut observations: Vec<(ResourceInstance, &'static str, serde_json::Value)> = Vec::new();

    // r[observe.volume]
    let facts = match observer.observe(&dr.instance, &dr.definition).await {
        Ok(f) => f,
        Err(e) => {
            error!(
                instance = %dr.instance.display_name,
                error = %e,
                "volumes: observe failed, skipping instance"
            );
            return None;
        }
    };

    for (fact, _ts) in &facts {
        for (kind, payload) in fact.to_obs_kinds() {
            observations.push((dr.instance.clone(), kind, payload));
        }
    }

    let volume_present = facts
        .iter()
        .any(|(f, _)| matches!(f, ObservationFact::VolumePresent));

    match dr.desired {
        LifecycleState::Ready if !volume_present => {
            // r[actuate.volume.start]
            if let Err(e) = actuator.start(&dr.instance, &dr.definition).await {
                error!(
                    instance = %dr.instance.display_name,
                    error = %e,
                    "volumes: create failed"
                );
            }
        }
        LifecycleState::Unscheduled if volume_present => {
            observations.push((dr.instance.clone(), "stop_sent", json!({})));
            // r[actuate.volume.stop]
            if let Err(e) = actuator.stop(&dr.instance, &dr.definition).await {
                error!(
                    instance = %dr.instance.display_name,
                    error = %e,
                    "volumes: remove failed"
                );
            }
        }
        _ => {}
    }

    Some(observations)
}

pub(super) async fn observe_and_actuate(
    observer: &Observer,
    actuator: &Actuator,
    desired: &DesiredState,
) -> Vec<(ResourceInstance, &'static str, serde_json::Value)> {
    let futures: Vec<_> = desired
        .resources
        .iter()
        .filter(|dr| matches!(&dr.definition, Resource::Volume(_)))
        .map(|dr| process_one_volume(observer, actuator, dr))
        .collect();

    let results = join_all(futures).await;

    results.into_iter().flatten().flatten().collect()
}
