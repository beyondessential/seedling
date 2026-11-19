use std::collections::HashSet;

use std::{collections::BTreeMap, time::Duration};

use seedling_protocol::names::AppName;
use tracing::{error, warn};

use super::{AppSnapshot, Reconciler, faults::AcquisitionError};
use crate::{
    defs::resource::ResourceKind,
    runtime::{
        AppPhase,
        apps::{AppEntry, AppRegistry},
        barrier::oracle::derive_lifecycle_state,
        gc::{KnownDesiredState, gc_unscheduled_instances, is_torn_down},
        health::{InstanceHealth, declares_healthcheck, read_health},
        history::{
            delete_instance, find_instances_for_group, insert_observation, query_observations,
            read_lifecycle,
        },
        identity::{InstanceId, ResourceInstance},
        lifecycle::LifecycleState,
        tls::env::{RuntimeHoldings, latest_acquisition_errors, runtime_holdings},
    },
};

/// Threshold after which we file a `cert_acquisition_failed` fault if a warm
/// cert hasn't been observed valid. Caddy's internal CA issues immediately;
/// public ACME issuance can take seconds to a minute. Three minutes leaves
/// generous margin for transient network issues.
// spec: FLT#certificate-faults
const CERT_ACQUISITION_DEADLINE: Duration = Duration::from_secs(180);

/// What the observation of one ingress's certificates found.
struct IngressCerts {
    instance: ResourceInstance,
    /// A certificate was observed for every hostname.
    all_observed: bool,
    /// Hostnames with no certificate observed past the deadline, with no
    /// acquisition in progress.
    failing: Vec<String>,
}

/// Input to the DB lookup performed in emit_state_changes.
#[derive(Clone)]
struct DesiredGroup {
    app: AppName,
    kind: ResourceKind,
    res_name: Option<String>,
    kind_str: String,
    declares_healthcheck: bool,
}

/// The resource groups whose instances could not be listed.
type UnreadGroups = HashSet<(AppName, ResourceKind, Option<String>)>;

/// One group per resource, from a stream of per-instance entries.
///
/// `desired.resources` holds an entry per *instance*, so a deployment scaled
/// to N yielded N identical groups. Each group then expands to all N
/// instances at lookup time, so every state change was emitted N times —
/// N² entries for one resource, and a client driven by the event feed saw
/// the same `resource_state_changed` repeated once per replica.
fn dedupe_groups(
    raw: impl Iterator<Item = (AppName, ResourceKind, Option<String>, bool)>,
) -> Vec<DesiredGroup> {
    let mut seen: HashSet<(AppName, ResourceKind, Option<String>)> = HashSet::new();
    let mut groups = Vec::new();
    for (app, kind, res_name, declares_healthcheck) in raw {
        if !seen.insert((app.clone(), kind, res_name.clone())) {
            continue;
        }
        groups.push(DesiredGroup {
            app,
            kind,
            res_name,
            kind_str: kind.type_name().to_owned(),
            declares_healthcheck,
        });
    }
    groups
}

/// Result returned from the DB lookup.
#[derive(Clone)]
struct StateEntry {
    app: AppName,
    kind_str: String,
    res_name: Option<String>,
    hex: String,
    inst_id: InstanceId,
    /// `None` while the instance's observation history cannot be read: its
    /// state is undetermined, not Pending.
    // spec: LCY#derivation
    state: Option<LifecycleState>,
    /// `None` while the observations the health derives from cannot be read:
    /// the health is undetermined, not absent.
    // spec: APPS#resources
    health: Option<Option<InstanceHealth>>,
}

