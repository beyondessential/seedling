use super::*;
use defs::resource::ResourceKind;

// spec: DEP#declaration
// spec: CTR#image
// spec: DEP#scale
#[test]
fn deployment_with_image_and_scale() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .scale(3);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert_eq!(dep_def.scale, 3..=3);
        let pod = dep_def.pod.lock();
        let container = pod.container.lock();
        assert_eq!(
            container.image.as_deref(),
            Some("docker.io/library/nginx:latest")
        );
    } else {
        panic!("expected Deployment");
    }
}

fn scale_of(scale: &str) -> std::ops::RangeInclusive<u16> {
    let app = run_test_script_app(&format!(r#"app.deployment("workers").scale({scale});"#));
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "workers")
        .unwrap();
    match &def.resources[id] {
        defs::resource::Resource::Deployment(dep) => dep.def.lock().scale.clone(),
        _ => panic!("expected Deployment"),
    }
}

// spec: DEP#scale
#[test]
fn an_exclusive_scale_range_excludes_its_end() {
    assert_eq!(scale_of("1..8"), 1..=7);
}

// spec: DEP#scale
#[test]
fn an_inclusive_scale_range_includes_its_end() {
    assert_eq!(scale_of("1..=8"), 1..=8);
}

// spec: DEP#scale
#[test]
fn a_fixed_scale_is_the_inclusive_range_of_itself() {
    assert_eq!(scale_of("4"), scale_of("4..=4"));
}

// spec: DEP#scale
#[test]
fn an_exclusive_scale_range_holding_no_value_throws() {
    let err = run_test_script_err(r#"app.deployment("web").scale(5..5);"#);
    assert!(err.to_string().contains("holds no instance count"), "{err}");
    let _ = run_test_script_err(r#"app.deployment("web").scale(0..1);"#);
}

// spec: DEP#update-and-termination-strategies
// spec: CONST#onupdate
#[test]
fn deployment_on_update_replace() {
    let app = run_test_script_app(
        r#"
        app.deployment("sync")
            .on_update(OnUpdate.Replace);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "sync")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert!(matches!(dep_def.on_update, defs::enums::OnUpdate::Replace));
    } else {
        panic!("expected Deployment");
    }
}

// spec: DEP#update-and-termination-strategies
// spec: CONST#onterminate
#[test]
fn deployment_on_terminate_recreate() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .on_terminate(OnTerminate.Recreate);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert!(matches!(
            dep_def.on_terminate,
            defs::enums::OnTerminate::Recreate
        ));
    } else {
        panic!("expected Deployment");
    }
}

// spec: DEP#scale
#[test]
fn scale_fixed_rejects_above_10() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").scale(11);
    "#,
    );
}

// spec: DEP#scale
#[test]
fn scale_fixed_accepts_10() {
    let app = run_test_script_app(
        r#"
        app.deployment("web").scale(10);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert_eq!(dep_def.scale, 10..=10);
    } else {
        panic!("expected Deployment");
    }
}

// spec: DEP#scale
#[test]
fn scale_range_lower_bound_rejects_above_10() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").scale(11..20);
    "#,
    );
}

// spec: DEP#scale
#[test]
fn scale_range_lower_bound_accepts_10() {
    let app = run_test_script_app(
        r#"
        app.deployment("web").scale(10..=20);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert_eq!(dep_def.scale, 10..=20);
    } else {
        panic!("expected Deployment");
    }
}

// spec: DEP#scale
#[test]
fn scale_fixed_rejects_zero() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").scale(0);
    "#,
    );
}

// spec: DEP#scale
#[test]
fn scale_fixed_rejects_negative() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").scale(-1);
    "#,
    );
}

// spec: DEP#scale
#[test]
fn scale_range_allows_zero_lower_bound() {
    let app = run_test_script_app(
        r#"
        app.deployment("web").scale(0..=5);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert_eq!(dep_def.scale, 0..=5);
    } else {
        panic!("expected Deployment");
    }
}

// spec: DEP#declaration
#[test]
fn deployment_implements_pod_interface() {
    let app = run_test_script_app(
        r#"
        let svc = app.service("ctrl");
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .command("nginx")
            .tcp(8080, svc)
            .env("PORT", "8080");
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        let pod = dep_def.pod.lock();
        assert_eq!(pod.tcp_bindings.len(), 1);
        let container = pod.container.lock();
        assert_eq!(
            container.image.as_deref(),
            Some("docker.io/library/nginx:latest")
        );
        assert_eq!(container.env.len(), 1);
    } else {
        panic!("expected Deployment");
    }
}

// spec: BSL#arguments
// spec: DEP#scale
// An inverted range is an empty one, which holds no instance count.
#[test]
fn scale_rejects_an_inverted_range() {
    let err = run_test_script_err(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:1.29")
            .scale(5..2);
    "#,
    );
    let message = err.to_string();
    assert!(message.contains("must not exceed"), "{message}");
    let err = run_test_script_err(r#"app.deployment("web").scale(5..=2);"#);
    assert!(err.to_string().contains("must not exceed"), "{err}");
}

// spec: CTR#privileges
#[test]
fn a_user_spec_with_surrounding_whitespace_throws() {
    for user in [" 1000", "1000 ", "1000:1000\t"] {
        let err = run_test_script_err(&format!(
            r#"app.deployment("web").image("docker.io/library/nginx:1").user("{user}");"#
        ));
        assert!(err.to_string().contains("whitespace"), "{user:?}: {err}");
    }
}

// spec: CTR#image
#[test]
fn a_declared_container_with_no_image_fails_evaluation() {
    let (_, err) = crate::runtime::apps::evaluate_script(
        &seedling_protocol::names::AppName::new_unchecked("demo"),
        r#"app.deployment("web").image("docker.io/library/nginx:1"); app.job("migrate");"#,
        &Default::default(),
        &crate::ScriptLimits::default(),
    );
    let err = err.expect("the definition fails to evaluate");
    assert!(err.0.contains("job 'migrate'"), "{err}");
}
