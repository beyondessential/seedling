use std::{collections::HashMap, collections::HashSet, sync::Arc};

use futures_util::future::join_all;
use ipnet::Ipv6Net;
use serde_json::json;
use tracing::{debug, error};

use crate::{
    defs::{enums::OnUpdate, resource::Resource},
    runtime::{
        desired::{DesiredResource, DesiredState},
        identity::{InstanceId, ResourceInstance},
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
    /// Instances whose registry lookup failed during start.
    pub registry_failures: Vec<ResourceInstance>,
    /// Instances blocked from starting because a required external volume has no mapping.
    pub external_volume_failures: Vec<(ResourceInstance, String)>,
    /// Instances whose start failed for reasons other than image pull or registry.
    pub start_failures: Vec<(ResourceInstance, String)>,
    /// Instances whose stop failed.
    pub stop_failures: Vec<(ResourceInstance, String)>,
    /// Instances whose observation failed.
    pub observe_failures: Vec<(ResourceInstance, String)>,
    /// Deployment names with an active rolling update (stale instances still
    /// being drained). The reconciler uses this to bump effective scale +1.
    pub rolling_deployments: HashSet<String>,
    /// Instances successfully started this tick; their written_obs entries must
    /// be cleared so the new lifecycle observations can be recorded.
    pub started_instances: Vec<ResourceInstance>,
    // r[impl autonomous.job-terminal.defense]
    /// Job instances detected as complete this tick, for the reconciler's
    /// completed-jobs set.
    pub completed_job_instances: Vec<InstanceId>,
}

struct PodInstanceResult {
    running: Option<RunningPod>,
    observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    image_pull_failure: Option<(ResourceInstance, String)>,
    image_pull_success: Option<(ResourceInstance, String)>,
    unit_failure: Option<ResourceInstance>,
    unit_healthy: Option<ResourceInstance>,
    registry_failure: Option<ResourceInstance>,
    external_volume_failure: Option<(ResourceInstance, String)>,
    start_failure: Option<(ResourceInstance, String)>,
    stop_failure: Option<(ResourceInstance, String)>,
    observe_failure: Option<(ResourceInstance, String)>,
    started_instance: Option<ResourceInstance>,
    // r[impl autonomous.job-terminal.defense]
    completed_job: Option<InstanceId>,
}

/// Per-instance observation collected before any actuation decisions are made.
struct ObservedInstance<'a> {
    dr: &'a DesiredResource,
    is_running: bool,
    spec_stale: bool,
    unit_failed: bool,
    unit_active: bool,
    container_exists: bool,
    /// Container has exited but not yet been removed (ContainerExited fact present).
    has_exited: bool,
    result: PodInstanceResult,
}

