use super::*;
use defs::resource::ResourceKind;

// l[verify deployment.type]
// l[verify container.image]
// l[verify deployment.scale]
#[test]
fn deployment_with_image_and_scale() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .scale(3);
    "#,
    );
    let def = app.def.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert_eq!(dep_def.scale, 3..3);
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

// l[verify deployment.scale]
#[test]
fn deployment_scale_range() {
    let app = run_test_script_app(
        r#"
        app.deployment("workers")
            .scale(1..8);
    "#,
    );
    let def = app.def.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "workers")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert_eq!(dep_def.scale, 1..8);
    } else {
        panic!("expected Deployment");
    }
}

// l[verify deployment.on-update]
// l[verify const.on-update.replace]
#[test]
fn deployment_on_update_replace() {
    let app = run_test_script_app(
        r#"
        app.deployment("sync")
            .on_update(OnUpdate.Replace);
    "#,
    );
    let def = app.def.lock();
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

// l[verify deployment.on-terminate]
// l[verify const.on-terminate.recreate]
#[test]
fn deployment_on_terminate_recreate() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .on_terminate(OnTerminate.Recreate);
    "#,
    );
    let def = app.def.lock();
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

// l[verify deployment.scale.max-lower-bound]
#[test]
fn scale_fixed_rejects_above_10() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").scale(11);
    "#,
    );
}

// l[verify deployment.scale.max-lower-bound]
#[test]
fn scale_fixed_accepts_10() {
    let app = run_test_script_app(
        r#"
        app.deployment("web").scale(10);
    "#,
    );
    let def = app.def.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert_eq!(dep_def.scale, 10..10);
    } else {
        panic!("expected Deployment");
    }
}

// l[verify deployment.scale.max-lower-bound]
#[test]
fn scale_range_lower_bound_rejects_above_10() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").scale(11..20);
    "#,
    );
}

// l[verify deployment.scale.max-lower-bound]
#[test]
fn scale_range_lower_bound_accepts_10() {
    let app = run_test_script_app(
        r#"
        app.deployment("web").scale(10..20);
    "#,
    );
    let def = app.def.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert_eq!(dep_def.scale, 10..20);
    } else {
        panic!("expected Deployment");
    }
}

// l[verify deployment.scale]
#[test]
fn scale_fixed_rejects_zero() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").scale(0);
    "#,
    );
}

// l[verify deployment.scale]
#[test]
fn scale_fixed_rejects_negative() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").scale(-1);
    "#,
    );
}

// l[verify deployment.scale]
#[test]
fn scale_range_allows_zero_lower_bound() {
    let app = run_test_script_app(
        r#"
        app.deployment("web").scale(0..5);
    "#,
    );
    let def = app.def.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert_eq!(dep_def.scale, 0..5);
    } else {
        panic!("expected Deployment");
    }
}

// l[verify deployment.pod]
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
    let def = app.def.lock();
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
