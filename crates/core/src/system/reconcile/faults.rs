use std::collections::BTreeMap;

use seedling_protocol::names::AppName;

use super::{Reconciler, pods, volumes};
use crate::runtime::{
    db::Db, faults, identity::ResourceInstance, site_services::resolve::UnroutableReason,
};

impl Reconciler {
    /// File a fault scoped to a specific resource instance, if no active fault
    /// of the same kind already exists for that instance. Used by callers that
    /// don't have a typed update struct (e.g. cert observation).
    // spec: FLT#filing-and-clearing
    // spec: FLT#fault-records
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

    // spec: FLT#workload-faults
    // An image_pull_failed fault is scoped to an IMAGE reference, not to a
    // specific instance — the fault's meaning is "this image is unavailable",
    // and anyone observing that image present on the system resolves it. We
    // therefore deduplicate on image ref at file time and clear on image ref
    // whenever any instance in the app successfully pulls the same image.
    pub(super) fn file_image_pull_faults(&self, app: &AppName, update: &pods::PodActuationUpdate) {
        let app = app.clone();
        let image_pull_failures: Vec<String> = update
            .image_pull_failures
            .iter()
            .map(|(_, reference)| reference.clone())
            .collect();
        let image_pull_successes: Vec<String> = update
            .image_pull_successes
            .iter()
            .map(|(_, r)| r.clone())
            .collect();
        self.db.call(move |db| {
            // Keyed by the image reference, so operators see one fault per
            // broken image, however many instances need it.
            for reference in &image_pull_failures {
                let key = faults::FaultKey::new(&app, "image_pull_failed", reference.as_str());
                let desc = format!("failed to pull image: {reference}");
                if let Err(e) = faults::file_once(db, &key, &faults::FaultMeta::default(), &desc) {
                    tracing::warn!(app = %app, image = %reference, "failed to file image-pull fault: {e}");
                }
            }
            for reference in &image_pull_successes {
                let key = faults::FaultKey::new(&app, "image_pull_failed", reference.as_str());
                if let Err(e) = faults::clear_fault_by_key(db, &key) {
                    tracing::warn!(app = %app, image = %reference, "failed to clear image-pull fault: {e}");
                }
            }
        });
    }

    // spec: FLT#workload-faults
    pub(super) fn file_replace_failed_fault(
        &self,
        app: &AppName,
        deployment: &str,
        replacement_display: &str,
    ) {
        let app = app.clone();
        let deployment = deployment.to_owned();
        let replacement_display = replacement_display.to_owned();
        self.db.call(move |db| {
            let desc = format!(
                "automatic replacement {replacement_display} for deployment '{deployment}' failed to become healthy; original instance kept running in degraded mode pending operator action"
            );
            // Scope the fault to the deployment, not the failed instance. The
            // failed replacement gets retired shortly after this fires, and
            // instance-scoped faults are auto-cleared by
            // `retire_unscheduled_excess` / `clear_faults_for_instance` —
            // which would silently remove our hard fault. The fault must
            // outlive the instance that triggered it; it clears only when the
            // generation changes.
            let key = faults::FaultKey::new(&app, "health_check_replace_failed", deployment.as_str());
            let meta = faults::FaultMeta::resource("deployment", deployment.as_str());
            if let Err(e) = faults::file_once(db, &key, &meta, &desc) {
                tracing::warn!(app = %app, deployment = %deployment, "failed to file health_check_replace_failed fault: {e}");
            }
        });
    }

