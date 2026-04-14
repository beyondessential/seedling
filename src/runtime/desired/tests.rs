use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use super::*;
use crate::defs::app::AppDef;
use crate::defs::deployment::{Deployment, DeploymentDef};
use crate::defs::resource::{Resource, ResourceId, ResourceKind};
use crate::runtime::barrier::{ActionLogEntry, CallKind};
use crate::runtime::identity::{InstanceVariant, ResourceInstance};
use crate::runtime::lifecycle::LifecycleState;
use crate::runtime::registry::EphemeralInstanceRegistry;

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

fn make_deployment(name: &str) -> (ResourceId, Resource) {
    make_deployment_with_scale(name, 1..1)
}

fn make_deployment_with_scale(name: &str, scale: std::ops::Range<u16>) -> (ResourceId, Resource) {
    let rname = Arc::new(name.to_owned());
    let id = ResourceId {
        kind: ResourceKind::Deployment,
        name: rname.clone(),
    };
    let def = DeploymentDef {
        scale,
        ..Default::default()
    };
    let resource = Resource::Deployment(Deployment {
        name: rname,
        def: Arc::new(Mutex::new(def)),
        frozen: false,
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

fn make_app_def_scaled(entries: &[(&str, std::ops::Range<u16>)]) -> AppDef {
    let mut def = AppDef::default();
    for &(name, ref scale) in entries {
        let (id, resource) = make_deployment_with_scale(name, scale.clone());
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

/// Build an EffectiveScales map from an AppDef, using each deployment's lower
/// bound as the effective scale (i.e. no stored decision).
fn default_effective_scales(app_def: &AppDef) -> EffectiveScales {
    let mut scales = EffectiveScales::new();
    for (id, resource) in &app_def.resources {
        if let Resource::Deployment(deployment) = resource {
            let dep_def = deployment.def.lock();
            let low = dep_def.scale.start;
            let high = dep_def.scale.end;
            scales.insert(id.name.as_str().to_owned(), (low, high, low));
        }
    }
    scales
}

/// Build an EffectiveScales map with a specific effective value per deployment.
fn custom_effective_scales(entries: &[(&str, u16, u16, u16)]) -> EffectiveScales {
    entries
        .iter()
        .map(|&(name, low, high, effective)| (name.to_owned(), (low, high, effective)))
        .collect()
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
    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, None, &registry, &scales).unwrap();

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
    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, None, &registry, &scales).unwrap();

    let map = to_map(state);
    assert!(map.contains_key("web"));
    assert!(map.contains_key("api"));
}

// r[verify desired-state.steady]
#[test]
fn steady_state_instances_carry_app_name() {
    let app_def = make_app_def(&["web"]);
    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, None, &registry, &scales).unwrap();

    assert_eq!(state.resources[0].instance.app, "myapp");
}

// r[verify desired-state.steady]
#[test]
fn steady_state_empty_app_def_gives_empty_desired_state() {
    let app_def = AppDef::default();
    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, None, &registry, &scales).unwrap();
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
    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, Some(&progress), &registry, &scales).unwrap();
    assert!(state.is_empty());
}

// r[verify desired-state.during-operation]
#[test]
fn started_resource_is_desired_at_ready() {
    let app_def = make_app_def(&["web", "api"]);
    let mut progress = OperationProgress::new();
    progress.started(dep("myapp", "web"));

    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, Some(&progress), &registry, &scales).unwrap();

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

    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, Some(&progress), &registry, &scales).unwrap();

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

    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, Some(&progress), &registry, &scales).unwrap();

    assert_eq!(state.resources.len(), 1);
    assert_eq!(state.resources[0].desired, LifecycleState::Unscheduled);
}

// r[verify desired-state.during-operation]
#[test]
fn started_resource_not_in_app_def_is_dropped() {
    let app_def = make_app_def(&["web"]);
    let mut progress = OperationProgress::new();
    progress.started(dep("myapp", "unknown"));

    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, Some(&progress), &registry, &scales).unwrap();

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

    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, Some(&progress), &registry, &scales).unwrap();

    let map = to_map(state);
    assert_eq!(map["web"], LifecycleState::Ready);
}

// r[verify desired-state.during-operation]
#[test]
fn from_log_stop_entry_maps_to_unscheduled() {
    let app_def = make_app_def(&["web"]);
    let entries = [log_entry(CallKind::Stop, vec![dep("myapp", "web")])];
    let progress = OperationProgress::from_log(&entries);

    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, Some(&progress), &registry, &scales).unwrap();

    let map = to_map(state);
    assert_eq!(map["web"], LifecycleState::Unscheduled);
}

