use std::collections::HashSet;

use std::{collections::BTreeMap, time::Duration};

use seedling_protocol::names::AppName;
use tracing::{error, warn};

use super::{AppSnapshot, Reconciler};
use crate::{
    defs::resource::ResourceKind,
    runtime::{
        AppPhase,
        apps::{AppEntry, AppRegistry},
        barrier::oracle::derive_lifecycle_state,
        gc::{KnownDesiredState, gc_unscheduled_instances},
        history::{
            delete_instance, find_instances_for_group, insert_observation, query_observations,
        },
        identity::{InstanceId, ResourceInstance},
        lifecycle::LifecycleState,
    },
};

/// Threshold after which we file a `cert_acquisition_failed` fault if a warm
/// cert hasn't been observed valid. Caddy's internal CA issues immediately;
/// public ACME issuance can take seconds to a minute. Three minutes leaves
/// generous margin for transient network issues.
// r[impl fault.cert-acquisition]
const CERT_ACQUISITION_DEADLINE: Duration = Duration::from_secs(180);

/// Input to the DB lookup performed in emit_state_changes.
#[derive(Clone)]
struct DesiredGroup {
    app: AppName,
    kind: ResourceKind,
    res_name: Option<String>,
    kind_str: String,
}

/// One group per resource, from a stream of per-instance entries.
///
/// `desired.resources` holds an entry per *instance*, so a deployment scaled
/// to N yielded N identical groups. Each group then expands to all N
/// instances at lookup time, so every state change was emitted N times —
/// N² entries for one resource, and a client driven by the event feed saw
/// the same `resource_state_changed` repeated once per replica.
fn dedupe_groups(
    raw: impl Iterator<Item = (AppName, ResourceKind, Option<String>)>,
) -> Vec<DesiredGroup> {
    let mut seen: HashSet<(AppName, ResourceKind, Option<String>)> = HashSet::new();
    let mut groups = Vec::new();
    for (app, kind, res_name) in raw {
        if !seen.insert((app.clone(), kind, res_name.clone())) {
            continue;
        }
        groups.push(DesiredGroup {
            app,
            kind,
            res_name,
            kind_str: format!("{kind:?}").to_lowercase(),
        });
    }
    groups
}

/// Result returned from the DB lookup.
#[derive(Clone)]
struct StateEntry {
    app: AppName,
    kind_str: String,
    res_name: String,
    hex: String,
    inst_id: InstanceId,
    state: LifecycleState,
}

impl Reconciler {
    pub(super) fn emit_state_changes(&mut self, apps: &[AppSnapshot]) {
        let groups = dedupe_groups(apps.iter().flat_map(|app| {
            app.desired.resources.iter().map(move |dr| {
                (
                    app.name.clone(),
                    dr.instance.kind,
                    dr.instance.name.as_deref().map(str::to_owned),
                )
            })
        }));

        let entries: Vec<StateEntry> = self.db.call(move |db| {
            let mut out = Vec::new();
            for g in &groups {
                let instances = find_instances_for_group(db, &g.app, g.kind, g.res_name.as_deref())
                    .unwrap_or_default();
                for inst in instances {
                    let hex = inst.id.to_hex();
                    let obs = query_observations(db, &inst).unwrap_or_default();
                    let state = derive_lifecycle_state(&inst, &obs);
                    out.push(StateEntry {
                        app: g.app.clone(),
                        kind_str: g.kind_str.clone(),
                        res_name: g.res_name.as_deref().unwrap_or("").to_owned(),
                        hex,
                        inst_id: inst.id,
                        state,
                    });
                }
            }
            out
        });

        let mut new_states = BTreeMap::new();

        for entry in &entries {
            let key = (entry.app.clone(), entry.hex.clone());
            if let Some(&prev) = self.prev_states.get(&key)
                && prev != entry.state
            {
                self.event_tx.resource_state_changed(
                    &entry.app,
                    &entry.kind_str,
                    &entry.res_name,
                    &entry.hex,
                    &format!("{:?}", entry.state),
                );
            }
            new_states.insert(key, entry.state);
        }

        // For desired resources that had no DB instances yet, mark Pending.
        for app in apps {
            for dr in &app.desired.resources {
                let inst_hex = dr.instance.id.to_hex();
                let key = (app.name.clone(), inst_hex.clone());
                if !entries.iter().any(|e| e.inst_id == dr.instance.id) {
                    new_states.insert(key, LifecycleState::Pending);
                }
            }
        }

        self.prev_states = new_states;
    }

