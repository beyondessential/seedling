use std::sync::Arc;
use std::time::SystemTime;

use snafu::Snafu;

use crate::{
    defs::resource::Resource,
    runtime::identity::ResourceInstance,
    system::{
        System,
        types::{ActiveState, ContainerHealth, ContainerStatus, ObservationFact},
    },
};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Observation-time error. The backend variant is intentionally erased:
/// callers see `ObserveError::Container` but cannot match on internal types.
#[derive(Debug, Snafu)]
pub enum ObserveError {
    #[snafu(display("container backend: {source}"))]
    Container {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("process manager: {source}"))]
    Process {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("proxy: {source}"))]
    Proxy {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("data plane: {source}"))]
    DataPlane {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

// ---------------------------------------------------------------------------
// Naming helpers
// ---------------------------------------------------------------------------

fn pod_network_name(instance: &ResourceInstance) -> String {
    format!("seedling-{}", instance.display_name)
}

fn unit_name(instance: &ResourceInstance) -> String {
    format!("seedling-{}.service", instance.display_name)
}

// ---------------------------------------------------------------------------
// Observer
// ---------------------------------------------------------------------------

pub struct Observer {
    driver: Arc<System>,
}

impl Observer {
    pub fn new(driver: Arc<System>) -> Self {
        Self { driver }
    }

    // r[impl observe.facts]
    /// Inspect all system primitives backing one resource instance.
    ///
    /// Returns timestamped facts; the reconciler loop persists them to
    /// `world_observations`.
    pub async fn observe(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Result<Vec<(ObservationFact, SystemTime)>, ObserveError> {
        let now = SystemTime::now();
        let mut facts = Vec::new();

        match resource {
            Resource::Deployment(_) | Resource::Job(_) => {
                self.observe_pod_instance(instance, resource, now, &mut facts)
                    .await?;
            }
            Resource::Volume(_) => {
                // r[impl observe.volume]
                let name = &instance.display_name;
                let exists = self
                    .driver
                    .container
                    .volume_exists(name)
                    .await
                    .map_err(|e| ObserveError::Container { source: e })?;
                facts.push((
                    if exists {
                        ObservationFact::VolumePresent
                    } else {
                        ObservationFact::VolumeMissing
                    },
                    now,
                ));
            }
            Resource::Ingress(_) => {
                // r[impl observe.ingress]
                let healthy = self
                    .driver
                    .proxy
                    .is_healthy()
                    .await
                    .map_err(|e| ObserveError::Proxy { source: e })?;
                facts.push((
                    if healthy {
                        ObservationFact::ProxyReachable
                    } else {
                        ObservationFact::ProxyUnreachable
                    },
                    now,
                ));
            }
            Resource::Service(_)
            | Resource::HttpService(_)
            | Resource::ExternalService(_)
            | Resource::ExternalVolume(_) => {
                // No directly observable system primitives via the current trait interfaces.
            }
        }

        Ok(facts)
    }

    // r[impl observe.deployment]
    async fn observe_pod_instance(
        &self,
        instance: &ResourceInstance,
        _resource: &Resource,
        now: SystemTime,
        facts: &mut Vec<(ObservationFact, SystemTime)>,
    ) -> Result<(), ObserveError> {
        let net_name = pod_network_name(instance);
        let net_exists = self
            .driver
            .container
            .network_exists(&net_name)
            .await
            .map_err(|e| ObserveError::Container { source: e })?;
        facts.push((
            if net_exists {
                ObservationFact::NetworkPresent
            } else {
                ObservationFact::NetworkMissing
            },
            now,
        ));

        let state = self
            .driver
            .container
            .inspect(&instance.display_name)
            .await
            .map_err(|e| ObserveError::Container { source: e })?;

        match state {
            None => facts.push((ObservationFact::ContainerMissing, now)),
            Some(ref s) => {
                let lifecycle_fact = match s.status {
                    ContainerStatus::Created => ObservationFact::ContainerCreated,
                    ContainerStatus::Running => ObservationFact::ContainerRunning {
                        pid: s.pid.unwrap_or(0),
                    },
                    ContainerStatus::Paused => ObservationFact::ContainerCreated,
                    ContainerStatus::Exited => ObservationFact::ContainerExited {
                        exit_code: s.exit_code.unwrap_or(-1),
                    },
                    ContainerStatus::Unknown => ObservationFact::ContainerMissing,
                };
                facts.push((lifecycle_fact, now));

                if s.status == ContainerStatus::Running
                    && let Some(hash) = &s.spec_hash
                {
                    facts.push((ObservationFact::ContainerSpecHash(hash.clone()), now));
                }

                match s.health {
                    ContainerHealth::Healthy => {
                        facts.push((ObservationFact::ContainerHealthy, now));
                    }
                    ContainerHealth::Unhealthy => {
                        facts.push((ObservationFact::ContainerUnhealthy, now));
                    }
                    ContainerHealth::None if s.status == ContainerStatus::Running => {
                        // No health check configured — a running container is
                        // implicitly healthy and therefore Ready.
                        facts.push((ObservationFact::ContainerHealthy, now));
                    }
                    _ => {}
                }
            }
        }

        let unit_state = self
            .driver
            .process
            .unit_state(&unit_name(instance))
            .await
            .map_err(|e| ObserveError::Process { source: e })?;

        let unit_fact = match unit_state.as_ref().map(|s| s.active) {
            None => ObservationFact::UnitGone,
            Some(ActiveState::Inactive) | Some(ActiveState::Deactivating) => {
                ObservationFact::UnitInactive
            }
            Some(ActiveState::Active) | Some(ActiveState::Activating) => {
                ObservationFact::UnitActive
            }
            Some(ActiveState::Failed) => ObservationFact::UnitFailed,
        };
        facts.push((unit_fact, now));

        Ok(())
    }
}