// r[observe.deployment]
async fn observe_one_pod<'a>(
    observer: &Observer,
    actuator: &Actuator,
    driver: &Arc<System>,
    dr: &'a DesiredResource,
    node_prefix: &Ipv6Net,
) -> Option<ObservedInstance<'a>> {
    let mut result = PodInstanceResult {
        running: None,
        observations: Vec::new(),
        image_pull_failure: None,
        image_pull_success: None,
        unit_failure: None,
        unit_healthy: None,
        registry_failure: None,
        external_volume_failure: None,
        start_failure: None,
        stop_failure: None,
        observe_failure: None,
        started_instance: None,
        completed_job: None,
    };

    let facts = match observer.observe(&dr.instance, &dr.definition).await {
        Ok(f) => f,
        Err(e) => {
            error!(
                instance = %dr.instance.display_name,
                error = %e,
                "pods: observe failed, skipping instance"
            );
            result.observe_failure = Some((dr.instance.clone(), e.to_string()));
            return Some(ObservedInstance {
                dr,
                is_running: false,
                spec_stale: false,
                unit_failed: false,
                unit_active: false,
                container_exists: false,
                has_exited: false,
                result,
            });
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

    // r[update.spec-hash]
    let observed_spec_hash = facts.iter().find_map(|(f, _)| {
        if let ObservationFact::ContainerSpecHash(h) = f {
            Some(h.clone())
        } else {
            None
        }
    });

    let desired_spec_hash = if is_running {
        actuator.desired_spec_hash(&dr.instance, &dr.definition)
    } else {
        None
    };

    // The spec is stale when the desired hash is known and either differs
    // from the observed hash or the observed hash is absent (container
    // predates the spec-hash label).
    let spec_stale = match (observed_spec_hash, desired_spec_hash) {
        (Some(observed), Some(desired)) => observed != desired,
        (None, Some(_)) => true,
        _ => false,
    };

    let unit_failed = facts
        .iter()
        .any(|(f, _)| matches!(f, ObservationFact::UnitFailed));
    let unit_active = facts
        .iter()
        .any(|(f, _)| matches!(f, ObservationFact::UnitActive));
    let container_exists = facts.iter().any(|(f, _)| {
        matches!(
            f,
            ObservationFact::ContainerCreated
                | ObservationFact::ContainerRunning { .. }
                | ObservationFact::ContainerExited { .. }
        )
    });
    let has_exited = facts
        .iter()
        .any(|(f, _)| matches!(f, ObservationFact::ContainerExited { .. }));

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

    Some(ObservedInstance {
        dr,
        is_running,
        spec_stale,
        unit_failed,
        unit_active,
        container_exists,
        has_exited,
        result,
    })
}

// r[actuate.deployment.start]
// r[actuate.deployment.stop]
// r[fault.non-blocking]
// r[fault.container-start]
// r[impl autonomous.job-terminal]
async fn actuate_one_pod(
    actuator: &Actuator,
    mut obs: ObservedInstance<'_>,
    inhibit_stop: bool,
    written_obs: &HashSet<(InstanceId, &'static str)>,
    completed_jobs: &HashSet<InstanceId>,
) -> Option<PodInstanceResult> {
    let dr = obs.dr;
    let result = &mut obs.result;

    // r[impl autonomous.job-terminal]
    // r[impl autonomous.job-terminal.defense]
    // Jobs that have completed naturally are not restarted. Two detection paths:
    // 1. Primary: container was previously seen running (written_obs) but is now
    //    gone — works even when Podman's --rm removes it instantly on exit.
    // 2. Defense in depth: if this instance ID was recorded as completed in a
    //    prior tick (completed_jobs), kill any container that somehow restarted.
    if matches!(dr.definition, Resource::Job(_)) && dr.desired == LifecycleState::Ready {
        let previously_ran = written_obs.contains(&(dr.instance.id, "container_running"));
        let already_completed = completed_jobs.contains(&dr.instance.id);
        let is_done = obs.has_exited
            || (!obs.container_exists && !obs.is_running && previously_ran)
            || already_completed;
        if is_done {
            // Always call stop: even when the container is already gone (--rm)
            // and the unit is merely Inactive, the pod network must be removed.
            // stop_pod_instance tolerates missing containers; the network
            // removal is the critical side-effect here.
            match actuator.stop(&dr.instance, &dr.definition).await {
                Ok(()) => {}
                Err(e) => {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "pods: stop completed job failed"
                    );
                    result.stop_failure = Some((dr.instance.clone(), e.to_string()));
                }
            }
            result.completed_job = Some(dr.instance.id);
            return Some(obs.result);
        }
    }

    // Track unit health for fault filing/clearing.
    // A unit that is "active" in systemd but whose container is not
    // running (e.g. exited inside a restarting unit) is not healthy —
    // it is stuck in a crash loop managed by systemd's restart logic.
    if dr.desired == LifecycleState::Ready {
        if obs.is_running && (!obs.spec_stale || inhibit_stop) {
            // Running and either current-spec or kept alive by rolling strategy.
            result.unit_healthy = Some(dr.instance.clone());
        } else if obs.unit_failed || (obs.unit_active && !obs.is_running) {
            result.unit_failure = Some(dr.instance.clone());
        }
    }

    match dr.desired {
        LifecycleState::Ready if !obs.is_running || obs.spec_stale => {
            if obs.spec_stale && inhibit_stop {
                // Rolling strategy decided to keep this stale instance alive.
                return Some(obs.result);
            }

            // r[fault.container-start]
            // If the unit is in a broken state (failed, or active but the
            // container is not running), or the running container has a
            // stale spec, tear it down so the next tick can start a fresh
            // unit with the current AppDef config.
            if obs.unit_failed || obs.unit_active || obs.spec_stale {
                result.running = None;
                match actuator.stop(&dr.instance, &dr.definition).await {
                    Ok(()) => {}
                    Err(e) => {
                        error!(
                            instance = %dr.instance.display_name,
                            error = %e,
                            "pods: stop broken unit failed"
                        );
                        result.stop_failure = Some((dr.instance.clone(), e.to_string()));
                    }
                }
                return Some(obs.result);
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
                    result.started_instance = Some(dr.instance.clone());
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
                Err(ref e @ crate::system::actuator::ActuateError::Registry { .. }) => {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "pods: registry lookup failed during start"
                    );
                    result.registry_failure = Some(dr.instance.clone());
                }
                // r[impl fault.external-volume-unmapped]
                Err(crate::system::actuator::ActuateError::ExternalVolumeNotMapped {
                    ref name,
                    ..
                }) => {
                    error!(
                        instance = %dr.instance.display_name,
                        volume = %name,
                        "pods: external volume not mapped"
                    );
                    result.external_volume_failure = Some((dr.instance.clone(), name.clone()));
                }
                Err(e) => {
                    error!(
                        instance = %dr.instance.display_name,
                        error = %e,
                        "pods: start failed"
                    );
                    result.start_failure = Some((dr.instance.clone(), e.to_string()));
                }
            }
        }
        LifecycleState::Unscheduled if obs.is_running || obs.container_exists => {
            result.running = None;
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
                    result.stop_failure = Some((dr.instance.clone(), e.to_string()));
                }
            }
        }
        _ => {}
    }

    Some(obs.result)
}

