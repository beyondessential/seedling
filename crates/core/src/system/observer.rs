use std::sync::Arc;
use std::time::SystemTime;

use snafu::{ResultExt, Snafu};

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
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("process manager: {source}"))]
    Process {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("proxy: {source}"))]
    Proxy {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("data plane: {source}"))]
    DataPlane {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
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
            Resource::Volume(vol) => {
                // r[impl observe.volume]
                let name = &instance.display_name;
                let tmpfs = vol.def.lock().tmpfs;
                if tmpfs {
                    let exists = self
                        .driver
                        .container
                        .volume_exists(name)
                        .await
                        .context(ContainerSnafu)?;
                    facts.push((
                        if exists {
                            ObservationFact::VolumePresent
                        } else {
                            ObservationFact::VolumeMissing
                        },
                        now,
                    ));
                } else {
                    let vol_store = &self.driver.volume_store;
                    let vol_name = crate::runtime::identity::VolumeName::of_instance(instance);
                    if vol_store.exists(&vol_name) {
                        // r[impl observe.volume.backend-mismatch]
                        if vol_store.is_backend_match(&vol_name).await {
                            facts.push((ObservationFact::VolumePresent, now));
                        } else {
                            facts.push((ObservationFact::VolumeBackendMismatch, now));
                        }
                    } else {
                        facts.push((ObservationFact::VolumeMissing, now));
                    }
                }
            }
            Resource::Ingress(_) => {
                // r[impl observe.ingress]
                let healthy = self.driver.proxy.is_healthy().await.context(ProxySnafu)?;
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
            | Resource::ExternalVolume(_)
            | Resource::ExternalService(_) => {
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
        let unit = unit_name(instance);

        let (net_exists, container_state, unit_state) = tokio::try_join!(
            async {
                self.driver
                    .container
                    .network_exists(&net_name)
                    .await
                    .context(ContainerSnafu)
            },
            async {
                self.driver
                    .container
                    .inspect(&instance.display_name)
                    .await
                    .context(ContainerSnafu)
            },
            async {
                self.driver
                    .process
                    .unit_state(&unit)
                    .await
                    .context(ProcessSnafu)
            },
        )?;

        facts.push((
            if net_exists {
                ObservationFact::NetworkPresent
            } else {
                ObservationFact::NetworkMissing
            },
            now,
        ));

        match container_state {
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
