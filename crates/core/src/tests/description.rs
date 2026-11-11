use super::*;
use defs::resource::{Resource, ResourceKind};

// spec: APP#app-description
#[test]
fn app_description_is_stored() {
    let app = run_test_script_app(r#"app.description("PostgreSQL database service");"#);
    let def = app.def.load();
    assert_eq!(
        def.description.as_deref(),
        Some("PostgreSQL database service")
    );
}

// spec: APP#app-description
#[test]
fn app_description_is_chainable_and_returns_app() {
    run_test_script_app(
        r#"
        let returned = app.description("first");
        let t = returned.type_of();
        if t != "App" { throw "expected App, got: " + t; }
        // Last call wins.
        app.description("second");
    "#,
    );
}

// spec: APP#app-description
#[test]
fn app_description_replaces_on_repeat() {
    let app = run_test_script_app(
        r#"
        app.description("first");
        app.description("second");
    "#,
    );
    assert_eq!(app.def.load().description.as_deref(), Some("second"));
}

// spec: BSL#descriptions
#[test]
fn description_is_stored_on_each_resource_kind() {
    let app = run_test_script_app(
        r#"
        app.service("svc").description("svc description");
        app.deployment("dep")
            .image("docker.io/library/nginx:1")
            .description("dep description");
        app.job("jbs")
            .image("docker.io/library/alpine:1")
            .description("jbs description");
        app.volume("vol").description("vol description");
        app.external_volume("evol").description("evol description");
        app.external_service("esvc").description("esvc description");
        // Ingress is created from a service.
        app.service("svc")
            .ingress("example.com", 443)
            .description("ing description");
    "#,
    );
    let def = app.def.load();
    let by_kind = |kind: ResourceKind, name: &str| -> Resource {
        def.resources
            .iter()
            .find(|(id, _)| id.kind == kind && id.name.as_str() == name)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| panic!("resource {kind:?} {name:?} not found"))
    };
    assert_eq!(
        by_kind(ResourceKind::Service, "svc")
            .description()
            .as_deref(),
        Some("svc description")
    );
    assert_eq!(
        by_kind(ResourceKind::Deployment, "dep")
            .description()
            .as_deref(),
        Some("dep description")
    );
    assert_eq!(
        by_kind(ResourceKind::Job, "jbs").description().as_deref(),
        Some("jbs description")
    );
    assert_eq!(
        by_kind(ResourceKind::Volume, "vol")
            .description()
            .as_deref(),
        Some("vol description")
    );
    assert_eq!(
        by_kind(ResourceKind::ExternalVolume, "evol")
            .description()
            .as_deref(),
        Some("evol description")
    );
    assert_eq!(
        by_kind(ResourceKind::ExternalService, "esvc")
            .description()
            .as_deref(),
        Some("esvc description")
    );
    assert_eq!(
        by_kind(ResourceKind::Ingress, "example.com:443")
            .description()
            .as_deref(),
        Some("ing description")
    );
}