impl Reconciler {
    pub(super) fn emit_state_changes(&mut self, apps: &[AppSnapshot]) {
        let groups = dedupe_groups(apps.iter().flat_map(|app| {
            let declared = app.desired.resources.iter().map(move |dr| {
                (
                    app.name.clone(),
                    dr.instance.kind,
                    dr.instance.name.as_deref().map(str::to_owned),
                    declares_healthcheck(&dr.definition),
                )
            });
            // An undeclared instance declares no healthcheck.
            let undeclared = app.desired.undeclared.iter().map(move |instance| {
                (
                    app.name.clone(),
                    instance.kind,
                    instance.name.as_deref().map(str::to_owned),
                    false,
                )
            });
            declared.chain(undeclared)
        }));

        let (entries, unread_groups): (Vec<StateEntry>, UnreadGroups) =
            self.db.call(move |db| {
                let mut out = Vec::new();
                let mut unread = HashSet::new();
                for g in &groups {
                    let instances =
                        match find_instances_for_group(db, &g.app, g.kind, g.res_name.as_deref()) {
                            Ok(instances) => instances,
                            Err(e) => {
                                warn!(app = %g.app, error = %e, "state changes: could not list a resource's instances");
                                unread.insert((g.app.clone(), g.kind, g.res_name.clone()));
                                continue;
                            }
                        };
                    for inst in instances {
                        let hex = inst.id.to_hex();
                        let state = match read_lifecycle(db, &inst) {
                            Ok((state, _)) => Some(state),
                            Err(e) => {
                                warn!(app = %g.app, instance = %inst.display_name, error = %e, "state changes: could not read what its lifecycle state derives from");
                                None
                            }
                        };
                        let health = match read_health(db, &inst, g.declares_healthcheck) {
                            Ok(health) => Some(health),
                            Err(e) => {
                                warn!(app = %g.app, instance = %inst.display_name, error = %e, "state changes: could not read what its health derives from");
                                None
                            }
                        };
                        out.push(StateEntry {
                            app: g.app.clone(),
                            kind_str: g.kind_str.clone(),
                            res_name: g.res_name.clone(),
                            hex,
                            inst_id: inst.id,
                            state,
                            health,
                        });
                    }
                }
                (out, unread)
            });

        let mut new_states = BTreeMap::new();

        for entry in &entries {
            let key = (entry.app.clone(), entry.hex.clone());
            let Some(state) = entry.state else {
                // Undetermined: no event, and the last known state stands.
                if let Some(&prev) = self.prev_states.get(&key) {
                    new_states.insert(key, prev);
                }
                continue;
            };
            if let Some(&prev) = self.prev_states.get(&key)
                && prev != state
            {
                self.event_tx.resource_state_changed(
                    &entry.app,
                    &entry.kind_str,
                    entry.res_name.as_deref(),
                    &entry.hex,
                    &format!("{state:?}"),
                );
            }
            new_states.insert(key, state);
        }

        // spec: APPS#resources
        let mut new_health = BTreeMap::new();
        for entry in &entries {
            let key = (entry.app.clone(), entry.hex.clone());
            let Some(health) = entry.health else {
                if let Some(&prev) = self.prev_health.get(&key) {
                    new_health.insert(key, prev);
                }
                continue;
            };
            if let Some(&prev) = self.prev_health.get(&key)
                && prev != health
            {
                self.event_tx.instance_health_changed(
                    &entry.app,
                    &entry.kind_str,
                    entry.res_name.as_deref(),
                    &entry.hex,
                    health.map(InstanceHealth::name),
                );
            }
            new_health.insert(key, health);
        }

        // For desired resources that had no DB instances yet, mark Pending.
        for app in apps {
            for dr in &app.desired.resources {
                let inst_hex = dr.instance.id.to_hex();
                let key = (app.name.clone(), inst_hex.clone());
                if entries.iter().any(|e| e.inst_id == dr.instance.id) {
                    continue;
                }
                let group = (
                    app.name.clone(),
                    dr.instance.kind,
                    dr.instance.name.as_deref().map(str::to_owned),
                );
                if unread_groups.contains(&group) {
                    if let Some(&prev) = self.prev_states.get(&key) {
                        new_states.insert(key.clone(), prev);
                    }
                    if let Some(&prev) = self.prev_health.get(&key) {
                        new_health.insert(key, prev);
                    }
                } else {
                    new_states.insert(key.clone(), LifecycleState::Pending);
                    new_health.insert(key, None);
                }
            }
        }

        self.prev_states = new_states;
        self.prev_health = new_health;
    }

