use std::sync::Arc;

use ipnet::Ipv6Net;
use tracing::error;

use crate::{
    defs::resource::Resource,
    runtime::{desired::DesiredState, lifecycle::LifecycleState},
    system::{
        System, actuator::Actuator, observer::Observer, translate::proxy::pod_network_prefix,
        types::ObservationFact,
    },
};

use super::RunningPod;

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
) -> Vec<RunningPod> {
    let mut running_pods = Vec::new();

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

        let is_running = facts
            .iter()
            .any(|(f, _)| matches!(f, ObservationFact::ContainerRunning { .. }));
        let unit_active = facts
            .iter()
            .any(|(f, _)| matches!(f, ObservationFact::UnitActive));

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
                        running_pods.push(RunningPod {
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
                if let Err(e) = actuator.start(&dr.instance, &dr.definition).await {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "pods: start failed"
                    );
                }
            }
            LifecycleState::Unscheduled if is_running || unit_active => {
                if let Err(e) = actuator.stop(&dr.instance, &dr.definition).await {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "pods: stop failed"
                    );
                }
            }
            _ => {}
        }
    }

    running_pods
}
