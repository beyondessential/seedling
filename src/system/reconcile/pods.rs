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

/// Returned from `observe_and_actuate` so the `Reconciler` can maintain the
/// bridge-name map without giving this module a mutable reference to it.
pub(super) struct PodActuationUpdate {
    pub running: Vec<RunningPod>,
    /// Networks created this tick: `(network_name, bridge_interface_name)`.
    pub new_bridges: Vec<(String, String)>,
    /// Network names removed this tick (the pod was stopped).
    pub removed_networks: Vec<String>,
    pub observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
}

fn pod_network_name(instance: &crate::runtime::identity::ResourceInstance) -> String {
    format!("seedling-{}", instance.display_name)
}

// r[observe.deployment]
// r[actuate.deployment.start]
// r[actuate.deployment.stop]
// r[fault.non-blocking]
pub(super) async fn observe_and_actuate(
    observer: &Observer,
    actuator: &Actuator,
    driver: &Arc<System>,
    desired: &DesiredState,
    node_prefix: &Ipv6Net,
) -> PodActuationUpdate {
    let mut running = Vec::new();
    let mut new_bridges = Vec::new();
    let mut removed_networks = Vec::new();
    let mut observations: Vec<(ResourceInstance, &'static str, serde_json::Value)> = Vec::new();

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
        let unit_loaded = facts.iter().any(|(f, _)| {
            matches!(
                f,
                ObservationFact::UnitActive
                    | ObservationFact::UnitInactive
                    | ObservationFact::UnitFailed
            )
        });

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
                match actuator.start(&dr.instance, &dr.definition).await {
                    Ok(Some(bridge_name)) => {
                        new_bridges.push((pod_network_name(&dr.instance), bridge_name));
                    }
                    Ok(None) => {}
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
                    Ok(()) => {
                        removed_networks.push(pod_network_name(&dr.instance));
                    }
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
        new_bridges,
        removed_networks,
        observations,
    }
}