    // r[impl observe.ingress.certs]
    // r[impl fault.cert-acquisition]
    pub(super) async fn observe_warm_certs(&mut self, apps: &[AppSnapshot]) {
        let targets = super::phases::warm_cert_targets(apps, &*self.registry);
        if targets.is_empty() {
            self.warm_cert_first_seen.clear();
            return;
        }

        // Lazily resolve the Caddy data volume mount path on the host.
        let path = match self
            .caddy_data_path
            .get_or_try_init(|| async {
                self.driver
                    .container
                    .volume_mountpoint(crate::system::caddy::CADDY_DATA_VOLUME)
                    .await
            })
            .await
        {
            Ok(p) => p.clone(),
            Err(e) => {
                warn!(error = %e, "warm_certs: failed to resolve Caddy data volume mount path");
                return;
            }
        };

        let now = std::time::Instant::now();
        let mut active_hostnames: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for (_, hostname) in &targets {
            active_hostnames.insert(hostname.clone());
            self.warm_cert_first_seen
                .entry(hostname.clone())
                .or_insert(now);
        }
        self.warm_cert_first_seen
            .retain(|h, _| active_hostnames.contains(h));

        let observations = crate::system::caddy::observe_certs(&path, &targets);

        let observed_hostnames: std::collections::BTreeSet<String> = observations
            .iter()
            .filter_map(|(_, _, payload)| {
                payload
                    .get("hostname")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .collect();

        for (instance, hostname) in &targets {
            if observed_hostnames.contains(hostname) {
                self.warm_cert_first_seen.remove(hostname);
                self.clear_resource_fault(instance, "cert_acquisition_failed");
            } else if let Some(first) = self.warm_cert_first_seen.get(hostname)
                && now.duration_since(*first) >= CERT_ACQUISITION_DEADLINE
            {
                self.file_resource_fault(
                    instance,
                    "cert_acquisition_failed",
                    &format!(
                        "TLS certificate for {hostname} not observed valid after {}s",
                        CERT_ACQUISITION_DEADLINE.as_secs()
                    ),
                );
            }
        }

        self.persist_obs(observations);
    }

    // r[impl gc.instances]
    /// Delete excess instances that have reached `Unscheduled` this tick so
    /// that scale-up immediately allocates fresh instances rather than reusing
    /// stale IDs.  Only runs for `Installed` apps (uninstall cleanup is handled
    /// separately by `run_uninstall_phase`).
    pub(super) fn retire_unscheduled_excess(&mut self, apps: &[AppSnapshot]) {
        for app in apps {
            if app.phase != AppPhase::Installed {
                continue;
            }
            for dr in &app.desired.resources {
                // A stopped singleton is desired Unscheduled too, but it is
                // still part of the active desired state: retiring it would
                // mint a fresh identity on the next tick, and retire that.
                if !dr.demoted {
                    continue;
                }
                let instance = dr.instance.clone();
                let (state, has_observations, has_terminal_obs) = self.db.call(move |db| {
                    let obs = query_observations(db, &instance).unwrap_or_default();
                    let has_obs = !obs.is_empty();
                    // r[impl gc.instances.never-actuated]
                    // Multiple observations recorded in a single tick may share
                    // a timestamp, in which case `query_observations` returns
                    // them in undefined order and `derive_lifecycle_state` can
                    // wedge at Pending even though terminal observations exist.
                    // Treat the presence of any terminal observation as proof
                    // that the actuator successfully tore the instance down.
                    let has_terminal = obs.iter().any(|o| {
                        matches!(
                            o.obs_kind.as_str(),
                            "container_removed" | "network_cleaned_up"
                        )
                    });
                    (
                        derive_lifecycle_state(&instance, &obs),
                        has_obs,
                        has_terminal,
                    )
                });
                let eligible = retire_eligible(state, has_observations, has_terminal_obs);
                if !eligible {
                    if matches!(
                        state,
                        LifecycleState::Terminating | LifecycleState::Terminated
                    ) {
                        self.written_obs.retain(|(id, _)| *id != dr.instance.id);
                        tracing::debug!(
                            app = %app.name,
                            instance = %dr.instance.display_name,
                            "clearing written_obs for stuck-terminating excess instance"
                        );
                    }
                    continue;
                }
                let instance_id = dr.instance.id;
                match self.db.call(move |db| delete_instance(db, instance_id)) {
                    Ok(()) => {
                        self.written_obs.retain(|(id, _)| *id != dr.instance.id);
                        tracing::debug!(
                            app = %app.name,
                            instance = %dr.instance.display_name,
                            ?state,
                            has_observations,
                            has_terminal_obs,
                            "retired excess instance"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            app = %app.name,
                            instance = %dr.instance.display_name,
                            error = %e,
                            "failed to retire excess instance"
                        );
                    }
                }
            }
        }
    }

    /// Delete instances that have sat Unscheduled past the retention period
    /// and are outside their app's active desired state, at most once per
    /// sweep interval.
    ///
    /// Only apps whose desired state this tick computed from the definition
    /// (and which are still steady now), plus apps that are not installed,
    /// contribute; every other app's instances are left alone.
    // r[impl gc.instances]
    pub(super) fn sweep_unscheduled_instances(&mut self, apps: &[AppSnapshot]) {
        let now = std::time::Instant::now();
        if self
            .last_instance_gc
            .is_some_and(|last| now.duration_since(last) < self.instance_gc.interval)
        {
            return;
        }
        self.last_instance_gc = Some(now);

        let known = known_desired_state(apps, &self.app_registry.read());
        let retain = self.instance_gc.retain;
        match self
            .db
            .call(move |db| gc_unscheduled_instances(db, retain, &known))
        {
            Ok(retired) if !retired.is_empty() => {
                self.written_obs.retain(|(id, _)| !retired.contains(id));
                tracing::debug!(
                    instances = retired.len(),
                    "gc: pruned unscheduled instances"
                );
            }
            Ok(_) => {}
            Err(e) => error!(error = %e, "gc: unscheduled instances cleanup failed"),
        }
    }

    // r[impl observe.persist]
    // r[impl history.world.source]
    // `batch` is collected from probing real podman/systemd state during the
    // current tick — every entry persisted here corresponds to a check or
    // event actually observed, never a synthesised or expected value.
    pub(super) fn persist_obs(
        &mut self,
        batch: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    ) {
        for (instance, kind, payload) in batch {
            if !self.written_obs.insert((instance.id, kind)) {
                tracing::trace!(
                    instance = %instance.display_name,
                    obs = kind,
                    "persist_obs: skipping already-written observation"
                );
                continue;
            }
            if let Err(e) = self
                .db
                .call(move |db| insert_observation(db, &instance, kind, &payload))
            {
                error!(
                    error = %e,
                    obs = kind,
                    "reconciler: failed to persist observation"
                );
            }
        }
    }
}

/// Each app's active desired state, for the apps where it is known.
///
/// A steady snapshot is rechecked against the registry because an operation
/// may have started since it was taken, and an operation resolves its own
/// scaled groups. An app not installed desires nothing. An app the registry
/// does not hold may simply have failed to load, so it gets no entry.
// r[impl gc.instances]
fn known_desired_state(apps: &[AppSnapshot], registry: &AppRegistry) -> KnownDesiredState {
    let still_steady = |entry: &AppEntry| {
        *entry.phase.lock() == AppPhase::Installed && entry.active_progress.read().is_none()
    };
    let mut known = KnownDesiredState::default();
    for app in apps.iter().filter(|app| app.steady) {
        if registry.get(app.name.as_str()).is_some_and(still_steady) {
            known.insert_app(
                &app.name,
                app.desired
                    .resources
                    .iter()
                    .filter(|dr| !dr.demoted)
                    .map(|dr| dr.instance.id),
            );
        }
    }
    for (name, _) in registry.list() {
        if registry.get(name.as_str()).is_some_and(|entry| {
            *entry.phase.lock() == AppPhase::NotInstalled && entry.active_progress.read().is_none()
        }) {
            known.insert_app(&name, []);
        }
    }
    known
}

// r[impl gc.instances.never-actuated]
fn retire_eligible(state: LifecycleState, has_observations: bool, has_terminal_obs: bool) -> bool {
    matches!(state, LifecycleState::Unscheduled) || !has_observations || has_terminal_obs
}

#[cfg(test)]
mod tests {

    use seedling_protocol::names::AppName;

    use super::dedupe_groups;
    use crate::defs::resource::ResourceKind;

    fn app(s: &str) -> AppName {
        AppName::new(s).expect("app name")
    }

    // i[verify event.types]
    // `desired.resources` has one entry per instance, so a deployment scaled
    // to 3 produced 3 identical groups; each then expanded to all 3
    // instances, so one state change was reported three times.
    #[test]
    fn instances_of_one_resource_collapse_to_a_single_group() {
        let raw = vec![
            (app("web"), ResourceKind::Deployment, Some("api".to_owned())),
            (app("web"), ResourceKind::Deployment, Some("api".to_owned())),
            (app("web"), ResourceKind::Deployment, Some("api".to_owned())),
        ];
        let groups = dedupe_groups(raw.into_iter());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].res_name.as_deref(), Some("api"));
        assert_eq!(groups[0].kind_str, "deployment");
    }