    // spec: OBS#ingresses
    // spec: FLT#certificate-faults
    // spec: TCAN#serving-and-observation
    pub(super) async fn observe_warm_certs(&mut self, apps: &[AppSnapshot]) {
        let targets = super::phases::warm_cert_targets(apps, &*self.registry);
        if targets.is_empty() {
            self.warm_cert_first_seen.clear();
            return;
        }

        // Lazily resolve the Caddy data volume mount path on the host. Without
        // it the proxy's own certificates cannot be seen, but a certificate the
        // runtime holds can still be, so this is not a reason to see nothing.
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
            Ok(p) => Some(p.clone()),
            Err(e) => {
                warn!(error = %e, "warm_certs: failed to resolve Caddy data volume mount path");
                None
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

        let (proxy_valid, proxy_not_valid, _undetermined) = match &path {
            Some(path) => {
                let seen =
                    crate::system::caddy::observe_certs(path, &targets, jiff::Timestamp::now());
                let valid: std::collections::BTreeSet<String> = seen
                    .observations
                    .iter()
                    .filter_map(|(_, _, payload)| {
                        payload
                            .get("hostname")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .collect();
                (valid, seen.not_valid, seen.undetermined)
            }
            None => Default::default(),
        };

        // A certificate the runtime holds is served to the proxy by the daemon
        // and is never written to the proxy's own cache, so the cache cannot
        // say whether the hostname has one. Ask the store the proxy is served
        // from, which also applies the validity window the endpoint does.
        let hostnames: Vec<String> = active_hostnames.iter().cloned().collect();
        let leaf_facts = self.leaf_facts.clone();
        let RuntimeHoldings { served, .. } = self
            .db
            .call(move |db| runtime_holdings(db, &hostnames, &leaf_facts));
        let runtime_known = served.is_some();
        let served = served.unwrap_or_default();

        // Whether a usable certificate is served for each hostname, where that
        // can be told: served by the runtime, or held by the proxy inside its
        // validity window, or definitely neither. A store or a cache that could
        // not be read leaves the hostname out, since unknown is not unserved.
        let mut observations: Vec<(ResourceInstance, &'static str, serde_json::Value)> = Vec::new();
        let mut usable: BTreeMap<&str, bool> = BTreeMap::new();
        for (instance, hostname) in &targets {
            let payload = serde_json::json!({ "hostname": hostname });
            if served.contains(hostname) || proxy_valid.contains(hostname) {
                usable.insert(hostname, true);
                observations.push((instance.clone(), "cert_valid", payload));
            } else if runtime_known && proxy_not_valid.contains(hostname) {
                usable.insert(hostname, false);
                observations.push((instance.clone(), "cert_not_valid", payload));
            }
        }

        // The status, and whether an acquisition is in progress, come from the
        // one decision the runtime makes for the hostname.
        let facts = self
            .tls_coordinator
            .as_ref()
            .and_then(|coordinator| coordinator.hostname_facts());
        if let Some(facts) = &facts {
            for (instance, hostname) in &targets {
                if let Some(status) = facts.get(hostname).and_then(|f| f.status) {
                    observations.push((
                        instance.clone(),
                        status.observation_kind(),
                        serde_json::json!({ "hostname": hostname, "status": status.as_str() }),
                    ));
                }
            }
        }
        // An ingress fails when any of its hostnames does, and recovers only
        // once a certificate is observed for every one of them.
        let mut ingresses: BTreeMap<String, IngressCerts> = BTreeMap::new();
        for (instance, hostname) in &targets {
            let entry = ingresses
                .entry(instance.id.to_hex())
                .or_insert_with(|| IngressCerts {
                    instance: instance.clone(),
                    all_observed: true,
                    failing: Vec::new(),
                });
            match usable.get(hostname.as_str()) {
                Some(true) => {
                    self.warm_cert_first_seen.remove(hostname);
                    continue;
                }
                Some(false) => entry.all_observed = false,
                // Whether a usable certificate is served could not be read, so
                // the hostname is neither failing nor served: faulting one that
                // is being served perfectly well would report a transient read
                // failure as the hostname's problem.
                None => {
                    entry.all_observed = false;
                    continue;
                }
            }
            // The deadline is for an acquisition that is not happening: an
            // issuance running, an order open or one due is in progress, and
            // so is the proxy's own ordering. Where that cannot be read it is
            // not known to be absent either.
            let in_progress = facts
                .as_ref()
                .and_then(|facts| facts.get(hostname))
                .and_then(|f| f.in_progress);
            if in_progress == Some(false)
                && let Some(first) = self.warm_cert_first_seen.get(hostname)
                && now.duration_since(*first) >= CERT_ACQUISITION_DEADLINE
            {
                entry.failing.push(hostname.clone());
            }
        }

        let failing_hostnames: Vec<String> = ingresses
            .values()
            .flat_map(|i| i.failing.iter().cloned())
            .collect();
        let errors = if failing_hostnames.is_empty() {
            Ok(BTreeMap::new())
        } else {
            let hostnames = failing_hostnames.clone();
            self.db
                .call(move |db| latest_acquisition_errors(db, &hostnames))
        };
        if let Err(e) = &errors {
            warn!(error = %e, "warm_certs: cannot read the latest acquisition errors");
        }

        for ingress in ingresses.values() {
            if ingress.all_observed {
                self.clear_resource_fault(&ingress.instance, "cert_acquisition_failed");
            } else if !ingress.failing.is_empty() {
                let failing: Vec<(String, AcquisitionError)> = ingress
                    .failing
                    .iter()
                    .map(|hostname| {
                        let error = match &errors {
                            Ok(errors) => match errors.get(hostname) {
                                Some(error) => AcquisitionError::Recorded(error.clone()),
                                None => AcquisitionError::None,
                            },
                            Err(_) => AcquisitionError::Unreadable,
                        };
                        (hostname.clone(), error)
                    })
                    .collect();
                self.file_cert_acquisition_fault(&ingress.instance, &failing);
            }
        }

        self.persist_obs(observations);
    }

    // spec: GC#unscheduled-instances
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
                let read = self.db.call(move |db| {
                    let obs = query_observations(db, &instance)?;
                    let has_obs = !obs.is_empty();
                    let state = derive_lifecycle_state(&instance, &obs);
                    Ok::<_, rusqlite::Error>((state, has_obs, is_torn_down(state, &obs)))
                });
                // A history that cannot be read is not an empty one: the
                // instance stays until it can be.
                let (state, has_observations, torn_down) = match read {
                    Ok(read) => read,
                    Err(e) => {
                        tracing::warn!(
                            app = %app.name,
                            instance = %dr.instance.display_name,
                            error = %e,
                            "could not read an excess instance's observation history; not retiring it"
                        );
                        continue;
                    }
                };
                let eligible = retire_eligible(torn_down, has_observations);
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
                            torn_down,
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
    // spec: GC#unscheduled-instances
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

    // spec: OBS#recording
    // The observation kinds that answer one question about an instance and are
    // never observed together, so that recording one of them makes the others
    // news again: a health result that flips back, or a unit that fails again
    // after recovering, differs from the last fact recorded and is recorded
    // afresh. Kinds that are observed side by side, such as a container's
    // creation and its running, are not here: each would displace the other
    // on every tick.
    pub(super) fn contradicting_kinds(kind: &str) -> &'static [&'static str] {
        match kind {
            "health_check_pass" | "health_check_fail" | "health_check_starting" => &[
                "health_check_pass",
                "health_check_fail",
                "health_check_starting",
            ],
            "backend_healthy" | "backend_lost" => &["backend_healthy", "backend_lost"],
            "cert_valid" | "cert_not_valid" => &["cert_valid", "cert_not_valid"],
            "cert_status_blocked"
            | "cert_status_expired"
            | "cert_status_error"
            | "cert_status_pending"
            | "cert_status_active"
            | "cert_status_none" => &[
                "cert_status_blocked",
                "cert_status_expired",
                "cert_status_error",
                "cert_status_pending",
                "cert_status_active",
                "cert_status_none",
            ],
            "unit_failed" | "unit_start_limit_hit" | "unit_inactive" => {
                &["unit_failed", "unit_start_limit_hit", "unit_inactive"]
            }
            _ => &[],
        }
    }

    // spec: OBS#recording
    // spec: HIST#observations
    // `batch` is collected from probing real podman/systemd state during the
    // current tick — every entry persisted here corresponds to a check or
    // event actually observed, never a synthesised or expected value.
    ///
    /// Returns the observations of `batch` that are on record, whether written
    /// now or before.
    pub(super) fn persist_obs(
        &mut self,
        batch: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    ) -> HashSet<(InstanceId, &'static str)> {
        let mut recorded = HashSet::new();
        for (instance, kind, payload) in batch {
            let id = instance.id;
            if !self.written_obs.insert((id, kind)) {
                tracing::trace!(
                    instance = %instance.display_name,
                    obs = kind,
                    "persist_obs: skipping already-written observation"
                );
                recorded.insert((id, kind));
                continue;
            }
            match self
                .db
                .call(move |db| insert_observation(db, &instance, kind, &payload))
            {
                Ok(()) => {
                    recorded.insert((id, kind));
                    // spec: OBS#recording
                    for other in Self::contradicting_kinds(kind) {
                        if *other != kind {
                            self.written_obs.remove(&(id, other));
                        }
                    }
                }
                Err(e) => {
                    // spec: OBS#recording
                    // Not recorded, so not marked as recorded: the next tick
                    // observes it again and writes it.
                    self.written_obs.remove(&(id, kind));
                    error!(
                        error = %e,
                        obs = kind,
                        "reconciler: failed to persist observation"
                    );
                }
            }
        }
        recorded
    }
}

/// Each app's active desired state, for the apps where it is known.
///
/// A steady snapshot is rechecked against the registry because an operation
/// may have started since it was taken, and an operation resolves its own
/// scaled groups. An app not installed desires nothing. An app the registry
/// does not hold may simply have failed to load, so it gets no entry.
// spec: GC#unscheduled-instances
fn known_desired_state(apps: &[AppSnapshot], registry: &AppRegistry) -> KnownDesiredState {
    let mut known = KnownDesiredState::default();
    for app in apps.iter().filter(|app| app.steady) {
        if registry
            .get(app.name.as_str())
            .is_some_and(AppEntry::is_steady)
        {
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
    for name in registry.names() {
        if registry.get(name.as_str()).is_some_and(|entry| {
            *entry.phase.lock() == AppPhase::NotInstalled
                && entry.active_progress.read().is_none()
                && !entry.has_dynamic_slots()
        }) {
            known.insert_app(&name, []);
        }
    }
    known
}

// spec: GC#demoted-instances
fn retire_eligible(torn_down: bool, has_observations: bool) -> bool {
    torn_down || !has_observations
}

#[cfg(test)]
mod tests {

    use seedling_protocol::names::AppName;

    use super::dedupe_groups;
    use crate::defs::resource::ResourceKind;

    fn app(s: &str) -> AppName {
        AppName::new(s).expect("app name")
    }

    // spec: OBS#recording
    // spec: OBS#ingresses
    #[test]
    fn a_certificate_fact_that_changes_back_is_recorded_again() {
        use crate::system::reconcile::Reconciler;
        for kind in ["cert_valid", "cert_not_valid"] {
            let group = Reconciler::contradicting_kinds(kind);
            assert!(group.contains(&"cert_valid") && group.contains(&"cert_not_valid"));
        }
        for status in ["blocked", "expired", "error", "pending", "active", "none"] {
            let kind = format!("cert_status_{status}");
            assert_eq!(Reconciler::contradicting_kinds(&kind).len(), 6, "{kind}");
        }
    }

    // spec: EVT#event-types
    // `desired.resources` has one entry per instance, so a deployment scaled
    // to 3 produced 3 identical groups; each then expanded to all 3
    // instances, so one state change was reported three times.
    #[test]
    fn instances_of_one_resource_collapse_to_a_single_group() {
        let raw = vec![
            (
                app("web"),
                ResourceKind::Deployment,
                Some("api".to_owned()),
                false,
            ),
            (
                app("web"),
                ResourceKind::Deployment,
                Some("api".to_owned()),
                false,
            ),
            (
                app("web"),
                ResourceKind::Deployment,
                Some("api".to_owned()),
                false,
            ),
        ];
        let groups = dedupe_groups(raw.into_iter());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].res_name.as_deref(), Some("api"));
        assert_eq!(groups[0].kind_str, "deployment");
    }

    // spec: EVT#event-types
    #[test]
    fn distinct_resources_stay_distinct() {
        let raw = vec![
            (
                app("web"),
                ResourceKind::Deployment,
                Some("api".to_owned()),
                false,
            ),
            (
                app("web"),
                ResourceKind::Deployment,
                Some("worker".to_owned()),
                false,
            ),
            (
                app("web"),
                ResourceKind::Service,
                Some("api".to_owned()),
                false,
            ),
            (
                app("other"),
                ResourceKind::Deployment,
                Some("api".to_owned()),
                false,
            ),
            (app("web"), ResourceKind::Volume, None, false),
        ];
        assert_eq!(dedupe_groups(raw.into_iter()).len(), 5);
    }
    use super::*;

    fn eligible(kinds: &[&str]) -> bool {
        let resource = crate::runtime::identity::ResourceInstance::new_singleton(
            app("web"),
            ResourceKind::Deployment,
            "api",
        );
        let obs: Vec<_> = kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| crate::runtime::history::WorldObservation {
                id: i as i64,
                recorded_at: i as i64,
                resource: resource.clone(),
                obs_kind: (*kind).to_owned(),
                payload: serde_json::Value::Null,
            })
            .collect();
        let state = crate::runtime::barrier::oracle::derive_lifecycle_state(&resource, &obs);
        retire_eligible(is_torn_down(state, &obs), !obs.is_empty())
    }

    // spec: GC#demoted-instances
    #[test]
    fn a_torn_down_instance_is_eligible() {
        assert!(eligible(&["container_running", "container_removed"]));
    }

    // spec: GC#demoted-instances
    #[test]
    fn an_instance_with_no_observations_is_eligible() {
        assert!(eligible(&[]));
    }

    // spec: GC#demoted-instances
    #[test]
    fn an_instance_still_actuating_is_not_eligible() {
        assert!(!eligible(&["container_created"]));
        assert!(!eligible(&["container_created", "container_running"]));
    }

    // spec: GC#demoted-instances
    // A removal observed in an earlier cycle says nothing of an instance that
    // has started again since.
    #[test]
    fn an_instance_that_started_a_later_cycle_is_not_eligible() {
        assert!(!eligible(&[
            "container_running",
            "container_removed",
            "container_created",
            "container_running",
        ]));
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
            declared_by: None,
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
                undeclared: Vec::new(),
            },
            steady: true,
            app_def: Default::default(),
            phase: AppPhase::Installed,
            phase_handle: std::sync::Arc::new(parking_lot::Mutex::new(AppPhase::Installed)),
            warm_cert_hostnames: Default::default(),
            app_priority: Default::default(),
            gate_keys: Default::default(),
            gates: Default::default(),
            install_seq: Some(1),
        }
    }