// spec: BSL#descriptions
#[test]
fn description_defaults_to_none() {
    let app = run_test_script_app(r#"app.deployment("dep").image("docker.io/library/nginx:1");"#);
    let def = app.def.load();
    let resource = def
        .resources
        .iter()
        .find(|(id, _)| id.kind == ResourceKind::Deployment && id.name.as_str() == "dep")
        .map(|(_, r)| r.clone())
        .unwrap();
    assert!(resource.description().is_none());
}

// spec: BSL#descriptions
#[test]
fn description_replaces_on_repeat() {
    let app = run_test_script_app(
        r#"
        app.deployment("dep")
            .image("docker.io/library/nginx:1")
            .description("first")
            .description("second");
    "#,
    );
    let def = app.def.load();
    let resource = def
        .resources
        .iter()
        .find(|(id, _)| id.kind == ResourceKind::Deployment && id.name.as_str() == "dep")
        .map(|(_, r)| r.clone())
        .unwrap();
    assert_eq!(resource.description().as_deref(), Some("second"));
}

// spec: BSL#descriptions
#[test]
fn description_returns_self_for_chaining() {
    // Chaining: `.description("...")` must return the same resource type so
    // builder calls can keep flowing.
    let app = run_test_script_app(
        r#"
        app.deployment("dep")
            .image("docker.io/library/nginx:1")
            .description("a deployment")
            .scale(2);
    "#,
    );
    let def = app.def.load();
    let resource = def
        .resources
        .iter()
        .find(|(id, _)| id.kind == ResourceKind::Deployment && id.name.as_str() == "dep")
        .map(|(_, r)| r.clone())
        .unwrap();
    if let Resource::Deployment(d) = &resource {
        assert_eq!(d.def.lock().scale.start, 2);
        assert_eq!(d.def.lock().scale.end, 2);
        assert_eq!(d.def.lock().description.as_deref(), Some("a deployment"));
    } else {
        panic!("expected Deployment");
    }
}

// spec: BSL#descriptions
#[test]
fn description_is_serialised_in_resource_summary() {
    use serde_json::Value;
    let app = run_test_script_app(
        r#"
        app.job("backup")
            .image("docker.io/library/alpine:1")
            .description("nightly backup runner");
    "#,
    );
    let def = app.def.load();
    let resource = def
        .resources
        .iter()
        .find(|(id, _)| id.kind == ResourceKind::Job)
        .map(|(_, r)| r.clone())
        .unwrap();
    let summary = serde_json::to_value(resource.summary(&def)).unwrap();
    assert_eq!(
        summary.get("description").and_then(Value::as_str),
        Some("nightly backup runner")
    );
}

// spec: APP#canopy-type
#[test]
fn canopy_type_is_stored() {
    let app = run_test_script_app(r#"app.canopy_type("tamanu-central");"#);
    assert_eq!(
        app.def.load().canopy_type.as_deref(),
        Some("tamanu-central")
    );
}

// spec: APP#canopy-type
#[test]
fn canopy_type_last_call_wins_and_returns_app() {
    let app = run_test_script_app(
        r#"
        let returned = app.canopy_type("first");
        if returned.type_of() != "App" { throw "expected App"; }
        app.canopy_type("second").description("chained");
    "#,
    );
    assert_eq!(app.def.load().canopy_type.as_deref(), Some("second"));
}

// spec: APP#canopy-type
#[test]
fn untyped_app_has_no_canopy_type() {
    let app = run_test_script_app(r#"app.description("plain");"#);
    assert_eq!(app.def.load().canopy_type, None);
}

// spec: APP#canopy-type
#[test]
fn canopy_type_is_a_script_error_inside_an_action_closure() {
    let (engine, mut scope, app, _) = run_test_script(r#"app.description("x");"#);
    let _guard = crate::runtime::barrier::runtime::ActionClosureGuard::new(
        std::sync::Arc::clone(&app.def),
        "op".to_owned(),
        Default::default(),
    );
    let err = engine
        .run_with_scope(&mut scope, r#"app.canopy_type("tamanu");"#)
        .expect_err("canopy_type inside an action closure throws");
    assert!(err.to_string().contains("action closure"), "{err}");
    assert_eq!(app.def.load().canopy_type, None);
}

// spec: APP#canopy-type
#[test]
fn malformed_canopy_types_throw() {
    for bad in [
        "Tamanu",
        "tamanu-Central",
        "1tamanu",
        "-tamanu",
        "tamanu-",
        "tamanu--central",
        "tamanu_central",
        "tamanu central",
        "",
    ] {
        let err = run_test_script_err(&format!(r#"app.canopy_type("{bad}");"#));
        assert!(err.to_string().contains("canopy_type"), "{bad:?}: {err}");
    }
    let err = run_test_script_err(r#"app.canopy_type(42);"#);
    assert!(err.to_string().contains("must be a string"), "{err}");
}

// spec: APP#canopy-type
#[test]
fn well_formed_canopy_types_are_accepted() {
    for good in ["a", "tamanu", "tamanu-central", "a1", "tamanu2-v3-x"] {
        let app = run_test_script_app(&format!(r#"app.canopy_type("{good}");"#));
        assert_eq!(app.def.load().canopy_type.as_deref(), Some(good));
    }
}