    // i[verify event.types]
    #[test]
    fn distinct_resources_stay_distinct() {
        let raw = vec![
            (app("web"), ResourceKind::Deployment, Some("api".to_owned())),
            (
                app("web"),
                ResourceKind::Deployment,
                Some("worker".to_owned()),
            ),
            (app("web"), ResourceKind::Service, Some("api".to_owned())),
            (
                app("other"),
                ResourceKind::Deployment,
                Some("api".to_owned()),
            ),
            (app("web"), ResourceKind::Volume, None),
        ];
        assert_eq!(dedupe_groups(raw.into_iter()).len(), 5);
    }
    use super::*;

    // r[verify gc.instances.never-actuated]
    #[test]
    fn unscheduled_state_is_eligible() {
        assert!(retire_eligible(LifecycleState::Unscheduled, true, false));
    }

    // r[verify gc.instances.never-actuated]
    #[test]
    fn no_observations_is_eligible() {
        assert!(retire_eligible(LifecycleState::Pending, false, false));
    }

    // r[verify gc.instances.never-actuated]
    #[test]
    fn pending_with_terminal_observation_is_eligible() {
        // The observation-derivation can wedge at Pending when same-tick
        // observations have ambiguous ordering. A terminal observation is the
        // ground-truth signal that actuation completed.
        assert!(retire_eligible(LifecycleState::Pending, true, true));
    }

