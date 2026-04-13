use std::collections::BTreeMap;

use tracing::error;

use crate::runtime::{
    barrier::oracle::derive_lifecycle_state,
    history::{find_instances_for_group, insert_observation, query_observations},
    lifecycle::LifecycleState,
};

use super::{AppSnapshot, Reconciler};

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