// r[verify desired-state.during-operation]
#[test]
fn from_log_reconcile_entry_maps_to_ready() {
    let app_def = make_app_def(&["web"]);
    let entries = [log_entry(CallKind::Reconcile, vec![dep("myapp", "web")])];
    let progress = OperationProgress::from_log(&entries);

    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, Some(&progress), &registry, &scales).unwrap();

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

    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, Some(&progress), &registry, &scales).unwrap();

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

    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, Some(&progress), &registry, &scales).unwrap();

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
    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, None, &registry, &scales).unwrap();

    assert_eq!(state.resources.len(), 1);
    assert!(matches!(
        state.resources[0].definition,
        Resource::Deployment(_)
    ));
}

// -----------------------------------------------------------------------
// Scaling
// -----------------------------------------------------------------------

// r[verify autonomous.scale]
#[test]
fn scaled_deployment_produces_multiple_instances() {
    let app_def = make_app_def_scaled(&[("web", 1..3)]);
    let registry = EphemeralInstanceRegistry::new();
    let scales = custom_effective_scales(&[("web", 1, 3, 3)]);
    let state = compute("myapp", &app_def, None, &registry, &scales).unwrap();

    let ready: Vec<_> = state
        .resources
        .iter()
        .filter(|r| r.desired == LifecycleState::Ready)
        .collect();
    assert_eq!(ready.len(), 3);
    assert!(
        ready
            .iter()
            .all(|r| r.instance.variant == InstanceVariant::Scaled),
        "all instances of a scalable deployment should be Scaled"
    );
}

// r[verify autonomous.scale]
#[test]
fn scaled_deployment_effective_less_than_existing_marks_excess_unscheduled() {
    let app_def = make_app_def_scaled(&[("web", 1..5)]);
    let registry = EphemeralInstanceRegistry::new();

    // First, create 4 instances.
    let scales_4 = custom_effective_scales(&[("web", 1, 5, 4)]);
    let _ = compute("myapp", &app_def, None, &registry, &scales_4).unwrap();

    // Now scale down to 2.
    let scales_2 = custom_effective_scales(&[("web", 1, 5, 2)]);
    let state = compute("myapp", &app_def, None, &registry, &scales_2).unwrap();

    let ready: Vec<_> = state
        .resources
        .iter()
        .filter(|r| r.desired == LifecycleState::Ready)
        .collect();
    let unscheduled: Vec<_> = state
        .resources
        .iter()
        .filter(|r| r.desired == LifecycleState::Unscheduled)
        .collect();
    assert_eq!(ready.len(), 2, "should keep 2 instances");
    assert_eq!(unscheduled.len(), 2, "should mark 2 excess as Unscheduled");
}

// r[verify autonomous.scale]
#[test]
fn fixed_scale_one_uses_singleton() {
    let app_def = make_app_def(&["web"]);
    let registry = EphemeralInstanceRegistry::new();
    let scales = default_effective_scales(&app_def);
    let state = compute("myapp", &app_def, None, &registry, &scales).unwrap();

    assert_eq!(state.resources.len(), 1);
    assert_eq!(
        state.resources[0].instance.variant,
        InstanceVariant::Singleton,
        "scale(1) should produce a singleton"
    );
}

// r[verify autonomous.scale]
#[test]
fn fixed_scale_two_uses_scaled_instances() {
    let app_def = make_app_def_scaled(&[("web", 2..2)]);
    let registry = EphemeralInstanceRegistry::new();
    let scales = custom_effective_scales(&[("web", 2, 2, 2)]);
    let state = compute("myapp", &app_def, None, &registry, &scales).unwrap();

    assert_eq!(state.resources.len(), 2);
    assert!(
        state
            .resources
            .iter()
            .all(|r| r.instance.variant == InstanceVariant::Scaled),
        "scale(2) should produce scaled instances"
    );
}

// r[verify autonomous.scale]
#[test]
fn scale_zero_lower_bound_starts_with_zero_instances() {
    let app_def = make_app_def_scaled(&[("web", 0..3)]);
    let registry = EphemeralInstanceRegistry::new();
    let scales = custom_effective_scales(&[("web", 0, 3, 0)]);
    let state = compute("myapp", &app_def, None, &registry, &scales).unwrap();

    let ready: Vec<_> = state
        .resources
        .iter()
        .filter(|r| r.desired == LifecycleState::Ready)
        .collect();
    assert_eq!(
        ready.len(),
        0,
        "effective scale 0 should produce no Ready instances"
    );
}

