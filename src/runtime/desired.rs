use std::collections::HashMap;
use std::sync::Arc;

use crate::defs::app::AppDef;
use crate::defs::resource::{Resource, ResourceId};
use crate::runtime::barrier::{ActionLogEntry, CallKind};
use crate::runtime::identity::ResourceInstance;
use crate::runtime::lifecycle::LifecycleState;

// r[impl desired-state.definition]
#[derive(Debug)]
pub struct DesiredResource {
    pub instance: ResourceInstance,
    pub desired: LifecycleState,
    pub definition: Resource,
}

// r[impl desired-state.definition]
#[derive(Debug, Default)]
pub struct DesiredState {
    pub resources: Vec<DesiredResource>,
}

impl DesiredState {
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

/// Records the resources an in-progress lifecycle operation has placed into
/// the desired state so far, as directed by `rt.start()`, `rt.stop()`, and
/// `rt.reconcile()` calls in the action closure.
#[derive(Debug, Default)]
pub struct OperationProgress {
    resources: HashMap<ResourceInstance, LifecycleState>,
}

impl OperationProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a resource as explicitly started (desired state: `Ready`).
    pub fn started(&mut self, resource: ResourceInstance) {
        self.resources.insert(resource, LifecycleState::Ready);
    }

    /// Mark a resource as explicitly stopped (desired state: `Unscheduled`).
    pub fn stopped(&mut self, resource: ResourceInstance) {
        self.resources.insert(resource, LifecycleState::Unscheduled);
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// Build from a slice of action log entries.
    ///
    /// `Start` and `Reconcile` entries map to desired state `Ready`.
    /// `Stop` entries map to desired state `Unscheduled`.
    /// `Query` entries are ignored; they do not affect the desired state.
    ///
    /// Later entries for the same resource override earlier ones.
    pub fn from_log(entries: &[ActionLogEntry]) -> Self {
        let mut this = Self::new();
        for entry in entries {
            match entry.call_kind {
                CallKind::Start | CallKind::Reconcile => {
                    for r in &entry.resources {
                        this.started(r.clone());
                    }
                }
                CallKind::Stop => {
                    for r in &entry.resources {
                        this.stopped(r.clone());
                    }
                }
                CallKind::Query => {}
            }
        }
        this
    }
}

/// Compute the desired state for an application.
///
/// When `operation_progress` is `None` the full AppDef contributes all
/// resources at desired state `Ready` (steady-state maintenance).
///
/// When an operation is in progress, only resources the action closure has
/// explicitly placed into the desired state are included.
// r[impl desired-state.definition]
pub fn compute(
    app_name: &str,
    app_def: &AppDef,
    operation_progress: Option<&OperationProgress>,
) -> DesiredState {
    match operation_progress {
        None => compute_steady(app_name, app_def),
        Some(progress) => compute_during_operation(app_def, progress),
    }
}

/// Compute the desired state for an app that is being uninstalled.
/// All resources are desired at `Unscheduled`.
pub fn compute_uninstalling(app_name: &str, app_def: &AppDef) -> DesiredState {
    let resources = app_def
        .resources
        .iter()
        .map(|(id, resource)| DesiredResource {
            instance: ResourceInstance::new_singleton(app_name, id.kind, id.name.as_str()),
            desired: LifecycleState::Unscheduled,
            definition: resource.clone(),
        })
        .collect();
    DesiredState { resources }
}

// r[impl desired-state.steady]
fn compute_steady(app_name: &str, app_def: &AppDef) -> DesiredState {
    let resources = app_def
        .resources
        .iter()
        .map(|(id, resource)| DesiredResource {
            instance: ResourceInstance::new_singleton(app_name, id.kind, id.name.as_str()),
            desired: LifecycleState::Ready,
            definition: resource.clone(),
        })
        .collect();
    DesiredState { resources }
}

// r[impl desired-state.during-operation]
fn compute_during_operation(app_def: &AppDef, progress: &OperationProgress) -> DesiredState {
    let resources = progress
        .resources
        .iter()
        .filter_map(|(instance, &desired)| {
            let definition = lookup_definition(app_def, instance)?;
            Some(DesiredResource {
                instance: instance.clone(),
                desired,
                definition,
            })
        })
        .collect();
    DesiredState { resources }
}

fn lookup_definition(app_def: &AppDef, instance: &ResourceInstance) -> Option<Resource> {
    let name = Arc::new(instance.name.as_deref().unwrap_or("").to_owned());
    let id = ResourceId {
        kind: instance.kind,
        name,
    };
    app_def.resources.get(&id).cloned()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;
    use crate::defs::app::AppDef;
    use crate::defs::deployment::{Deployment, DeploymentDef};
    use crate::defs::resource::{Resource, ResourceId, ResourceKind};
    use crate::runtime::barrier::{ActionLogEntry, CallKind};
    use crate::runtime::identity::ResourceInstance;
    use crate::runtime::lifecycle::LifecycleState;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_deployment(name: &str) -> (ResourceId, Resource) {
        let rname = Arc::new(name.to_owned());
        let id = ResourceId {
            kind: ResourceKind::Deployment,
            name: rname.clone(),
        };
        let resource = Resource::Deployment(Deployment {
            name: rname,
            def: Arc::new(Mutex::new(DeploymentDef::default())),
        });
        (id, resource)
    }

    fn make_app_def(names: &[&str]) -> AppDef {
        let mut def = AppDef::default();
        for &name in names {
            let (id, resource) = make_deployment(name);
            def.resources.insert(id, resource);
        }
        def
    }

    fn dep(app: &str, name: &str) -> ResourceInstance {
        ResourceInstance::new_singleton(app, ResourceKind::Deployment, name)
    }

    fn log_entry(call_kind: CallKind, resources: Vec<ResourceInstance>) -> ActionLogEntry {
        ActionLogEntry {
            call_index: 0,
            call_kind,
            resources,
            barrier: None,
        }
    }

    /// Collect a `DesiredState` into a name → desired-state map for easy assertion.
    fn to_map(state: DesiredState) -> HashMap<String, LifecycleState> {
        state
            .resources
            .into_iter()
            .map(|r| (r.instance.name.unwrap_or_default(), r.desired))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Steady state (no operation)
    // -----------------------------------------------------------------------

    // r[verify desired-state.steady]
    #[test]
    fn steady_state_all_resources_are_ready() {
        let app_def = make_app_def(&["web", "api"]);
        let state = compute("myapp", &app_def, None);

        assert_eq!(state.resources.len(), 2);
        assert!(
            state
                .resources
                .iter()
                .all(|r| r.desired == LifecycleState::Ready)
        );
    }

    // r[verify desired-state.steady]
    #[test]
    fn steady_state_resource_names_match_app_def() {
        let app_def = make_app_def(&["web", "api"]);
        let state = compute("myapp", &app_def, None);

        let map = to_map(state);
        assert!(map.contains_key("web"));
        assert!(map.contains_key("api"));
    }

    // r[verify desired-state.steady]
    #[test]
    fn steady_state_instances_carry_app_name() {
        let app_def = make_app_def(&["web"]);
        let state = compute("myapp", &app_def, None);

        assert_eq!(state.resources[0].instance.app, "myapp");
    }

    // r[verify desired-state.steady]
    #[test]
    fn steady_state_empty_app_def_gives_empty_desired_state() {
        let app_def = AppDef::default();
        let state = compute("myapp", &app_def, None);
        assert!(state.is_empty());
    }

    // -----------------------------------------------------------------------
    // During operation
    // -----------------------------------------------------------------------

    // r[verify desired-state.during-operation]
    #[test]
    fn operation_with_no_starts_gives_empty_desired_state() {
        let app_def = make_app_def(&["web", "api"]);
        let progress = OperationProgress::new();
        let state = compute("myapp", &app_def, Some(&progress));
        assert!(state.is_empty());
    }

    // r[verify desired-state.during-operation]
    #[test]
    fn started_resource_is_desired_at_ready() {
        let app_def = make_app_def(&["web", "api"]);
        let mut progress = OperationProgress::new();
        progress.started(dep("myapp", "web"));

        let state = compute("myapp", &app_def, Some(&progress));

        assert_eq!(state.resources.len(), 1);
        assert_eq!(state.resources[0].desired, LifecycleState::Ready);
        assert_eq!(state.resources[0].instance.name.as_deref(), Some("web"));
    }

    // r[verify desired-state.during-operation]
    #[test]
    fn stopped_resource_is_desired_at_unscheduled() {
        let app_def = make_app_def(&["web"]);
        let mut progress = OperationProgress::new();
        progress.stopped(dep("myapp", "web"));

        let state = compute("myapp", &app_def, Some(&progress));

        assert_eq!(state.resources.len(), 1);
        assert_eq!(state.resources[0].desired, LifecycleState::Unscheduled);
    }

    // r[verify desired-state.during-operation]
    #[test]
    fn stop_after_start_overrides_to_unscheduled() {
        let app_def = make_app_def(&["web"]);
        let web = dep("myapp", "web");
        let mut progress = OperationProgress::new();
        progress.started(web.clone());
        progress.stopped(web);

        let state = compute("myapp", &app_def, Some(&progress));

        assert_eq!(state.resources.len(), 1);
        assert_eq!(state.resources[0].desired, LifecycleState::Unscheduled);
    }

    // r[verify desired-state.during-operation]
    #[test]
    fn started_resource_not_in_app_def_is_dropped() {
        let app_def = make_app_def(&["web"]);
        let mut progress = OperationProgress::new();
        progress.started(dep("myapp", "unknown"));

        let state = compute("myapp", &app_def, Some(&progress));

        assert!(state.is_empty());
    }

    // -----------------------------------------------------------------------
    // OperationProgress::from_log
    // -----------------------------------------------------------------------

    // r[verify desired-state.during-operation]
    #[test]
    fn from_log_start_entry_maps_to_ready() {
        let app_def = make_app_def(&["web"]);
        let entries = [log_entry(CallKind::Start, vec![dep("myapp", "web")])];
        let progress = OperationProgress::from_log(&entries);

        let state = compute("myapp", &app_def, Some(&progress));

        let map = to_map(state);
        assert_eq!(map["web"], LifecycleState::Ready);
    }

    // r[verify desired-state.during-operation]
    #[test]
    fn from_log_stop_entry_maps_to_unscheduled() {
        let app_def = make_app_def(&["web"]);
        let entries = [log_entry(CallKind::Stop, vec![dep("myapp", "web")])];
        let progress = OperationProgress::from_log(&entries);

        let state = compute("myapp", &app_def, Some(&progress));

        let map = to_map(state);
        assert_eq!(map["web"], LifecycleState::Unscheduled);
    }

    // r[verify desired-state.during-operation]
    #[test]
    fn from_log_reconcile_entry_maps_to_ready() {
        let app_def = make_app_def(&["web"]);
        let entries = [log_entry(CallKind::Reconcile, vec![dep("myapp", "web")])];
        let progress = OperationProgress::from_log(&entries);

        let state = compute("myapp", &app_def, Some(&progress));

        let map = to_map(state);
        assert_eq!(map["web"], LifecycleState::Ready);
    }

    // r[verify desired-state.during-operation]
    #[test]
    fn from_log_query_entry_is_ignored() {
        let entries = [log_entry(CallKind::Query, vec![dep("myapp", "web")])];
        let progress = OperationProgress::from_log(&entries);
        assert!(progress.is_empty());
    }

    // r[verify desired-state.during-operation]
    #[test]
    fn from_log_later_entry_overrides_earlier_for_same_resource() {
        let app_def = make_app_def(&["web"]);
        let web = dep("myapp", "web");
        let entries = [
            log_entry(CallKind::Start, vec![web.clone()]),
            log_entry(CallKind::Stop, vec![web]),
        ];
        let progress = OperationProgress::from_log(&entries);

        let state = compute("myapp", &app_def, Some(&progress));

        let map = to_map(state);
        assert_eq!(map["web"], LifecycleState::Unscheduled);
    }

    // r[verify desired-state.during-operation]
    #[test]
    fn from_log_multiple_resources_in_one_entry() {
        let app_def = make_app_def(&["web", "api"]);
        let entries = [log_entry(
            CallKind::Start,
            vec![dep("myapp", "web"), dep("myapp", "api")],
        )];
        let progress = OperationProgress::from_log(&entries);

        let state = compute("myapp", &app_def, Some(&progress));

        assert_eq!(state.resources.len(), 2);
        assert!(
            state
                .resources
                .iter()
                .all(|r| r.desired == LifecycleState::Ready)
        );
    }

    // r[verify desired-state.definition]
    #[test]
    fn definition_field_is_populated_from_app_def() {
        let app_def = make_app_def(&["web"]);
        let state = compute("myapp", &app_def, None);

        assert_eq!(state.resources.len(), 1);
        assert!(matches!(
            state.resources[0].definition,
            Resource::Deployment(_)
        ));
    }
}