    // spec: GC#unscheduled-instances
    // spec: RCN#desired-state
    #[test]
    fn only_apps_with_a_known_desired_state_are_swept() {
        let scaled = || ResourceInstance::new_scaled(app("demo"), ResourceKind::Deployment, "web");
        let (member, excess, unrelated) = (scaled(), scaled(), scaled());

        let mut reg = AppRegistry::new();
        register(&mut reg, "steady", AppPhase::Installed);
        // Its snapshot was steady, but an operation has started since.
        register(&mut reg, "busy", AppPhase::Installed);
        *reg.get("busy").unwrap().active_progress.write() = Some(Default::default());
        // Likewise, but for a shell session that has opened.
        register(&mut reg, "sessioned", AppPhase::Installed);
        reg.get("sessioned")
            .unwrap()
            .sessions
            .open(seedling_protocol::names::SessionId::generate());
        register(&mut reg, "absent", AppPhase::NotInstalled);
        // Not installed, but a session that has ended is still tearing down.
        register(&mut reg, "ended", AppPhase::NotInstalled);
        reg.get("ended")
            .unwrap()
            .sessions
            .open(seedling_protocol::names::SessionId::generate());

        // A gate check is running, which leaves the app steady.
        register(&mut reg, "checking", AppPhase::Installed);
        reg.get("checking")
            .unwrap()
            .checks
            .open(crate::runtime::apps::CheckId::generate());
        // Not installed, but a gate check that has ended is still tearing down.
        register(&mut reg, "check-ended", AppPhase::NotInstalled);
        reg.get("check-ended")
            .unwrap()
            .checks
            .open(crate::runtime::apps::CheckId::generate());

        let apps = [
            steady_snapshot("steady", &[&member], &[&excess]),
            steady_snapshot("checking", &[&member], &[&excess]),
            steady_snapshot("busy", &[&member], &[]),
            steady_snapshot("sessioned", &[&member], &[]),
            // Snapshotted, but no longer held by the registry.
            steady_snapshot("unloaded", &[&member], &[]),
        ];
        let known = known_desired_state(&apps, &reg);

        assert!(!known.sweepable("steady", member.id));
        assert!(known.sweepable("steady", excess.id));
        assert!(known.sweepable("steady", unrelated.id));
        assert!(!known.sweepable("busy", unrelated.id));
        assert!(!known.sweepable("sessioned", unrelated.id));
        assert!(!known.sweepable("ended", unrelated.id));
        assert!(!known.sweepable("checking", member.id));
        assert!(known.sweepable("checking", excess.id));
        assert!(!known.sweepable("check-ended", unrelated.id));
        assert!(!known.sweepable("unloaded", unrelated.id));
        assert!(known.sweepable("absent", unrelated.id));
    }
}
