use crate::runtime::{faults, identity::ResourceInstance};

use super::{Reconciler, pods, volumes};

impl Reconciler {
    /// File a fault scoped to a specific resource instance, if no active fault
    /// of the same kind already exists for that instance. Used by callers that
    /// don't have a typed update struct (e.g. cert observation).
    // r[impl fault.detection]
    // r[impl fault.surfacing]
    pub(super) fn file_resource_fault(
        &self,
        instance: &ResourceInstance,
        fault_kind: &str,
        description: &str,
    ) {
        let instance = instance.clone();
        let fault_kind = fault_kind.to_owned();
        let description = description.to_owned();
        self.db.call(move |db| {
            let inst_hex = instance.id.to_hex();
            let kind_str = format!("{:?}", instance.kind).to_lowercase();
            let already_filed = faults::list_active_faults(db, Some(&instance.app))
                .unwrap_or_default()
                .iter()
                .any(|f| f.kind == fault_kind && f.instance_id.as_deref() == Some(&inst_hex));
            if !already_filed
                && let Err(e) = faults::file_fault(
                    db,
                    &instance.app,
                    Some(&kind_str),
                    instance.name.as_deref(),
                    Some(&inst_hex),
                    &fault_kind,
                    &description,
                )
            {
                tracing::warn!(app = %instance.app, instance = %inst_hex, kind = %fault_kind, "failed to file resource fault: {e}");
            }
        });
    }

