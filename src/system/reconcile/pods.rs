use std::sync::Arc;

use futures_util::future::join_all;
use ipnet::Ipv6Net;
use serde_json::json;
use tracing::error;

use crate::{
    defs::resource::Resource,
    runtime::{
        desired::{DesiredResource, DesiredState},
        identity::ResourceInstance,
        lifecycle::LifecycleState,
    },
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

struct PodInstanceResult {
    running: Option<RunningPod>,
    observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    image_pull_failure: Option<(ResourceInstance, String)>,
    image_pull_success: Option<(ResourceInstance, String)>,
    unit_failure: Option<ResourceInstance>,
    unit_healthy: Option<ResourceInstance>,
}

// r[observe.deployment]
// r[actuate.deployment.start]
// r[actuate.deployment.stop]
// r[fault.non-blocking]
// r[fault.container-start]
async fn process_one_pod(
    observer: &Observer,
    actuator: &Actuator,
    driver: &Arc<System>,
    dr: &DesiredResource,
    node_prefix: &Ipv6Net,
) -> Option<PodInstanceResult> {
    let mut result = PodInstanceResult {
        running: None,
        observations: Vec::new(),
        image_pull_failure: None,
        image_pull_success: None,
        unit_failure: None,
        unit_healthy: None,
    };

    // Observe current state before any actuation this tick.
    let facts = match observer.observe(&dr.instance, &dr.definition).await {
        Ok(f) => f,
        Err(e) => {
            error!(
                instance = %dr.instance.display_name,
                error = %e,
                "pods: observe failed, skipping instance"
            );
            return None;
        }
    };

    for (fact, _ts) in &facts {
        for (kind, payload) in fact.to_obs_kinds() {
            result
                .observations
                .push((dr.instance.clone(), kind, payload));
        }
    }

    let is_running = facts
        .iter()
        .any(|(f, _)| matches!(f, ObservationFact::ContainerRunning { .. }));
    // Extract the spec hash observed on the running container, if any.
    let observed_spec_hash = facts.iter().find_map(|(f, _)| {
        if let ObservationFact::ContainerSpecHash(h) = f {
            Some(h.clone())
        } else {
            None
        }
    });

    // Compute the desired spec hash from the current AppDef.
    let desired_spec_hash = if is_running {
        actuator.desired_spec_hash(&dr.instance, &dr.definition)
    } else {
        None
    };

    // The spec is stale when both hashes are known and they differ.
    let spec_stale = match (observed_spec_hash, desired_spec_hash) {
        (Some(observed), Some(desired)) => observed != desired,
        _ => false,
    };

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
    // A unit that is "active" in systemd but whose container is not
    // running (e.g. exited inside a restarting unit) is not healthy —
    // it is stuck in a crash loop managed by systemd's restart logic.
    // A stale spec is also not considered healthy.
    if dr.desired == LifecycleState::Ready {
        if is_running && !spec_stale {
            result.unit_healthy = Some(dr.instance.clone());
        } else if unit_failed || (unit_active && !is_running) {
            result.unit_failure = Some(dr.instance.clone());
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
                    result.running = Some(RunningPod {
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
        LifecycleState::Ready if !is_running || spec_stale => {
            // r[fault.container-start]
            // If the unit is in a broken state (failed, or active but the
            // container is not running), or the running container has a
            // stale spec, tear it down so the next tick can start a fresh
            // unit with the current AppDef config.
            if unit_failed || unit_active || spec_stale {
                match actuator.stop(&dr.instance, &dr.definition).await {
                    Ok(()) => {}
                    Err(e) => {
                        error!(
                            instance = %dr.instance.display_name,
                            error = %e,
                            "pods: stop broken unit failed"
                        );
                    }
                }
                return Some(result);
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
                        result.image_pull_success = Some((dr.instance.clone(), img));
                    }
                }
                Err(crate::system::actuator::ActuateError::ImageUnavailable {
                    ref reference,
                    ..
                }) => {
                    error!(
                        instance = %dr.instance.display_name,
                        image = %reference,
                        "pods: image pull failed"
                    );
                    result.image_pull_failure = Some((dr.instance.clone(), reference.clone()));
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
            result
                .observations
                .push((dr.instance.clone(), "stop_sent", json!({})));
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

    Some(result)
}

// r[impl reconciliation.liveness]
pub(super) async fn observe_and_actuate(
    observer: &Observer,
    actuator: &Actuator,
    driver: &Arc<System>,
    desired: &DesiredState,
    node_prefix: &Ipv6Net,
) -> PodActuationUpdate {
    let futures: Vec<_> = desired
        .resources
        .iter()
        .filter(|dr| matches!(&dr.definition, Resource::Deployment(_) | Resource::Job(_)))
        .map(|dr| process_one_pod(observer, actuator, driver, dr, node_prefix))
        .collect();

    let results = join_all(futures).await;

    let mut update = PodActuationUpdate {
        running: Vec::new(),
        observations: Vec::new(),
        image_pull_failures: Vec::new(),
        image_pull_successes: Vec::new(),
        unit_failures: Vec::new(),
        unit_healthy: Vec::new(),
    };

    for result in results.into_iter().flatten() {
        if let Some(rp) = result.running {
            update.running.push(rp);
        }
        update.observations.extend(result.observations);
        if let Some(f) = result.image_pull_failure {
            update.image_pull_failures.push(f);
        }
        if let Some(s) = result.image_pull_success {
            update.image_pull_successes.push(s);
        }
        if let Some(f) = result.unit_failure {
            update.unit_failures.push(f);
        }
        if let Some(h) = result.unit_healthy {
            update.unit_healthy.push(h);
        }
    }

    update
}
