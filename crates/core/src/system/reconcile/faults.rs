use crate::runtime::{faults, identity::ResourceInstance};

use super::{Reconciler, pods, volumes};

impl Reconciler {
    /// File a fault scoped to a specific resource instance, if no active fault
    /// of the same kind already exists for that instance. Used by callers that
    /// don't have a typed update struct (e.g. cert observation).
    pub(super) fn file_resource_fault(
        &self,
        instance: &ResourceInstance,
        fault_kind: &str,
        description: &str,
    ) {
        let db = self.db.lock();
        let inst_hex = instance.id.to_hex();
        let kind_str = format!("{:?}", instance.kind).to_lowercase();
        let already_filed = faults::list_active_faults(&db, Some(&instance.app))
            .unwrap_or_default()
            .iter()
            .any(|f| f.kind == fault_kind && f.instance_id.as_deref() == Some(&inst_hex));
        if !already_filed
            && let Err(e) = faults::file_fault(
                &db,
                &instance.app,
                Some(&kind_str),
                instance.name.as_deref(),
                Some(&inst_hex),
                fault_kind,
                description,
            )
        {
            tracing::warn!(app = %instance.app, instance = %inst_hex, kind = %fault_kind, "failed to file resource fault: {e}");
        }
    }

    /// Clear all active faults of the given kind for a specific resource
    /// instance.
    pub(super) fn clear_resource_fault(&self, instance: &ResourceInstance, fault_kind: &str) {
        let db = self.db.lock();
        let inst_hex = instance.id.to_hex();
        let cleared: Vec<_> = faults::list_active_faults(&db, Some(&instance.app))
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.kind == fault_kind && f.instance_id.as_deref() == Some(&inst_hex))
            .collect();
        for f in cleared {
            if let Err(e) = faults::clear_fault(&db, &f.id, &instance.app) {
                tracing::warn!(app = %instance.app, fault_id = %f.id, "failed to clear resource fault: {e}");
            }
        }
    }

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
                    matches!(
                        f.kind.as_str(),
                        "container_start_failed"
                            | "start_failed"
                            | "observe_failed"
                            | "stop_failed"
                    ) && f.instance_id.as_deref() == Some(&inst_hex)
                })
                .collect();
            for f in cleared {
                if let Err(e) = faults::clear_fault(&db, &f.id, app) {
                    tracing::warn!(app = %app, fault_id = %f.id, "failed to clear unit-failure fault: {e}");
                }
            }
        }
    }

    pub(super) fn file_pod_actuation_faults(&self, app: &str, update: &pods::PodActuationUpdate) {
        let db = self.db.lock();
        Self::file_instance_faults(&db, app, &update.start_failures, "start_failed");
        Self::file_instance_faults(&db, app, &update.stop_failures, "stop_failed");
        Self::file_instance_faults(&db, app, &update.observe_failures, "observe_failed");

        // Clear stop_failed faults for instances that were successfully stopped
        // this tick (present in stop_failures last tick but not this tick and
        // the unit is no longer loaded) or that are now running healthy.
        // The unit_healthy path above covers the "now healthy" case; here we
        // cover instances whose desired state is Unscheduled and whose unit has
        // disappeared (the stop ultimately succeeded).
        let stopped_instances: Vec<_> = update
            .observations
            .iter()
            .filter(|(_, kind, _)| *kind == "stop_sent")
            .map(|(inst, _, _)| inst.id.to_hex())
            .collect();
        if !stopped_instances.is_empty() {
            let active_faults = faults::list_active_faults(&db, Some(app)).unwrap_or_default();
            for f in active_faults {
                if f.kind == "stop_failed"
                    && f.instance_id
                        .as_ref()
                        .is_some_and(|id| stopped_instances.contains(id))
                    && let Err(e) = faults::clear_fault(&db, &f.id, app)
                {
                    tracing::warn!(app = %app, fault_id = %f.id, "failed to clear stop_failed fault: {e}");
                }
            }
        }
    }

    pub(super) fn file_volume_actuation_faults(
        &self,
        app: &str,
        update: &volumes::VolumeActuationUpdate,
    ) {
        let db = self.db.lock();
        Self::file_instance_faults(&db, app, &update.observe_failures, "observe_failed");
        Self::file_instance_faults(&db, app, &update.create_failures, "volume_create_failed");
        Self::file_instance_faults(&db, app, &update.remove_failures, "volume_remove_failed");
    }

    pub(super) fn file_registry_fault(&self, app: &str, description: &str) {
        let db = self.db.lock();
        let already_filed = faults::list_active_faults(&db, Some(app))
            .unwrap_or_default()
            .iter()
            .any(|f| f.kind == "registry_error");
        if !already_filed
            && let Err(e) =
                faults::file_fault(&db, app, None, None, None, "registry_error", description)
        {
            tracing::warn!(app = %app, "failed to file registry fault: {e}");
        }
    }

    pub(super) fn file_instance_registry_faults(
        &self,
        app: &str,
        update: &pods::PodActuationUpdate,
    ) {
        let db = self.db.lock();
        for instance in &update.registry_failures {
            Self::file_instance_registry_fault_inner(&db, app, instance);
        }
        for (instance, _) in &update.image_pull_successes {
            Self::clear_instance_registry_fault_inner(&db, app, instance);
        }
    }

    pub fn file_system_fault(&self, fault_kind: &str, description: &str) {
        let db = self.db.lock();
        let already_filed = faults::list_active_faults(&db, Some("_system"))
            .unwrap_or_default()
            .iter()
            .any(|f| f.kind == fault_kind);
        if !already_filed
            && let Err(e) =
                faults::file_fault(&db, "_system", None, None, None, fault_kind, description)
        {
            tracing::warn!("failed to file system fault ({fault_kind}): {e}");
        }
    }

    pub(super) fn clear_system_fault(&self, fault_kind: &str) {
        let db = self.db.lock();
        let cleared: Vec<_> = faults::list_active_faults(&db, Some("_system"))
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.kind == fault_kind)
            .collect();
        for f in cleared {
            if let Err(e) = faults::clear_fault(&db, &f.id, "_system") {
                tracing::warn!(fault_id = %f.id, "failed to clear system fault ({fault_kind}): {e}");
            }
        }
    }

    pub(super) fn clear_registry_faults(&self, app: &str) {
        let db = self.db.lock();
        let cleared: Vec<_> = faults::list_active_faults(&db, Some(app))
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.kind == "registry_error")
            .collect();
        for f in cleared {
            if let Err(e) = faults::clear_fault(&db, &f.id, app) {
                tracing::warn!(app = %app, fault_id = %f.id, "failed to clear registry fault: {e}");
            }
        }
    }

    fn file_instance_registry_fault_inner(
        db: &crate::runtime::db::Db,
        app: &str,
        instance: &ResourceInstance,
    ) {
        let inst_hex = instance.id.to_hex();
        let kind_str = format!("{:?}", instance.kind).to_lowercase();
        let already_filed = faults::list_active_faults(db, Some(app))
            .unwrap_or_default()
            .iter()
            .any(|f| f.kind == "registry_error" && f.instance_id.as_deref() == Some(&inst_hex));
        if !already_filed {
            let desc = format!(
                "instance registry lookup failed for {}",
                instance.display_name,
            );
            if let Err(e) = faults::file_fault(
                db,
                app,
                Some(&kind_str),
                instance.name.as_deref(),
                Some(&inst_hex),
                "registry_error",
                &desc,
            ) {
                tracing::warn!(app = %app, instance = %inst_hex, "failed to file registry fault: {e}");
            }
        }
    }

    fn clear_instance_registry_fault_inner(
        db: &crate::runtime::db::Db,
        app: &str,
        instance: &ResourceInstance,
    ) {
        let inst_hex = instance.id.to_hex();
        let cleared: Vec<_> = faults::list_active_faults(db, Some(app))
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.kind == "registry_error" && f.instance_id.as_deref() == Some(&inst_hex))
            .collect();
        for f in cleared {
            if let Err(e) = faults::clear_fault(db, &f.id, app) {
                tracing::warn!(app = %app, fault_id = %f.id, "failed to clear registry fault: {e}");
            }
        }
    }

    fn file_instance_faults(
        db: &crate::runtime::db::Db,
        app: &str,
        failures: &[(ResourceInstance, String)],
        fault_kind: &str,
    ) {
        for (instance, description) in failures {
            let inst_hex = instance.id.to_hex();
            let kind_str = format!("{:?}", instance.kind).to_lowercase();
            let already_filed = faults::list_active_faults(db, Some(app))
                .unwrap_or_default()
                .iter()
                .any(|f| f.kind == fault_kind && f.instance_id.as_deref() == Some(&inst_hex));
            if !already_filed
                && let Err(e) = faults::file_fault(
                    db,
                    app,
                    Some(&kind_str),
                    instance.name.as_deref(),
                    Some(&inst_hex),
                    fault_kind,
                    description,
                )
            {
                tracing::warn!(app = %app, instance = %inst_hex, "failed to file {fault_kind} fault: {e}");
            }
        }
    }
}