    /// Clear all active faults of the given kind for a specific resource
    /// instance.
    pub(super) fn clear_resource_fault(&self, instance: &ResourceInstance, fault_kind: &str) {
        let instance = instance.clone();
        let fault_kind = fault_kind.to_owned();
        self.db.call(move |db| {
            let inst_hex = instance.id.to_hex();
            let cleared: Vec<_> = faults::list_active_faults(db, Some(&instance.app))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| f.kind == fault_kind && f.instance_id.as_deref() == Some(&inst_hex))
                .collect();
            for f in cleared {
                if let Err(e) = faults::clear_fault(db, &f.id, &instance.app) {
                    tracing::warn!(app = %instance.app, fault_id = %f.id, "failed to clear resource fault: {e}");
                }
            }
        });
    }

    // r[fault.image-pull]
    // An image_pull_failed fault is scoped to an IMAGE reference, not to a
    // specific instance — the fault's meaning is "this image is unavailable",
    // and anyone observing that image present on the system resolves it. We
    // therefore deduplicate on image ref at file time and clear on image ref
    // whenever any instance in the app successfully pulls the same image.
    pub(super) fn file_image_pull_faults(&self, app: &str, update: &pods::PodActuationUpdate) {
        let app = app.to_owned();
        let image_pull_failures: Vec<(ResourceInstance, String)> = update
            .image_pull_failures
            .iter()
            .map(|(inst, r)| (inst.clone(), r.clone()))
            .collect();
        let image_pull_successes: Vec<String> = update
            .image_pull_successes
            .iter()
            .map(|(_, r)| r.clone())
            .collect();
        self.db.call(move |db| {
            for (instance, reference) in &image_pull_failures {
                let inst_hex = instance.id.to_hex();
                let kind_str = format!("{:?}", instance.kind).to_lowercase();
                let desc = format!("failed to pull image: {reference}");
                // Dedupe on the fault's description: if another instance in
                // this app has already reported the same image unavailable,
                // we don't file a second fault. Operators see one fault per
                // broken image, not one per instance.
                let already_filed = faults::list_active_faults(db, Some(&app))
                    .unwrap_or_default()
                    .iter()
                    .any(|f| f.kind == "image_pull_failed" && f.description == desc);
                if !already_filed
                    && let Err(e) = faults::file_fault(
                        db,
                        &app,
                        Some(&kind_str),
                        instance.name.as_deref(),
                        Some(&inst_hex),
                        "image_pull_failed",
                        &desc,
                    ) {
                        tracing::warn!(app = %app, instance = %inst_hex, "failed to file image-pull fault: {e}");
                    }
            }
            for reference in &image_pull_successes {
                let desc = format!("failed to pull image: {reference}");
                let cleared: Vec<_> = faults::list_active_faults(db, Some(&app))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| f.kind == "image_pull_failed" && f.description == desc)
                    .collect();
                for f in cleared {
                    if let Err(e) = faults::clear_fault(db, &f.id, &app) {
                        tracing::warn!(app = %app, fault_id = %f.id, "failed to clear image-pull fault: {e}");
                    }
                }
            }
        });
    }

    // r[fault.container-start]
    pub(super) fn file_unit_failure_faults(&self, app: &str, update: &pods::PodActuationUpdate) {
        let app = app.to_owned();
        let unit_failures: Vec<ResourceInstance> = update.unit_failures.to_vec();
        let unit_healthy: Vec<ResourceInstance> = update.unit_healthy.to_vec();
        self.db.call(move |db| {
            for instance in &unit_failures {
                let inst_hex = instance.id.to_hex();
                let kind_str = format!("{:?}", instance.kind).to_lowercase();
                let already_filed = faults::list_active_faults(db, Some(&app))
                    .unwrap_or_default()
                    .iter()
                    .any(|f| {
                        f.kind == "container_start_failed"
                            && f.instance_id.as_deref() == Some(&inst_hex)
                    });
                if !already_filed {
                    let desc = format!("unit for {} entered failed state", instance.display_name);
                    if let Err(e) = faults::file_fault(
                        db,
                        &app,
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
            // When a unit is observed healthy, clear the transient per-instance
            // faults that the actuator path may have filed against it.
            // image_pull_failed is handled separately in file_image_pull_faults
            // because it is scoped by image reference rather than instance —
            // pods.rs now emits an image_pull_success for every healthy
            // instance which triggers that per-image clear.
            for instance in &unit_healthy {
                let inst_hex = instance.id.to_hex();
                let cleared: Vec<_> = faults::list_active_faults(db, Some(&app))
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
                    if let Err(e) = faults::clear_fault(db, &f.id, &app) {
                        tracing::warn!(app = %app, fault_id = %f.id, "failed to clear unit-failure fault: {e}");
                    }
                }
            }
        });
    }

    // r[impl fault.external-volume-unmapped]
    pub(super) fn file_external_volume_faults(&self, app: &str, update: &pods::PodActuationUpdate) {
        let app = app.to_owned();
        let external_volume_failures: Vec<(ResourceInstance, String)> = update
            .external_volume_failures
            .iter()
            .map(|(inst, name)| (inst.clone(), name.clone()))
            .collect();
        let unit_healthy: Vec<ResourceInstance> = update.unit_healthy.to_vec();
        self.db.call(move |db| {
            for (instance, vol_name) in &external_volume_failures {
                let inst_hex = instance.id.to_hex();
                let kind_str = format!("{:?}", instance.kind).to_lowercase();
                let already_filed = faults::list_active_faults(db, Some(&app))
                    .unwrap_or_default()
                    .iter()
                    .any(|f| {
                        f.kind == "external_volume_not_mapped"
                            && f.instance_id.as_deref() == Some(&inst_hex)
                    });
                if !already_filed {
                    let desc =
                        format!("external volume '{vol_name}' is not mapped for app '{app}'");
                    if let Err(e) = faults::file_fault(
                        db,
                        &app,
                        Some(&kind_str),
                        instance.name.as_deref(),
                        Some(&inst_hex),
                        "external_volume_not_mapped",
                        &desc,
                    ) {
                        tracing::warn!(app = %app, instance = %inst_hex, "failed to file external-volume-not-mapped fault: {e}");
                    }
                }
            }
            for instance in &unit_healthy {
                let inst_hex = instance.id.to_hex();
                let cleared: Vec<_> = faults::list_active_faults(db, Some(&app))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| {
                        f.kind == "external_volume_not_mapped"
                            && f.instance_id.as_deref() == Some(&inst_hex)
                    })
                    .collect();
                for f in cleared {
                    if let Err(e) = faults::clear_fault(db, &f.id, &app) {
                        tracing::warn!(app = %app, fault_id = %f.id, "failed to clear external-volume-not-mapped fault: {e}");
                    }
                }
            }
        });
    }

    pub(super) fn file_pod_actuation_faults(&self, app: &str, update: &pods::PodActuationUpdate) {
        let app = app.to_owned();
        let start_failures: Vec<(ResourceInstance, String)> = update
            .start_failures
            .iter()
            .map(|(i, s)| (i.clone(), s.clone()))
            .collect();
        let stop_failures: Vec<(ResourceInstance, String)> = update
            .stop_failures
            .iter()
            .map(|(i, s)| (i.clone(), s.clone()))
            .collect();
        let observe_failures: Vec<(ResourceInstance, String)> = update
            .observe_failures
            .iter()
            .map(|(i, s)| (i.clone(), s.clone()))
            .collect();
        let stopped_instances: Vec<String> = update
            .observations
            .iter()
            .filter(|(_, kind, _)| *kind == "stop_sent")
            .map(|(inst, _, _)| inst.id.to_hex())
            .collect();
        self.db.call(move |db| {
            Self::file_instance_faults(db, &app, &start_failures, "start_failed");
            Self::file_instance_faults(db, &app, &stop_failures, "stop_failed");
            Self::file_instance_faults(db, &app, &observe_failures, "observe_failed");

            if !stopped_instances.is_empty() {
                let active_faults = faults::list_active_faults(db, Some(&app)).unwrap_or_default();
                for f in active_faults {
                    if f.kind == "stop_failed"
                        && f.instance_id
                            .as_ref()
                            .is_some_and(|id| stopped_instances.contains(id))
                        && let Err(e) = faults::clear_fault(db, &f.id, &app)
                    {
                        tracing::warn!(app = %app, fault_id = %f.id, "failed to clear stop_failed fault: {e}");
                    }
                }
            }
        });
    }

    pub(super) fn file_volume_actuation_faults(
        &self,
        app: &str,
        update: &volumes::VolumeActuationUpdate,
    ) {
        let app = app.to_owned();
        let observe_failures: Vec<(ResourceInstance, String)> = update
            .observe_failures
            .iter()
            .map(|(i, s)| (i.clone(), s.clone()))
            .collect();
        let create_failures: Vec<(ResourceInstance, String)> = update
            .create_failures
            .iter()
            .map(|(i, s)| (i.clone(), s.clone()))
            .collect();
        let remove_failures: Vec<(ResourceInstance, String)> = update
            .remove_failures
            .iter()
            .map(|(i, s)| (i.clone(), s.clone()))
            .collect();
        self.db.call(move |db| {
            Self::file_instance_faults(db, &app, &observe_failures, "observe_failed");
            Self::file_instance_faults(db, &app, &create_failures, "volume_create_failed");
            Self::file_instance_faults(db, &app, &remove_failures, "volume_remove_failed");
        });
    }

    pub(super) fn file_registry_fault(&self, app: &str, description: &str) {
        let app = app.to_owned();
        let description = description.to_owned();
        self.db.call(move |db| {
            let already_filed = faults::list_active_faults(db, Some(&app))
                .unwrap_or_default()
                .iter()
                .any(|f| f.kind == "registry_error");
            if !already_filed
                && let Err(e) =
                    faults::file_fault(db, &app, None, None, None, "registry_error", &description)
            {
                tracing::warn!(app = %app, "failed to file registry fault: {e}");
            }
        });
    }

    pub(super) fn file_instance_registry_faults(
        &self,
        app: &str,
        update: &pods::PodActuationUpdate,
    ) {
        let app = app.to_owned();
        let registry_failures: Vec<ResourceInstance> = update.registry_failures.to_vec();
        let image_pull_successes: Vec<ResourceInstance> = update
            .image_pull_successes
            .iter()
            .map(|(inst, _)| inst.clone())
            .collect();
        self.db.call(move |db| {
            for instance in &registry_failures {
                Self::file_instance_registry_fault_inner(db, &app, instance);
            }
            for instance in &image_pull_successes {
                Self::clear_instance_registry_fault_inner(db, &app, instance);
            }
        });
    }

    pub fn file_system_fault(&self, fault_kind: &str, description: &str) {
        let fault_kind = fault_kind.to_owned();
        let description = description.to_owned();
        self.db.call(move |db| {
            let already_filed = faults::list_active_faults(db, Some("_system"))
                .unwrap_or_default()
                .iter()
                .any(|f| f.kind == fault_kind);
            if !already_filed
                && let Err(e) =
                    faults::file_fault(db, "_system", None, None, None, &fault_kind, &description)
            {
                tracing::warn!("failed to file system fault ({fault_kind}): {e}");
            }
        });
    }

    pub(super) fn clear_system_fault(&self, fault_kind: &str) {
        let fault_kind = fault_kind.to_owned();
        self.db.call(move |db| {
            let cleared: Vec<_> = faults::list_active_faults(db, Some("_system"))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| f.kind == fault_kind)
                .collect();
            for f in cleared {
                if let Err(e) = faults::clear_fault(db, &f.id, "_system") {
                    tracing::warn!(fault_id = %f.id, "failed to clear system fault ({fault_kind}): {e}");
                }
            }
        });
    }

    pub(super) fn clear_registry_faults(&self, app: &str) {
        let app = app.to_owned();
        self.db.call(move |db| {
            let cleared: Vec<_> = faults::list_active_faults(db, Some(&app))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| f.kind == "registry_error")
                .collect();
            for f in cleared {
                if let Err(e) = faults::clear_fault(db, &f.id, &app) {
                    tracing::warn!(app = %app, fault_id = %f.id, "failed to clear registry fault: {e}");
                }
            }
        });
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
