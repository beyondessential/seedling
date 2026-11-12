use super::*;
use defs::resource::ResourceKind;

// spec: POD#pod-interface
#[test]
fn pod_is_an_interface_on_deployment_and_job() {
    run_test_script_app(
        r#"
        let svc = app.service("ctrl");
        let sp = svc.port(8080);

        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .mount(sp)
            .tcp(8080, svc)
            .http(3000, svc.http(80));

        app.job("task")
            .image("docker.io/library/tools:latest")
            .mount(sp)
            .tcp(9090, svc);
    "#,
    );
}

// spec: POD#service-mounts
#[test]
fn pod_mount_serviceport() {
    let app = run_test_script_app(
        r#"
        let svc = app.service("ctrl");
        let sp = svc.port(5432);
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .mount(sp);
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
        assert_eq!(pod.service_mounts.len(), 1);
    } else {
        panic!("expected Deployment");
    }
}

// spec: POD#service-mounts
#[test]
fn mounting_a_second_service_port_on_the_same_number_throws_and_keeps_the_first() {
    let err = run_test_script_err(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .mount(app.service("primary").port(5432))
            .mount(app.service("replica").port(5432));
    "#,
    );
    assert!(
        err.to_string().contains("already mounts port 5432"),
        "{err}"
    );

    let app = run_test_script_app(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        web.mount(app.service("primary").port(5432));
        try { web.mount(app.service("replica").port(5432)); } catch {}
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    let defs::resource::Resource::Deployment(dep) = &def.resources[id] else {
        panic!("expected Deployment");
    };
    let dep_def = dep.def.lock();
    let pod = dep_def.pod.lock();
    assert_eq!(pod.service_mounts.len(), 1);
    assert_eq!(pod.service_mounts[0].service.name().as_str(), "primary");
}

// spec: POD#port-bindings
// spec: HTTP#routes
#[test]
fn pod_http_route_binding() {
    let app = run_test_script_app(
        r#"
        let traffic = app.service("public").http(80);
        app.deployment("api")
            .http(3000, traffic.route("/api"));
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "api")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        let pod = dep_def.pod.lock();
        assert_eq!(pod.http_bindings.len(), 1);
        assert_eq!(pod.http_bindings[0].pod_port, 3000);
        assert_eq!(pod.http_bindings[0].route.prefix, "/api");
    } else {
        panic!("expected Deployment");
    }
}

// spec: POD#port-bindings
#[test]
fn pod_http_service_defaults_to_root_route() {
    let app = run_test_script_app(
        r#"
        let traffic = app.service("public").http(80);
        app.deployment("web")
            .http(3000, traffic);
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
        assert_eq!(pod.http_bindings.len(), 1);
        assert_eq!(pod.http_bindings[0].route.prefix, "/");
    } else {
        panic!("expected Deployment");
    }
}

// spec: POD#port-bindings
#[test]
fn pod_tcp_binding() {
    let app = run_test_script_app(
        r#"
        let svc = app.service("ctrl");
        app.deployment("web")
            .tcp(8080, svc);
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
        assert_eq!(pod.tcp_bindings[0].pod_port, 8080);
    } else {
        panic!("expected Deployment");
    }
}

// spec: POD#port-bindings
#[test]
fn pod_tcp_with_service_port() {
    let app = run_test_script_app(
        r#"
        let svc = app.service("ctrl");
        let sp = svc.port(5432);
        app.deployment("web")
            .tcp(5432, sp);
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
        assert_eq!(pod.tcp_bindings[0].pod_port, 5432);
    } else {
        panic!("expected Deployment");
    }
}

// spec: POD#port-bindings
#[test]
fn pod_udp_binding_with_service() {
    let app = run_test_script_app(
        r#"
        let svc = app.service("dns");
        app.deployment("resolver")
            .udp(53, svc);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "resolver")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        let pod = dep_def.pod.lock();
        assert_eq!(pod.udp_bindings.len(), 1);
        assert_eq!(pod.udp_bindings[0].pod_port, 53);
    } else {
        panic!("expected Deployment");
    }
}

// spec: POD#port-bindings
#[test]
fn pod_udp_binding_with_service_port() {
    let app = run_test_script_app(
        r#"
        let svc = app.service("dns");
        let sp = svc.port(53);
        app.deployment("resolver")
            .udp(53, sp);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "resolver")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        let pod = dep_def.pod.lock();
        assert_eq!(pod.udp_bindings.len(), 1);
        assert_eq!(pod.udp_bindings[0].pod_port, 53);
    } else {
        panic!("expected Deployment");
    }
}