    #[test]
    fn pending_with_only_partial_observations_is_not_eligible() {
        // Instance still actuating: observations exist but none terminal yet.
        assert!(!retire_eligible(LifecycleState::Pending, true, false));
    }

    #[test]
    fn running_is_not_eligible() {
        // An excess instance currently observed running needs its actuator
        // pass first; retiring before stop would orphan the unit.
        assert!(!retire_eligible(LifecycleState::Running, true, false));
    }

    #[test]
    fn ready_is_not_eligible() {
        assert!(!retire_eligible(LifecycleState::Ready, true, false));
    }

    fn register(reg: &mut AppRegistry, name: &str, phase: AppPhase) {
        reg.register(
            app(name),
            std::sync::Arc::new(crate::runtime::definition::Bundle::from_stored_script("")),
            crate::runtime::definition::Source::unknown_push(),
            std::sync::Arc::new(tokio::sync::Notify::new()),
            &crate::ScriptLimits::default(),
        )
        .unwrap();
        *reg.get(name).unwrap().phase.lock() = phase;
    }

    fn steady_snapshot(
        name: &str,
        members: &[&ResourceInstance],
        excess: &[&ResourceInstance],
    ) -> AppSnapshot {
        let volume = crate::defs::resource::Resource::Volume(crate::defs::volume::Volume::new(
            Some(std::sync::Arc::new("data".to_owned())),
        ));
        let entry = |instance: &ResourceInstance, demoted: bool| crate::runtime::DesiredResource {
            instance: instance.clone(),
            desired: if demoted {
                LifecycleState::Unscheduled
            } else {
                LifecycleState::Ready
            },
            definition: volume.clone(),
            demoted,
        };
        AppSnapshot {
            name: app(name),
            desired: crate::runtime::DesiredState {
                resources: members
                    .iter()
                    .map(|i| entry(i, false))
                    .chain(excess.iter().map(|i| entry(i, true)))
                    .collect(),
            },
            steady: true,
            app_def: Default::default(),
            phase: AppPhase::Installed,
            phase_handle: std::sync::Arc::new(parking_lot::Mutex::new(AppPhase::Installed)),
            warm_cert_hostnames: Default::default(),
            current_generation: 1,
            app_priority: Default::default(),
        }
    }

    // r[verify gc.instances]
    #[test]
    fn only_apps_with_a_known_desired_state_are_swept() {
        let scaled = || ResourceInstance::new_scaled(app("demo"), ResourceKind::Deployment, "web");
        let (member, excess, unrelated) = (scaled(), scaled(), scaled());

        let mut reg = AppRegistry::new();
        register(&mut reg, "steady", AppPhase::Installed);
        // Its snapshot was steady, but an operation has started since.
        register(&mut reg, "busy", AppPhase::Installed);
        *reg.get("busy").unwrap().active_progress.write() = Some(Default::default());
        register(&mut reg, "installing", AppPhase::Installing);
        register(&mut reg, "absent", AppPhase::NotInstalled);

        let apps = [
            steady_snapshot("steady", &[&member], &[&excess]),
            steady_snapshot("busy", &[&member], &[]),
            // Snapshotted, but no longer held by the registry.
            steady_snapshot("unloaded", &[&member], &[]),
        ];
        let known = known_desired_state(&apps, &reg);

        assert!(!known.sweepable("steady", member.id));
        assert!(known.sweepable("steady", excess.id));
        assert!(known.sweepable("steady", unrelated.id));
        assert!(!known.sweepable("busy", unrelated.id));
        assert!(!known.sweepable("installing", unrelated.id));
        assert!(!known.sweepable("unloaded", unrelated.id));
        assert!(known.sweepable("absent", unrelated.id));
    }
}