    // spec: FLT#ingress-and-service-faults
    /// File a `service_degraded` fault per service whose routing pool has
    /// fallen back to "anything running" because no healthy backend exists,
    /// and clear the fault for any service that isn't in the degraded set
    /// this tick.
    pub(super) fn file_service_degraded_faults(
        &self,
        apps: &[super::AppSnapshot],
        degraded_by_app: &std::collections::HashMap<AppName, std::collections::BTreeSet<String>>,
    ) {
        // Snapshot what we need into owned data so the closure can move.
        let updates: Vec<(AppName, std::collections::BTreeSet<String>)> = apps
            .iter()
            .map(|app| {
                (
                    app.name.clone(),
                    degraded_by_app.get(&app.name).cloned().unwrap_or_default(),
                )
            })
            .collect();
        self.db.call(move |db| {
            for (app, degraded) in &updates {
                // File new degraded faults that aren't already active.
                for svc in degraded {
                    let already = faults::list_active_faults(db, Some(app))
                        .unwrap_or_default()
                        .iter()
                        .any(|f| {
                            f.kind == "service_degraded"
                                && f.resource_name.as_deref() == Some(svc.as_str())
                        });
                    if !already
                        && let Err(e) = faults::file_fault(
                            db,
                            app,
                            Some("service"),
                            Some(svc.as_str()),
                            None,
                            "service_degraded",
                            &format!(
                                "service '{svc}' has no healthy backend; routing to running-but-unhealthy backends"
                            ),
                        )
                    {
                        tracing::warn!(app = %app, service = %svc, "failed to file service_degraded fault: {e}");
                    }
                }
                // Clear degraded faults for services no longer in the set.
                let active = faults::list_active_faults(db, Some(app)).unwrap_or_default();
                for f in active {
                    if f.kind == "service_degraded"
                        && let Some(svc) = f.resource_name.as_deref()
                        && !degraded.contains(svc)
                        && let Err(e) = faults::clear_fault(db, &f.id, app)
                    {
                        tracing::warn!(app = %app, fault_id = %f.id, "failed to clear service_degraded fault: {e}");
                    }
                }
            }
        });
    }