/// Determine which stale instances should have their stop inhibited based on
/// the deployment's update strategy.
///
/// Returns a set of instance display names that must NOT be stopped this tick,
/// and whether a rolling update is still active for the deployment.
fn compute_stop_inhibitions(
    deployment_name: &str,
    on_update: OnUpdate,
    instances: &[&ObservedInstance<'_>],
) -> (HashSet<String>, bool) {
    let stale_running: Vec<&str> = instances
        .iter()
        .filter(|o| o.spec_stale && o.is_running)
        .map(|o| o.dr.instance.display_name.as_str())
        .collect();

    if stale_running.is_empty() {
        return (HashSet::new(), false);
    }

    match on_update {
        // r[update.replace]
        OnUpdate::Replace => {
            debug!(
                deployment = deployment_name,
                stale_count = stale_running.len(),
                "replace: stopping all stale instances"
            );
            (HashSet::new(), false)
        }
        // r[update.rolling]
        OnUpdate::Rolling => {
            let current_ready: Vec<&ObservedInstance<'_>> = instances
                .iter()
                .filter(|o| !o.spec_stale && o.dr.desired == LifecycleState::Ready)
                .copied()
                .collect();

            let current_running = current_ready.iter().filter(|o| o.is_running).count();
            let current_pending = current_ready.len() - current_running;

            if current_running == 0 {
                // No current-hash instance is running yet. Keep all stale
                // instances alive (they're serving traffic). Signal that a
                // rolling update is active so the reconciler bumps scale.
                debug!(
                    deployment = deployment_name,
                    stale_count = stale_running.len(),
                    current_pending,
                    "rolling: no current-hash instance healthy, inhibiting all stale stops"
                );
                let inhibited: HashSet<String> =
                    stale_running.iter().map(|s| (*s).to_owned()).collect();
                (inhibited, true)
            } else if current_pending > 0 {
                // At least one current-hash instance is healthy, but another
                // is still starting up (a previous replacement). Wait for it
                // to be confirmed running before retiring more stale instances.
                debug!(
                    deployment = deployment_name,
                    current_running,
                    current_pending,
                    stale_count = stale_running.len(),
                    "rolling: replacement still starting, inhibiting all stale stops"
                );
                let inhibited: HashSet<String> =
                    stale_running.iter().map(|s| (*s).to_owned()).collect();
                (inhibited, true)
            } else {
                // All current-hash instances are running. Safe to retire
                // exactly one stale instance; inhibit the rest.
                let mut inhibited: HashSet<String> =
                    stale_running.iter().map(|s| (*s).to_owned()).collect();
                if let Some(victim) = stale_running.first() {
                    debug!(
                        deployment = deployment_name,
                        victim = *victim,
                        remaining_stale = stale_running.len() - 1,
                        "rolling: stopping one stale instance"
                    );
                    inhibited.remove(*victim);
                }
                let still_active = inhibited.iter().any(|name| {
                    instances
                        .iter()
                        .any(|o| o.dr.instance.display_name == *name && o.spec_stale)
                });
                (inhibited, still_active)
            }
        }
    }
}

// r[impl reconciliation.liveness]
// r[impl update.rolling.restart-resume]
// r[impl update.rolling.reboot-resume]
pub(super) async fn observe_and_actuate(
    observer: &Observer,
    actuator: &Actuator,
    driver: &Arc<System>,
    desired: &DesiredState,
    node_prefix: &Ipv6Net,
    written_obs: &HashSet<(InstanceId, &'static str)>,
    completed_jobs: &HashSet<InstanceId>,
) -> PodActuationUpdate {
    // Phase 1: observe all instances concurrently.
    let pod_resources: Vec<&DesiredResource> = desired
        .resources
        .iter()
        .filter(|dr| matches!(&dr.definition, Resource::Deployment(_) | Resource::Job(_)))
        .collect();

    let observe_futures: Vec<_> = pod_resources
        .iter()
        .map(|dr| observe_one_pod(observer, actuator, driver, dr, node_prefix))
        .collect();

    let observed: Vec<ObservedInstance<'_>> = join_all(observe_futures)
        .await
        .into_iter()
        .flatten()
        .collect();

    // Phase 2: group deployments and compute stop inhibitions.
    let mut deployment_groups: HashMap<String, Vec<usize>> = HashMap::new();
    let mut job_indices: Vec<usize> = Vec::new();

    for (idx, obs) in observed.iter().enumerate() {
        match &obs.dr.definition {
            // r[update.jobs]
            Resource::Job(_) => job_indices.push(idx),
            Resource::Deployment(_) => {
                let dep_name = obs
                    .dr
                    .instance
                    .name
                    .clone()
                    .unwrap_or_else(|| obs.dr.instance.display_name.clone());
                deployment_groups.entry(dep_name).or_default().push(idx);
            }
            _ => {}
        }
    }

    let mut inhibited_instances: HashSet<String> = HashSet::new();
    let mut rolling_deployments: HashSet<String> = HashSet::new();

    for (dep_name, indices) in &deployment_groups {
        let group_refs: Vec<&ObservedInstance<'_>> =
            indices.iter().map(|&i| &observed[i]).collect();

        let on_update = match &group_refs[0].dr.definition {
            Resource::Deployment(dep) => dep.def.lock().on_update,
            _ => OnUpdate::Replace,
        };

        let (inhibited, rolling_active) =
            compute_stop_inhibitions(dep_name, on_update, &group_refs);

        inhibited_instances.extend(inhibited);
        if rolling_active {
            rolling_deployments.insert(dep_name.clone());
        }
    }

    // Phase 3: actuate all instances, passing stop decisions.
    let mut actuate_futures = Vec::with_capacity(observed.len());
    for obs in observed {
        let inhibit = inhibited_instances.contains(&obs.dr.instance.display_name);
        actuate_futures.push(actuate_one_pod(
            actuator,
            obs,
            inhibit,
            written_obs,
            completed_jobs,
        ));
    }

    let results = join_all(actuate_futures).await;

    let mut update = PodActuationUpdate {
        running: Vec::new(),
        observations: Vec::new(),
        image_pull_failures: Vec::new(),
        image_pull_successes: Vec::new(),
        unit_failures: Vec::new(),
        unit_healthy: Vec::new(),
        registry_failures: Vec::new(),
        external_volume_failures: Vec::new(),
        start_failures: Vec::new(),
        stop_failures: Vec::new(),
        observe_failures: Vec::new(),
        rolling_deployments,
        started_instances: Vec::new(),
        completed_job_instances: Vec::new(),
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
        if let Some(f) = result.registry_failure {
            update.registry_failures.push(f);
        }
        if let Some(f) = result.external_volume_failure {
            update.external_volume_failures.push(f);
        }
        if let Some(f) = result.start_failure {
            update.start_failures.push(f);
        }
        if let Some(f) = result.stop_failure {
            update.stop_failures.push(f);
        }
        if let Some(f) = result.observe_failure {
            update.observe_failures.push(f);
        }
        if let Some(s) = result.started_instance {
            update.started_instances.push(s);
        }
        // r[impl autonomous.job-terminal.defense]
        if let Some(id) = result.completed_job {
            update.completed_job_instances.push(id);
        }
    }

    update
}
