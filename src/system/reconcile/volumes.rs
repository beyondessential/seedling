use tracing::error;

use crate::{
    defs::resource::Resource,
    runtime::{desired::DesiredState, lifecycle::LifecycleState},
    system::{actuator::Actuator, observer::Observer, types::ObservationFact},
};

// r[observe.volume]
// r[actuate.volume.start]
// r[actuate.volume.stop]
// r[fault.non-blocking]
pub(super) async fn observe_and_actuate(
    observer: &Observer,
    actuator: &Actuator,
    desired: &DesiredState,
) {
    for dr in &desired.resources {
        match &dr.definition {
            Resource::Volume(_) => {}
            // ExternalVolume and ExternalService are no-ops in this phase.
            _ => continue,
        }

        let facts = match observer.observe(&dr.instance, &dr.definition).await {
            Ok(f) => f,
            Err(e) => {
                error!(
                    instance = %dr.instance.display_name,
                    error = %e,
                    "volumes: observe failed, skipping instance"
                );
                continue;
            }
        };

        let volume_present = facts
            .iter()
            .any(|(f, _)| matches!(f, ObservationFact::VolumePresent));

        match dr.desired {
            LifecycleState::Ready if !volume_present => {
                if let Err(e) = actuator.start(&dr.instance, &dr.definition).await {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "volumes: create failed"
                    );
                }
            }
            LifecycleState::Unscheduled if volume_present => {
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
    }
}
