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
                self.observe_pod_instance(instance, now, &mut facts).await?;
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
                    // r[impl observe.failure-not-absence] — this arm has
                    // already proven the container exists: the inspect
                    // returned it. Mapping either of these to
                    // ContainerMissing recorded `container_removed`, which
                    // the oracle reads as the transition to Unscheduled and
                    // termination_success reads as terminal success — so a
                    // container still draining through its stop timeout
                    // released barriers over volumes and networks it held.
                    ContainerStatus::Stopping | ContainerStatus::Unknown => {
                        ObservationFact::ContainerPresentIndeterminate
                    }
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

        // r[impl autonomous.restart.start-limit-hit]
        // sub-state `start-limit-hit` is systemd's "I gave up" signal — the
        // unit has burned through `StartLimitBurst` restarts within
        // `StartLimitIntervalSec`. It is reported alongside `failed` and is
        // distinct from a transient failure because no further automatic
        // recovery will happen.
        let unit_fact = match unit_state.as_ref() {
            None => ObservationFact::UnitGone,
            Some(s) if matches!(s.active, ActiveState::Failed) && s.sub == "start-limit-hit" => {
                ObservationFact::UnitStartLimitHit
            }
            Some(s) => match s.active {
                ActiveState::Inactive | ActiveState::Deactivating => ObservationFact::UnitInactive,
                ActiveState::Active | ActiveState::Activating => ObservationFact::UnitActive,
                ActiveState::Failed => ObservationFact::UnitFailed,
            },
        };
        facts.push((unit_fact, now));

        // r[impl autonomous.restart.record]
        // The counter is reported separately from the unit's state because it
        // is the only signal that survives a restart the observer never saw:
        // a unit that went down and came back inside one observe interval
        // looks `active` at both ends, but its counter has moved.
        if let Some(s) = unit_state.as_ref()
            && let Some(count) = s.restarts
        {
            facts.push((
                ObservationFact::UnitRestartCounter {
                    count,
                    exit: s.last_exit,
                },
                now,
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        defs::resource::ResourceKind,
        system::{
            ContainerRuntime, ProcessManager,
            stub::{StubContainerRuntime, StubDataPlane, StubNetworkProxy, StubProcessManager},
            types::{TransientRestart, TransientUnitSpec, UnitExit, UnitExitKind},
            volume_store::VolumeStore,
        },
    };
    use seedling_protocol::names::AppName;

    fn unit_spec(name: &str) -> TransientUnitSpec {
        TransientUnitSpec {
            name: name.to_owned(),
            description: String::new(),
            exec_start: vec![
                "podman".to_owned(),
                "run".to_owned(),
                "img:latest".to_owned(),
            ],
            restart: TransientRestart::Always,
            log_extra_fields: vec![],
            kill_signal: None,
            timeout_stop_secs: None,
            restart_sec: None,
            start_limit_interval_sec: None,
            start_limit_burst: None,
        }
    }

    /// A stubbed system whose process manager stays reachable, so a test can
    /// move the restart counter behind the observer's back — which is exactly
    /// what a restart completing between two ticks looks like.
    fn stubbed() -> (Arc<System>, Arc<StubProcessManager>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let volumes = dir.path().join("stub-volumes");
        std::fs::create_dir_all(&volumes).expect("volumes dir");
        let container = Arc::new(StubContainerRuntime::new(volumes));
        let process = Arc::new(StubProcessManager::new(Arc::clone(&container)));
        let system = Arc::new(System {
            container: Arc::clone(&container) as Arc<dyn ContainerRuntime>,
            process: Arc::clone(&process) as Arc<dyn ProcessManager>,
            proxy: Arc::new(StubNetworkProxy),
            data_plane: Arc::new(StubDataPlane),
            volume_store: VolumeStore::new(dir.path(), false).expect("volume store"),
            degraded: None,
        });
        (system, process, dir)
    }

    async fn observed_counter(
        observer: &Observer,
        instance: &ResourceInstance,
    ) -> Option<(u32, Option<UnitExit>)> {
        let mut facts = Vec::new();
        observer
            .observe_pod_instance(instance, SystemTime::now(), &mut facts)
            .await
            .expect("observe");
        facts.into_iter().find_map(|(f, _)| match f {
            ObservationFact::UnitRestartCounter { count, exit } => Some((count, exit)),
            _ => None,
        })
    }

    // r[verify autonomous.restart.record]
    #[tokio::test]
    async fn observes_the_restart_counter_and_last_exit() {
        let (system, process, _dir) = stubbed();
        let instance = ResourceInstance::new_singleton(
            AppName::new("myapp").unwrap(),
            ResourceKind::Deployment,
            "web",
        );
        let unit = unit_name(&instance);
        process
            .start_transient(unit_spec(&unit))
            .await
            .expect("start");

        let observer = Observer::new(Arc::clone(&system));
        assert_eq!(
            observed_counter(&observer, &instance).await,
            Some((0, None))
        );

        // The unit restarts twice and is running again by the time the next
        // observation lands; only the counter carries the evidence.
        let exit = UnitExit {
            kind: UnitExitKind::Exited,
            code: 1,
        };
        process.simulate_restarts(&unit, 2, Some(exit));

        assert_eq!(
            observed_counter(&observer, &instance).await,
            Some((2, Some(exit)))
        );
    }
}
