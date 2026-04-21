use std::{collections::BTreeMap, time::Duration};

use tracing::{error, warn};

use crate::{
    defs::resource::ResourceKind,
    runtime::{
        AppPhase,
        barrier::oracle::derive_lifecycle_state,
        history::{
            delete_instance, find_instances_for_group, insert_observation, query_observations,
        },
        identity::{InstanceId, ResourceInstance},
        lifecycle::LifecycleState,
    },
};

use super::{AppSnapshot, Reconciler};

/// Threshold after which we file a `cert_acquisition_failed` fault if a warm
/// cert hasn't been observed valid. Caddy's internal CA issues immediately;
/// public ACME issuance can take seconds to a minute. Three minutes leaves
/// generous margin for transient network issues.
// r[impl fault.cert-acquisition]
const CERT_ACQUISITION_DEADLINE: Duration = Duration::from_secs(180);

/// Input to the DB lookup performed in emit_state_changes.
#[derive(Clone)]
struct DesiredGroup {
    app: String,
    kind: ResourceKind,
    res_name: Option<String>,
    kind_str: String,
}

/// Result returned from the DB lookup.
#[derive(Clone)]
struct StateEntry {
    app: String,
    kind_str: String,
    res_name: String,
    hex: String,
    inst_id: InstanceId,
    state: LifecycleState,
}

impl Reconciler {
    pub(super) fn emit_state_changes(&mut self, apps: &[AppSnapshot]) {
        let groups: Vec<DesiredGroup> = apps
            .iter()
            .flat_map(|app| {
                app.desired.resources.iter().map(move |dr| DesiredGroup {
                    app: app.name.clone(),
                    kind: dr.instance.kind,
                    res_name: dr.instance.name.as_deref().map(|s| s.to_owned()),
                    kind_str: format!("{:?}", dr.instance.kind).to_lowercase(),
                })
            })
            .collect();

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
                if dr.desired != LifecycleState::Unscheduled {
                    continue;
                }
                let instance = dr.instance.clone();
                let state = self.db.call(move |db| {
                    let obs = query_observations(db, &instance).unwrap_or_default();
                    derive_lifecycle_state(&instance, &obs)
                });
                match state {
                    LifecycleState::Unscheduled => {}
                    LifecycleState::Terminating | LifecycleState::Terminated => {
                        self.written_obs.retain(|(id, _)| *id != dr.instance.id);
                        tracing::debug!(
                            app = %app.name,
                            instance = %dr.instance.display_name,
                            "clearing written_obs for stuck-terminating excess instance"
                        );
                        continue;
                    }
                    _ => continue,
                }
                let instance_id = dr.instance.id;
                match self.db.call(move |db| delete_instance(db, instance_id)) {
                    Ok(()) => {
                        self.written_obs.retain(|(id, _)| *id != dr.instance.id);
                        tracing::debug!(
                            app = %app.name,
                            instance = %dr.instance.display_name,
                            "retired unscheduled excess instance"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            app = %app.name,
                            instance = %dr.instance.display_name,
                            error = %e,
                            "failed to retire unscheduled excess instance"
                        );
                    }
                }
            }
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
