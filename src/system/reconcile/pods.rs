use std::sync::Arc;

use ipnet::Ipv6Net;
use serde_json::json;
use tracing::error;

use crate::{
    defs::resource::Resource,
    runtime::{desired::DesiredState, identity::ResourceInstance, lifecycle::LifecycleState},
    system::{
        System, actuator::Actuator, observer::Observer, translate::proxy::pod_network_prefix,
        types::ObservationFact,
    },
};

use super::RunningPod;

pub(super) struct PodActuationUpdate {
    pub running: Vec<RunningPod>,
    pub observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    /// Instances whose image pull failed this tick, with the image reference.
    pub image_pull_failures: Vec<(ResourceInstance, String)>,
    /// Instances whose image pull succeeded this tick (or was already present).
    pub image_pull_successes: Vec<(ResourceInstance, String)>,
    /// Instances whose backing unit was observed in a failed state while desired active.
    pub unit_failures: Vec<ResourceInstance>,
    /// Instances whose backing unit was observed active/activating (clears prior failures).
    pub unit_healthy: Vec<ResourceInstance>,
}

// r[observe.deployment]
// r[actuate.deployment.start]
// r[actuate.deployment.stop]
// r[fault.non-blocking]
// r[fault.container-start]
pub(super) async fn observe_and_actuate(
    observer: &Observer,
    actuator: &Actuator,
    driver: &Arc<System>,
    desired: &DesiredState,
    node_prefix: &Ipv6Net,
) -> PodActuationUpdate {
    let mut running = Vec::new();
    let mut observations: Vec<(ResourceInstance, &'static str, serde_json::Value)> = Vec::new();
    let mut image_pull_failures: Vec<(ResourceInstance, String)> = Vec::new();
    let mut image_pull_successes: Vec<(ResourceInstance, String)> = Vec::new();
    let mut unit_failures: Vec<ResourceInstance> = Vec::new();
    let mut unit_healthy: Vec<ResourceInstance> = Vec::new();

    for dr in &desired.resources {
        match &dr.definition {
            Resource::Deployment(_) | Resource::Job(_) => {}
            _ => continue,
        }

        // Observe current state before any actuation this tick.
        let facts = match observer.observe(&dr.instance, &dr.definition).await {
            Ok(f) => f,
            Err(e) => {
                error!(
                    instance = %dr.instance.display_name,
                    error = %e,
                    "pods: observe failed, skipping instance"
                );
                continue;
            }
        };

        for (fact, _ts) in &facts {
            for (kind, payload) in fact.to_obs_kinds() {
                observations.push((dr.instance.clone(), kind, payload));
            }
        }

        let is_running = facts
            .iter()
            .any(|(f, _)| matches!(f, ObservationFact::ContainerRunning { .. }));
        let unit_failed = facts
            .iter()
            .any(|(f, _)| matches!(f, ObservationFact::UnitFailed));
        let unit_active = facts
            .iter()
            .any(|(f, _)| matches!(f, ObservationFact::UnitActive));
        let unit_loaded = facts.iter().any(|(f, _)| {
            matches!(
                f,
                ObservationFact::UnitActive
                    | ObservationFact::UnitInactive
                    | ObservationFact::UnitFailed
            )
        });

        // Track unit health for fault filing/clearing.
        if dr.desired == LifecycleState::Ready {
            if unit_active || is_running {
                unit_healthy.push(dr.instance.clone());
            } else if unit_failed {
                unit_failures.push(dr.instance.clone());
            }
        }

        // Collect running pods from the pre-actuation observation.
        //
        // A container started during this tick will not yet have a SLAAC
        // address assigned and will therefore appear in service routes only
        // on the next tick. This one-tick lag is intentional and idempotent.
        if is_running {
            match driver.container.inspect(&dr.instance.display_name).await {
                Ok(Some(state)) => {
                    if let Some(pod_ip) = state.pod_addr {
                        let pod_prefix = pod_network_prefix(node_prefix, &dr.instance);
                        running.push(RunningPod {
                            instance: dr.instance.clone(),
                            pod_prefix,
                            pod_ip,
                            resource: dr.definition.clone(),
                        });
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "pods: inspect failed while collecting running pod, skipping"
                    );
                }
            }
        }

        // Decide and actuate.
        match dr.desired {
            LifecycleState::Ready if !is_running => {
                // r[fault.container-start]
                // If the unit is in a failed state, skip the start attempt.
                // The failure is reported via unit_failures above; the
                // reconciler will file a fault. Retrying immediately would
                // just reset_failed + start_transient in a tight loop.
                if unit_failed {
                    continue;
                }

                let image_ref = match &dr.definition {
                    Resource::Deployment(dep) => {
                        dep.def.lock().pod.lock().container.lock().image.clone()
                    }
                    Resource::Job(job) => job.def.lock().pod.lock().container.lock().image.clone(),
                    _ => None,
                };
                match actuator.start(&dr.instance, &dr.definition).await {
                    Ok(Some(_)) | Ok(None) => {
                        if let Some(img) = image_ref {
                            image_pull_successes.push((dr.instance.clone(), img));
                        }
                    }
                    Err(crate::system::actuator::ActuateError::ImageUnavailable {
                        ref reference,
                    }) => {
                        error!(
                            instance = %dr.instance.display_name,
                            image = %reference,
                            "pods: image pull failed"
                        );
                        image_pull_failures.push((dr.instance.clone(), reference.clone()));
                    }
                    Err(e) => {
                        error!(
                            instance = %dr.instance.display_name,
                            error = %e,
                            "pods: start failed"
                        );
                    }
                }
            }
            LifecycleState::Unscheduled if is_running || unit_loaded => {
                observations.push((dr.instance.clone(), "stop_sent", json!({})));
                match actuator.stop(&dr.instance, &dr.definition).await {
                    Ok(()) => {}
                    Err(e) => {
                        error!(
                            instance = %dr.instance.display_name,
                            error = %e,
                            "pods: stop failed"
                        );
                    }
                }
            }
            _ => {}
        }
    }

    PodActuationUpdate {
        running,
        observations,
        image_pull_failures,
        image_pull_successes,
        unit_failures,
        unit_healthy,
    }
}