// r[verify autonomous.scale]
#[test]
fn scaled_instances_have_distinct_ids() {
    let app_def = make_app_def_scaled(&[("web", 1..5)]);
    let registry = EphemeralInstanceRegistry::new();
    let scales = custom_effective_scales(&[("web", 1, 5, 3)]);
    let state = compute("myapp", &app_def, None, &registry, &scales).unwrap();

    let ids: Vec<_> = state.resources.iter().map(|r| r.instance.id).collect();
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(ids.len(), unique.len(), "all instance IDs must be distinct");
}

// r[verify autonomous.scale]
#[test]
fn scaled_instances_are_stable_across_recomputes() {
    let app_def = make_app_def_scaled(&[("web", 1..5)]);
    let registry = EphemeralInstanceRegistry::new();
    let scales = custom_effective_scales(&[("web", 1, 5, 3)]);

    let state1 = compute("myapp", &app_def, None, &registry, &scales).unwrap();
    let ids1: Vec<_> = state1
        .resources
        .iter()
        .filter(|r| r.desired == LifecycleState::Ready)
        .map(|r| r.instance.id)
        .collect();

    let state2 = compute("myapp", &app_def, None, &registry, &scales).unwrap();
    let ids2: Vec<_> = state2
        .resources
        .iter()
        .filter(|r| r.desired == LifecycleState::Ready)
        .map(|r| r.instance.id)
        .collect();

    assert_eq!(
        ids1, ids2,
        "instance IDs should be stable across recomputes"
    );
}

// r[verify autonomous.scale]
#[test]
fn scale_up_preserves_existing_instances() {
    let app_def = make_app_def_scaled(&[("web", 1..5)]);
    let registry = EphemeralInstanceRegistry::new();

    // Start with 2.
    let scales_2 = custom_effective_scales(&[("web", 1, 5, 2)]);
    let state1 = compute("myapp", &app_def, None, &registry, &scales_2).unwrap();
    let ids_2: Vec<_> = state1
        .resources
        .iter()
        .filter(|r| r.desired == LifecycleState::Ready)
        .map(|r| r.instance.id)
        .collect();
    assert_eq!(ids_2.len(), 2);

    // Scale up to 4.
    let scales_4 = custom_effective_scales(&[("web", 1, 5, 4)]);
    let state2 = compute("myapp", &app_def, None, &registry, &scales_4).unwrap();
    let ids_4: Vec<_> = state2
        .resources
        .iter()
        .filter(|r| r.desired == LifecycleState::Ready)
        .map(|r| r.instance.id)
        .collect();
    assert_eq!(ids_4.len(), 4);

    // The original 2 IDs should still be present.
    for id in &ids_2 {
        assert!(
            ids_4.contains(id),
            "original instance {id:?} should be preserved on scale-up"
        );
    }
}

// r[verify autonomous.scale]
#[test]
fn mixed_singleton_and_scaled_deployments() {
    let mut def = AppDef::default();
    let (id1, res1) = make_deployment("singleton-dep");
    let (id2, res2) = make_deployment_with_scale("scaled-dep", 1..3);
    def.resources.insert(id1, res1);
    def.resources.insert(id2, res2);

    let registry = EphemeralInstanceRegistry::new();
    let scales = custom_effective_scales(&[("singleton-dep", 1, 1, 1), ("scaled-dep", 1, 3, 2)]);
    let state = compute("myapp", &def, None, &registry, &scales).unwrap();

    let singletons: Vec<_> = state
        .resources
        .iter()
        .filter(|r| r.instance.name.as_deref() == Some("singleton-dep"))
        .collect();
    let scaled: Vec<_> = state
        .resources
        .iter()
        .filter(|r| r.instance.name.as_deref() == Some("scaled-dep"))
        .collect();

    assert_eq!(singletons.len(), 1);
    assert_eq!(singletons[0].instance.variant, InstanceVariant::Singleton);

    assert_eq!(scaled.len(), 2);
    assert!(
        scaled
            .iter()
            .all(|r| r.instance.variant == InstanceVariant::Scaled)
    );
}

// r[verify autonomous.scale]
#[test]
fn uninstall_tears_down_all_scaled_instances() {
    let app_def = make_app_def_scaled(&[("web", 1..5)]);
    let registry = EphemeralInstanceRegistry::new();

    // Create 3 scaled instances via steady-state compute.
    let scales = custom_effective_scales(&[("web", 1, 5, 3)]);
    let _ = compute("myapp", &app_def, None, &registry, &scales).unwrap();

    // Now compute uninstalling — all 3 should be Unscheduled.
    let state = compute_uninstalling("myapp", &app_def, &registry).unwrap();
    assert_eq!(state.resources.len(), 3);
    assert!(
        state
            .resources
            .iter()
            .all(|r| r.desired == LifecycleState::Unscheduled)
    );
}
