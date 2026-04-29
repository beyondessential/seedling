use super::*;
use defs::resource::{Resource, ResourceKind};

// l[verify app.description]
#[test]
fn app_description_is_stored() {
    let app = run_test_script_app(r#"app.description("PostgreSQL database service");"#);
    let def = app.def.load();
    assert_eq!(
        def.description.as_deref(),
        Some("PostgreSQL database service")
    );
}

// l[verify app.description]
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

// l[verify app.description]
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

// l[verify bsl.resource.description]
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

// l[verify bsl.resource.description]
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

// l[verify bsl.resource.description]
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

// l[verify bsl.resource.description]
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

// Each shipped sample app must parse and load any required params it declares
// (a few of these set `app.description(...)`, which is what we want to
// exercise here on top of regular parsing).
#[test]
fn shipped_seed_apps_parse() {
    use seedling_protocol::names::AppName;
    let limits = crate::ScriptLimits::default();
    let app_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("apps");
    let entries =
        std::fs::read_dir(&app_dir).expect("apps directory should exist next to crates/core");
    let mut required_seed_params = std::collections::BTreeMap::new();
    required_seed_params.insert("password".to_string(), "x".to_string());
    required_seed_params.insert("bucket".to_string(), "x".to_string());
    required_seed_params.insert("access-key".to_string(), "x".to_string());
    required_seed_params.insert("secret-key".to_string(), "x".to_string());
    required_seed_params.insert("hostname".to_string(), "x".to_string());
    required_seed_params.insert("passphrase".to_string(), "x".to_string());
    required_seed_params.insert("version".to_string(), "18.0".to_string());
    required_seed_params.insert("public-hostname".to_string(), "example.com".to_string());
    required_seed_params.insert(
        "central-url".to_string(),
        "https://central.example.com".to_string(),
    );
    required_seed_params.insert("facility-id".to_string(), "x".to_string());
    required_seed_params.insert("sync-password".to_string(), "x".to_string());
    required_seed_params.insert("auth-secret".to_string(), "x".to_string());
    required_seed_params.insert("refresh-secret".to_string(), "x".to_string());

    let mut count = 0;
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if !path.to_string_lossy().ends_with(".seed.rhai") {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap();
        let app_name = AppName::new_unchecked("seed-app");
        let (_app, err) =
            crate::runtime::apps::evaluate_script(&app_name, &body, &required_seed_params, &limits);
        assert!(
            err.is_none(),
            "{} failed to parse: {:?}",
            path.display(),
            err
        );
        count += 1;
    }
    assert!(count > 0, "expected at least one .seed.rhai under apps/");
}

// l[verify bsl.resource.description]
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
    let summary = serde_json::to_value(resource.summary()).unwrap();
    assert_eq!(
        summary.get("description").and_then(Value::as_str),
        Some("nightly backup runner")
    );
}