    // spec: FLT#workload-faults
    pub(super) fn file_health_check_faults(
        &self,
        app: &AppName,
        update: &pods::PodActuationUpdate,
    ) {
        let app = app.clone();
        let failures: Vec<ResourceInstance> = update.health_check_failures.to_vec();
        let passes: Vec<ResourceInstance> = update.health_check_passes.to_vec();
        self.db.call(move |db| {
            for instance in &failures {
                let inst_hex = instance.id.to_hex();
                let kind_str = format!("{:?}", instance.kind).to_lowercase();
                let already_filed = faults::list_active_faults(db, Some(&app))
                    .unwrap_or_default()
                    .iter()
                    .any(|f| {
                        f.kind == "health_check_failed"
                            && f.instance_id.as_deref() == Some(&inst_hex)
                    });
                if !already_filed {
                    let desc = format!(
                        "healthcheck for {} has been failing past its grace window",
                        instance.display_name,
                    );
                    if let Err(e) = faults::file_fault(
                        db,
                        &app,
                        Some(&kind_str),
                        instance.name.as_deref(),
                        Some(&inst_hex),
                        "health_check_failed",
                        &desc,
                    ) {
                        tracing::warn!(app = %app, instance = %inst_hex, "failed to file health_check_failed fault: {e}");
                    }
                }
            }
            for instance in &passes {
                let inst_hex = instance.id.to_hex();
                let cleared: Vec<_> = faults::list_active_faults(db, Some(&app))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| {
                        f.kind == "health_check_failed"
                            && f.instance_id.as_deref() == Some(&inst_hex)
                    })
                    .collect();
                for f in cleared {
                    if let Err(e) = faults::clear_fault(db, &f.id, &app) {
                        tracing::warn!(app = %app, fault_id = %f.id, "failed to clear health_check_failed fault: {e}");
                    }
                }
            }
        });
    }

    // spec: FLT#workload-faults
    pub(super) fn file_unit_failure_faults(
        &self,
        app: &AppName,
        update: &pods::PodActuationUpdate,
    ) {
        let app = app.clone();
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

    // spec: FLT#workload-faults
    /// File a `crash_loop` fault for each instance the restart bookkeeping or
    /// the start-limit observation has flagged. This is a hard fault: the
    /// runtime stops auto-recovering until the operator clears it (typically
    /// by fixing config and reinstalling, which generates a new instance ID
    /// and therefore a fresh fault scope).
    pub(super) fn file_crash_loop_faults(&self, app: &AppName, update: &pods::PodActuationUpdate) {
        let app = app.clone();
        let crash_loops: Vec<pods::CrashLoop> = update.crash_loops.to_vec();
        let unit_healthy: Vec<ResourceInstance> = update.unit_healthy.to_vec();
        self.db
            .call(move |db| apply_crash_loop_faults(db, &app, &crash_loops, &unit_healthy));
    }

    // spec: FLT#workload-faults
    pub(super) fn file_external_volume_faults(
        &self,
        app: &AppName,
        update: &pods::PodActuationUpdate,
    ) {
        let app = app.clone();
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

    /// How many consecutive ticks an instance's observation must fail before
    /// it becomes an operator-visible fault.
    ///
    /// A single failed probe is a blip — podman or systemd momentarily busy —
    /// and minting a fault for it trains operators to ignore the fault list.
    /// The failure is logged on the first tick regardless; the threshold gates
    /// only the fault.
    // spec: OBS#failed-observation
    const OBSERVE_FAULT_AFTER_FAILURES: u32 = 3;

    /// Advance the consecutive-observation-failure counters and return the
    /// instances that have now failed often enough to fault.
    // spec: OBS#failed-observation
    fn escalate_observe_failures(
        &mut self,
        failures: &[(ResourceInstance, String)],
        observed: &[ResourceInstance],
    ) -> Vec<(ResourceInstance, String)> {
        // An instance observed successfully this tick has its streak reset, so
        // a flapping probe never accumulates to a fault. Only those this
        // caller observed are reset: the streaks are shared by every app and
        // every kind of resource, so keeping only this call's failing set
        // dropped everyone else's on each call.
        for instance in observed {
            self.observe_failure_streaks.remove(&instance.id);
        }

        let mut escalated = Vec::new();
        for (instance, error) in failures {
            let streak = self.observe_failure_streaks.entry(instance.id).or_insert(0);
            *streak += 1;
            if *streak >= Self::OBSERVE_FAULT_AFTER_FAILURES {
                escalated.push((instance.clone(), error.clone()));
            }
        }
        escalated
    }

    pub(super) fn file_pod_actuation_faults(
        &mut self,
        app: &AppName,
        update: &pods::PodActuationUpdate,
    ) {
        let app = app.clone();
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
        let observed: Vec<ResourceInstance> = update.observed.to_vec();
        let observe_failures =
            self.escalate_observe_failures(&update.observe_failures, &update.observed);
        // spec: FLT#filing-and-clearing — the file set and the clear set must be
        // disjoint per instance. `stop_sent` is recorded before the stop is
        // attempted, so an instance whose stop just failed appears in both:
        // its `stop_failed` fault was filed and then cleared in the same tick,
        // and the operator never saw a stop that keeps failing.
        let failed_stops: std::collections::HashSet<String> = update
            .stop_failures
            .iter()
            .map(|(inst, _)| inst.id.to_hex())
            .collect();
        let stopped_instances: Vec<String> = update
            .observations
            .iter()
            .filter(|(_, kind, _)| *kind == "stop_sent")
            .map(|(inst, _, _)| inst.id.to_hex())
            .filter(|hex| !failed_stops.contains(hex))
            .collect();
        self.db.call(move |db| {
            Self::file_instance_faults(db, &app, &start_failures, "start_failed");
            Self::file_instance_faults(db, &app, &stop_failures, "stop_failed");
            Self::file_instance_faults(db, &app, &observe_failures, "observe_failed");
            Self::clear_observe_failed(db, &app, &observed);

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

    /// Clear every `cert_issue_failed` fault whose hostname is not one of
    /// `needing`, the hostnames some ingress needs a certificate for.
    // spec: FLT#certificate-faults
    pub(super) fn clear_cert_issue_faults_not_needed(
        &self,
        needing: std::collections::BTreeSet<String>,
    ) {
        self.db.call(move |db| {
            let system = AppName::new_unchecked("_system");
            let active = match faults::list_active_faults(db, Some(&system)) {
                Ok(active) => active,
                Err(e) => {
                    tracing::warn!(
                        "could not read the host's faults to clear cert_issue_failed: {e}"
                    );
                    return;
                }
            };
            for f in active
                .iter()
                .filter(|f| f.kind == "cert_issue_failed" && !needing.contains(&f.subject))
            {
                let key = faults::FaultKey::new(&system, &f.kind, &f.subject);
                if let Err(e) = faults::clear_fault_by_key(db, &key) {
                    tracing::warn!(hostname = %f.subject, "failed to clear cert_issue_failed: {e}");
                }
            }
        });
    }

    // spec: FLT#volume-faults
    pub(super) fn file_volume_actuation_faults(
        &mut self,
        app: &AppName,
        update: &volumes::VolumeActuationUpdate,
    ) {
        let app = app.clone();
        let observe_failures =
            self.escalate_observe_failures(&update.observe_failures, &update.observed);
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
        let observed: Vec<ResourceInstance> = update.observed.to_vec();
        // The faults filed and cleared in one pass share no key: an instance
        // whose create or removal failed this tick is not cleared for it.
        let failed_creates: std::collections::HashSet<_> =
            create_failures.iter().map(|(i, _)| i.id).collect();
        let present: Vec<ResourceInstance> = update
            .present
            .iter()
            .filter(|i| !failed_creates.contains(&i.id))
            .cloned()
            .collect();
        let failed_removes: std::collections::HashSet<_> =
            remove_failures.iter().map(|(i, _)| i.id).collect();
        let removed: Vec<ResourceInstance> = update
            .removed
            .iter()
            .filter(|i| !failed_removes.contains(&i.id))
            .cloned()
            .collect();
        self.db.call(move |db| {
            Self::file_instance_faults(db, &app, &observe_failures, "observe_failed");
            Self::clear_observe_failed(db, &app, &observed);
            Self::file_instance_faults(db, &app, &create_failures, "volume_create_failed");
            Self::clear_instance_faults(db, &app, "volume_create_failed", &present);
            Self::file_instance_faults(db, &app, &remove_failures, "volume_remove_failed");
            Self::clear_instance_faults(db, &app, "volume_remove_failed", &removed);
        });
    }

    pub(super) fn file_registry_fault(&self, app: &AppName, description: &str) {
        let app = app.clone();
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
        app: &AppName,
        update: &pods::PodActuationUpdate,
    ) {
        let app = app.clone();
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

    /// Converge `ingress_conflict` faults to the current `(hostname, port)`
    /// collision set: one fault per involved app ingress, one per app ingress
    /// left out for another app's claim, and one against `_system` per
    /// involved site ingress, and nothing else.
    ///
    /// This used to clear only `prior \\ current`, where `prior` was a
    /// `Reconciler` field that starts empty on every daemon start — so a
    /// conflict fault filed before a restart could never clear, however long
    /// the conflict had been resolved. Comparing against the persisted active
    /// set instead needs no warm-up state and is restart-safe by construction.
    // spec: SITE#ingress-conflicts
    // spec: FLT#filing-and-clearing
    pub(super) fn reconcile_ingress_conflicts(
        &mut self,
        report: &super::site_proxy::ConflictReport,
    ) {
        let parties = report.parties.clone();
        let app_claims = report.app_claims.clone();
        self.db.call(move |db| {
            let system = AppName::new_unchecked("_system");
            let mut current: BTreeMap<faults::FaultKey, (faults::FaultMeta, String)> =
                BTreeMap::new();

            // spec: ING#claims-across-apps
            for lost in &app_claims {
                let (served_app, served_ingress) = &lost.served;
                current.insert(
                    faults::FaultKey::new(&lost.app, "ingress_conflict", lost.ingress.as_str()),
                    (
                        faults::FaultMeta::resource("ingress", lost.ingress.as_str()),
                        format!(
                            "ingress conflict on ({}:{}) with app ingress {served_app}/\
                             {served_ingress}, whose app installed first; this ingress is left \
                             out of the proxy while that one claims them",
                            lost.hostname, lost.port
                        ),
                    ),
                );
            }

            for party in &parties {
                let app_summary: Vec<String> = party
                    .apps
                    .iter()
                    .map(|(a, ing)| format!("{a}/{ing}"))
                    .collect();
                let site_summary: Vec<String> = party.site.clone();
                let host_port = format!("{}:{}", party.hostname, party.port);

                for (app, ingress_name) in &party.apps {
                    // Subject is the party's own ingress, so one app's fault
                    // clearing cannot depend on another app's still standing.
                    current.insert(
                        faults::FaultKey::new(app, "ingress_conflict", ingress_name.as_str()),
                        (
                            faults::FaultMeta::resource("ingress", ingress_name.as_str()),
                            format!(
                                "ingress conflict on ({host_port}) with site ingress(es) \
                                 {site_summary:?}; both sides are dropped from the proxy until \
                                 resolved"
                            ),
                        ),
                    );
                }

                for site_name in &party.site {
                    current.insert(
                        faults::FaultKey::new(&system, "ingress_conflict", site_name.as_str()),
                        (
                            faults::FaultMeta::resource("site_ingress", site_name.as_str()),
                            format!(
                                "ingress conflict on ({host_port}) with app ingress(es) \
                                 {app_summary:?}; both sides are dropped from the proxy until \
                                 resolved"
                            ),
                        ),
                    );
                }
            }

            if let Err(e) = faults::sync_faults(
                db,
                &faults::FaultScope::Kind("ingress_conflict".to_owned()),
                &current,
            ) {
                tracing::warn!("failed to converge ingress_conflict faults: {e}");
            }
        });
    }

    /// File a `site_ingress_target_missing` fault for each unresolved
    /// site-ingress attachment. We dedup on (site_ingress, port, protocol)
    /// so a stable misconfiguration doesn't fan out into duplicate faults.
    // spec: SITE#attachments
    pub(super) fn reconcile_unresolved_site_attachments(
        &self,
        unresolved: &[super::site_proxy::UnresolvedAttachment],
    ) {
        let entries: Vec<_> = unresolved.to_vec();
        self.db.call(move |db| {
            let system = AppName::new_unchecked("_system");
            // The subject is the attachment: its site ingress, port and
            // protocol. Not part of the description, which is free to change.
            let mut current: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            for entry in &entries {
                let subject = format!(
                    "{}:{}:{}",
                    entry.site_ingress, entry.port, entry.protocol
                );
                let desc = format!(
                    "{} (site ingress {} port {} protocol {})",
                    entry.reason, entry.site_ingress, entry.port, entry.protocol
                );
                let key =
                    faults::FaultKey::new(&system, "site_ingress_target_missing", subject.as_str());
                let meta = faults::FaultMeta::resource("site_ingress", entry.site_ingress.as_str());
                if let Err(e) = faults::file_once(db, &key, &meta, &desc) {
                    tracing::warn!(site_ingress = %entry.site_ingress, "failed to file site_ingress_target_missing fault: {e}");
                }
                current.insert(subject);
            }
            // Clear the attachments no longer unresolved, and only those.
            let active = match faults::list_active_faults(db, Some(&system)) {
                Ok(active) => active,
                Err(e) => {
                    tracing::warn!("could not read the host's faults to clear site_ingress_target_missing: {e}");
                    return;
                }
            };
            for f in active
                .iter()
                .filter(|f| f.kind == "site_ingress_target_missing" && !current.contains(&f.subject))
            {
                let key = faults::FaultKey::new(&system, &f.kind, &f.subject);
                if let Err(e) = faults::clear_fault_by_key(db, &key) {
                    tracing::warn!(fault_id = %f.id, "failed to clear site_ingress_target_missing fault: {e}");
                }
            }
        });
    }

    pub fn file_system_fault(&self, fault_kind: &str, description: &str) {
        let fault_kind = fault_kind.to_owned();
        let description = description.to_owned();
        self.db.call(move |db| {
            let system = AppName::new_unchecked("_system");
            let already_filed = faults::list_active_faults(db, Some(&system))
                .unwrap_or_default()
                .iter()
                .any(|f| f.kind == fault_kind);
            if !already_filed
                && let Err(e) =
                    faults::file_fault(db, &system, None, None, None, &fault_kind, &description)
            {
                tracing::warn!("failed to file system fault ({fault_kind}): {e}");
            }
        });
    }

    /// Converge `site_service_endpoint_unresolvable` and
    /// `site_service_endpoint_unroutable` faults to the current per-service
    /// classification, with the failing hosts in the description.
    ///
    /// Like the ingress-conflict sweep, this used to clear only
    /// `prior \\ current` from an in-memory field that empties on restart, so
    /// a fault outlived the condition across a daemon restart.
    // spec: SITE#endpoint-addressing
    // spec: FLT#filing-and-clearing
    pub(super) fn reconcile_site_service_faults(
        &mut self,
        set: super::phases::SiteServiceFaultSet,
    ) {
        const KIND_UNRESOLVABLE: &str = "site_service_endpoint_unresolvable";
        const KIND_UNROUTABLE: &str = "site_service_endpoint_unroutable";

        let unresolvable = set.unresolvable.clone();
        let unroutable = set.unroutable.clone();
        self.db.call(move |db| {
            let system = AppName::new_unchecked("_system");
            let mut by_kind: BTreeMap<&'static str, BTreeMap<faults::FaultKey, (faults::FaultMeta, String)>> =
                BTreeMap::new();

            for (name, hosts) in &unresolvable {
                by_kind.entry(KIND_UNRESOLVABLE).or_default().insert(
                    faults::FaultKey::new(&system, KIND_UNRESOLVABLE, name.as_str()),
                    (
                        faults::FaultMeta::resource("site_service", name.as_str()),
                        format!(
                            "site service {:?} has DNS-named endpoint(s) that failed to resolve: {}",
                            name.as_str(),
                            hosts.join(", "),
                        ),
                    ),
                );
            }
            for (name, hosts) in &unroutable {
                // The two reasons need different remedies, so they get
                // different sentences: reporting an AAAA-only host with no
                // v6 egress as "NAT64 is not active" sent operators to check
                // a setting that was already on and could not have helped.
                let mut needs_nat64: Vec<&str> = Vec::new();
                let mut needs_v6: Vec<&str> = Vec::new();
                for (host, reason) in hosts {
                    match reason {
                        UnroutableReason::NeedsNat64ButDisabled => needs_nat64.push(host),
                        UnroutableReason::NoIpv6Egress => needs_v6.push(host),
                    }
                }
                let mut parts: Vec<String> = Vec::new();
                if !needs_nat64.is_empty() {
                    parts.push(format!(
                        "endpoint(s) reachable only over IPv4, but NAT64 is not active: {}",
                        needs_nat64.join(", ")
                    ));
                }
                if !needs_v6.is_empty() {
                    parts.push(format!(
                        "endpoint(s) with only AAAA records, but this host has no IPv6 egress: {}",
                        needs_v6.join(", ")
                    ));
                }
                by_kind.entry(KIND_UNROUTABLE).or_default().insert(
                    faults::FaultKey::new(&system, KIND_UNROUTABLE, name.as_str()),
                    (
                        faults::FaultMeta::resource("site_service", name.as_str()),
                        format!("site service {:?} has {}", name.as_str(), parts.join("; ")),
                    ),
                );
            }

            // Each kind converges within its own scope, so an empty set for
            // one kind clears that kind without touching the other.
            for kind in [KIND_UNRESOLVABLE, KIND_UNROUTABLE] {
                let current = by_kind.remove(kind).unwrap_or_default();
                if let Err(e) = faults::sync_faults(
                    db,
                    &faults::FaultScope::AppKind(system.clone(), kind.to_owned()),
                    &current,
                ) {
                    tracing::warn!("failed to converge {kind} faults: {e}");
                }
            }
        });
    }

    pub(super) fn clear_system_fault(&self, fault_kind: &str) {
        let fault_kind = fault_kind.to_owned();
        self.db.call(move |db| {
            let system = AppName::new_unchecked("_system");
            let cleared: Vec<_> = faults::list_active_faults(db, Some(&system))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| f.kind == fault_kind)
                .collect();
            for f in cleared {
                if let Err(e) = faults::clear_fault(db, &f.id, &system) {
                    tracing::warn!(fault_id = %f.id, "failed to clear system fault ({fault_kind}): {e}");
                }
            }
        });
    }

    /// File a `proxy_apply_failed` fault against every ingress whose data
    /// was included in the proxy config that just failed to apply. App
    /// ingresses get a fault on their owning app; site ingresses get one
    /// on the `_system` sentinel app, mirroring the convention used by
    /// `ingress_conflict` and `site_ingress_target_missing`. Filing is
    /// idempotent — an existing matching fault is left in place rather
    /// than re-filed each tick.
    // spec: FLT#ingress-and-service-faults
    pub(super) fn file_proxy_apply_failed_faults(
        &self,
        app_ingresses: Vec<(AppName, String)>,
        site_ingress_names: Vec<String>,
        description: &str,
    ) {
        let description = description.to_owned();
        self.db.call(move |db| {
            let system = AppName::new_unchecked("_system");
            for (app, ingress_name) in &app_ingresses {
                let already = faults::list_active_faults(db, Some(app))
                    .unwrap_or_default()
                    .iter()
                    .any(|f| {
                        f.kind == "proxy_apply_failed"
                            && f.resource_type.as_deref() == Some("ingress")
                            && f.resource_name.as_deref() == Some(ingress_name.as_str())
                    });
                if already {
                    continue;
                }
                if let Err(e) = faults::file_fault(
                    db,
                    app,
                    Some("ingress"),
                    Some(ingress_name.as_str()),
                    None,
                    "proxy_apply_failed",
                    &description,
                ) {
                    tracing::warn!(app = %app, ingress = %ingress_name, "failed to file proxy_apply_failed fault: {e}");
                }
            }
            for site_name in &site_ingress_names {
                let already = faults::list_active_faults(db, Some(&system))
                    .unwrap_or_default()
                    .iter()
                    .any(|f| {
                        f.kind == "proxy_apply_failed"
                            && f.resource_type.as_deref() == Some("site_ingress")
                            && f.resource_name.as_deref() == Some(site_name.as_str())
                    });
                if already {
                    continue;
                }
                if let Err(e) = faults::file_fault(
                    db,
                    &system,
                    Some("site_ingress"),
                    Some(site_name.as_str()),
                    None,
                    "proxy_apply_failed",
                    &description,
                ) {
                    tracing::warn!(site_ingress = %site_name, "failed to file proxy_apply_failed fault: {e}");
                }
            }
        });
    }

    /// Clear every active `proxy_apply_failed` fault, regardless of which
    /// app or site ingress it was filed against. Called when a proxy apply
    /// succeeds: at that point every ingress in the just-applied config is
    /// healthy, so any lingering fault from a prior failed tick is stale.
    // spec: FLT#ingress-and-service-faults
    pub(super) fn clear_proxy_apply_failed_faults(&self) {
        self.db.call(|db| {
            let active = faults::list_active_faults(db, None).unwrap_or_default();
            for f in active {
                if f.kind != "proxy_apply_failed" {
                    continue;
                }
                if let Err(e) = faults::clear_fault(db, &f.id, &f.app) {
                    tracing::warn!(fault_id = %f.id, "failed to clear proxy_apply_failed fault: {e}");
                }
            }
        });
    }

    /// Clear the app-wide `registry_error` fault, and not the instance-scoped
    /// ones, which clear when their own instance is next started.
    // spec: FLT#filing-and-clearing
    pub(super) fn clear_registry_faults(&self, app: &AppName) {
        let app = app.clone();
        self.db.call(move |db| {
            let key = faults::FaultKey::app_wide(&app, "registry_error");
            if let Err(e) = faults::clear_fault_by_key(db, &key) {
                tracing::warn!(app = %app, "failed to clear registry fault: {e}");
            }
        });
    }

    fn file_instance_registry_fault_inner(
        db: &crate::runtime::db::Db,
        app: &AppName,
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
        app: &AppName,
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

    /// Clear the `kind` fault held by each of `instances`, and no other.
    // spec: FLT#filing-and-clearing
    fn clear_instance_faults(
        db: &crate::runtime::db::Db,
        app: &AppName,
        kind: &str,
        instances: &[ResourceInstance],
    ) {
        for instance in instances {
            let key = faults::FaultKey::new(app, kind, instance.id.to_hex());
            if let Err(e) = faults::clear_fault_by_key(db, &key) {
                tracing::warn!(app = %app, instance = %key.subject, "failed to clear {kind} fault: {e}");
            }
        }
    }

    /// Clear the `observe_failed` fault of each instance observed successfully
    /// this iteration: the observation it reports failing has since succeeded.
    // spec: OBS#failed-observation
    // spec: FLT#filing-and-clearing
    fn clear_observe_failed(
        db: &crate::runtime::db::Db,
        app: &AppName,
        observed: &[ResourceInstance],
    ) {
        if observed.is_empty() {
            return;
        }
        let hexes: std::collections::HashSet<String> =
            observed.iter().map(|i| i.id.to_hex()).collect();
        for f in faults::list_active_faults(db, Some(app)).unwrap_or_default() {
            if f.kind == "observe_failed"
                && f.instance_id.as_ref().is_some_and(|id| hexes.contains(id))
                && let Err(e) = faults::clear_fault(db, &f.id, app)
            {
                tracing::warn!(app = %app, fault_id = %f.id, "failed to clear observe_failed fault: {e}");
            }
        }
    }

    fn file_instance_faults(
        db: &crate::runtime::db::Db,
        app: &AppName,
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

// spec: FLT#workload-faults
/// The database side of [`Reconciler::file_crash_loop_faults`], split out so
/// the filing and clearing policy can be exercised against a database without
/// standing up a reconciliation tick.
pub(super) fn apply_crash_loop_faults(
    db: &Db,
    app: &AppName,
    crash_loops: &[pods::CrashLoop],
    unit_healthy: &[ResourceInstance],
) {
    for crash_loop in crash_loops {
        let instance = &crash_loop.instance;
        let inst_hex = instance.id.to_hex();
        let kind_str = format!("{:?}", instance.kind).to_lowercase();
        let already_filed = faults::list_active_faults(db, Some(app))
            .unwrap_or_default()
            .iter()
            .any(|f| f.kind == "crash_loop" && f.instance_id.as_deref() == Some(&inst_hex));
        if !already_filed {
            // The description names which trigger fired: a rate-derived crash
            // loop and one systemd has already given up on need different
            // operator responses.
            let desc = match crash_loop.cause {
                pods::CrashLoopCause::RestartRate { count, window_secs } => format!(
                    "{} restarted {count} times in the last {} minutes. \
                     Auto-recovery is paused until this fault is cleared.",
                    instance.display_name,
                    window_secs / 60,
                ),
                pods::CrashLoopCause::StartLimitHit => format!(
                    "systemd hit start-limit for {}: too many restarts in window. \
                     Auto-recovery is paused until this fault is cleared.",
                    instance.display_name
                ),
            };
            if let Err(e) = faults::file_fault(
                db,
                app,
                Some(&kind_str),
                instance.name.as_deref(),
                Some(&inst_hex),
                "crash_loop",
                &desc,
            ) {
                tracing::warn!(app = %app, instance = %inst_hex, "failed to file crash_loop fault: {e}");
            }
        }
    }
    // Once the unit is back up healthy, clear any prior crash_loop fault —
    // operator clearing the fault and the unit recovering are both valid paths
    // out of this state.
    for instance in unit_healthy {
        let inst_hex = instance.id.to_hex();
        let cleared: Vec<_> = faults::list_active_faults(db, Some(app))
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.kind == "crash_loop" && f.instance_id.as_deref() == Some(&inst_hex))
            .collect();
        for f in cleared {
            if let Err(e) = faults::clear_fault(db, &f.id, app) {
                tracing::warn!(app = %app, fault_id = %f.id, "failed to clear crash_loop fault: {e}");
            }
        }
    }
}

#[cfg(test)]
mod crash_loop_tests;
