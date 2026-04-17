use std::{collections::BTreeMap, time::Duration};

use tracing::{error, warn};

use crate::runtime::{
    barrier::oracle::derive_lifecycle_state,
    history::{find_instances_for_group, insert_observation, query_observations},
    lifecycle::LifecycleState,
};

use super::{AppSnapshot, Reconciler};

/// Threshold after which we file a `cert_acquisition_failed` fault if a warm
/// cert hasn't been observed valid. Caddy's internal CA issues immediately;
/// public ACME issuance can take seconds to a minute. Three minutes leaves
/// generous margin for transient network issues.
// r[impl fault.cert-acquisition]
const CERT_ACQUISITION_DEADLINE: Duration = Duration::from_secs(180);

impl Reconciler {
    pub(super) fn emit_state_changes(&mut self, apps: &[AppSnapshot]) {
        let db = self.db.lock();
        let mut new_states = BTreeMap::new();

        for app in apps {
            for dr in &app.desired.resources {
                let kind_str = format!("{:?}", dr.instance.kind).to_lowercase();
                let res_name = dr.instance.name.as_deref().unwrap_or("");
                let inst_hex = dr.instance.id.to_hex();

                let instances = find_instances_for_group(
                    &db,
                    &app.name,
                    dr.instance.kind,
                    dr.instance.name.as_deref(),
                )
                .unwrap_or_default();

                for inst in &instances {
                    let hex = inst.id.to_hex();
                    let obs = query_observations(&db, inst).unwrap_or_default();
                    let state = derive_lifecycle_state(inst, &obs);
                    let key = (app.name.clone(), hex.clone());

                    if let Some(&prev) = self.prev_states.get(&key)
                        && prev != state
                    {
                        crate::oi::events::resource_state_changed(
                            &self.event_tx,
                            &app.name,
                            &kind_str,
                            res_name,
                            &hex,
                            &format!("{state:?}"),
                        );
                    }

                    new_states.insert(key, state);
                }

                if instances.is_empty() {
                    let key = (app.name.clone(), inst_hex);
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
        // Track which hostnames are still being awaited so we can prune the
        // first-seen map for ones that are no longer in any app's warm set.
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

        // Identify hostnames that are still un-acquired and have exceeded the
        // deadline → file fault. Conversely, hostnames whose cert appeared
        // clear any prior fault.
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

    // r[impl observe.persist]
    pub(super) fn persist_obs(
        &mut self,
        batch: Vec<(
            crate::runtime::identity::ResourceInstance,
            &'static str,
            serde_json::Value,
        )>,
    ) {
        for (instance, kind, payload) in batch {
            if !self.written_obs.insert((instance.id, kind)) {
                continue;
            }
            let db = self.db.lock();
            if let Err(e) = insert_observation(&db, &instance, kind, &payload) {
                error!(
                    error = %e,
                    instance = %instance.display_name,
                    obs = kind,
                    "reconciler: failed to persist observation"
                );
            }
        }
    }
}
