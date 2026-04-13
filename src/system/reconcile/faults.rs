use crate::runtime::faults;

use super::{Reconciler, pods};

impl Reconciler {
    // r[fault.image-pull]
    pub(super) fn file_image_pull_faults(&self, app: &str, update: &pods::PodActuationUpdate) {
        let db = self.db.lock();
        for (instance, reference) in &update.image_pull_failures {
            let inst_hex = instance.id.to_hex();
            let kind_str = format!("{:?}", instance.kind).to_lowercase();
            let already_filed = faults::list_active_faults(&db, Some(app))
                .unwrap_or_default()
                .iter()
                .any(|f| {
                    f.kind == "image_pull_failed" && f.instance_id.as_deref() == Some(&inst_hex)
                });
            if !already_filed {
                let desc = format!("failed to pull image: {reference}");
                if let Err(e) = faults::file_fault(
                    &db,
                    app,
                    Some(&kind_str),
                    instance.name.as_deref(),
                    Some(&inst_hex),
                    "image_pull_failed",
                    &desc,
                ) {
                    tracing::warn!(app = %app, instance = %inst_hex, "failed to file image-pull fault: {e}");
                }
            }
        }
        for (instance, _reference) in &update.image_pull_successes {
            let inst_hex = instance.id.to_hex();
            let cleared: Vec<_> = faults::list_active_faults(&db, Some(app))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| {
                    f.kind == "image_pull_failed" && f.instance_id.as_deref() == Some(&inst_hex)
                })
                .collect();
            for f in cleared {
                if let Err(e) = faults::clear_fault(&db, &f.id, app) {
                    tracing::warn!(app = %app, fault_id = %f.id, "failed to clear image-pull fault: {e}");
                }
            }
        }
    }

    // r[fault.container-start]
    pub(super) fn file_unit_failure_faults(&self, app: &str, update: &pods::PodActuationUpdate) {
        let db = self.db.lock();
        for instance in &update.unit_failures {
            let inst_hex = instance.id.to_hex();
            let kind_str = format!("{:?}", instance.kind).to_lowercase();
            let already_filed = faults::list_active_faults(&db, Some(app))
                .unwrap_or_default()
                .iter()
                .any(|f| {
                    f.kind == "container_start_failed"
                        && f.instance_id.as_deref() == Some(&inst_hex)
                });
            if !already_filed {
                let desc = format!("unit for {} entered failed state", instance.display_name);
                if let Err(e) = faults::file_fault(
                    &db,
                    app,
                    Some(&kind_str),
                    instance.name.as_deref(),
                    Some(&inst_hex),
                    "container_start_failed",
                    &desc,
                ) {
                    tracing::warn!(app = %app, instance = %inst_hex, "failed to file unit-failure fault: {e}");
                }
            }
        }
        for instance in &update.unit_healthy {
            let inst_hex = instance.id.to_hex();
            let cleared: Vec<_> = faults::list_active_faults(&db, Some(app))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| {
                    f.kind == "container_start_failed"
                        && f.instance_id.as_deref() == Some(&inst_hex)
                })
                .collect();
            for f in cleared {
                if let Err(e) = faults::clear_fault(&db, &f.id, app) {
                    tracing::warn!(app = %app, fault_id = %f.id, "failed to clear unit-failure fault: {e}");
                }
            }
        }
    }
}
