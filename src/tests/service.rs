use super::*;
use defs::resource::ResourceKind;

// l[verify service.type]
#[test]
fn service_creates_resource() {
    let app = run_test_script_app(r#"let s = app.service("web");"#);
    let def = app.def.lock();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Service && &*id.name == "web")
    );
}

// l[verify service.port]
#[test]
fn service_port_creates_service_port() {
    run_test_script_app(
        r#"
        let svc = app.service("web");
        let sp = svc.port(8080);
    "#,
    );
}

// l[verify service.port]
#[test]
fn service_port_rejects_invalid() {
    let _ = run_test_script_err(r#"app.service("web").port(0);"#);
    let _ = run_test_script_err(r#"app.service("web").port(65536);"#);
}

// l[verify service.routing]
#[test]
fn service_accepts_tcp_and_udp_routing() {
    run_test_script_app(
        r#"
        let svc = app.service("web");
        app.deployment("web")
            .tcp(8080, svc)
            .udp(9090, svc);
    "#,
    );
}

// l[verify service.http]
#[test]
fn service_http_specialisation() {
    let app = run_test_script_app(
        r#"
        let h = app.service("api").http(8080);
    "#,
    );
    let def = app.def.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Service && &*id.name == "api")
        .unwrap();
    if let defs::resource::Resource::Service(svc) = &def.resources[id] {
        assert!(svc.def.lock().http.is_some());
    } else {
        panic!("expected Service");
    }
}

// l[verify service.http]
#[test]
fn service_http_default_port_80() {
    run_test_script_app(
        r#"
        let h = app.service("web").http();
    "#,
    );
}

// l[verify service.http.route]
#[test]
fn http_service_route() {
    run_test_script_app(
        r#"
        let h = app.service("web").http(80);
        let r = h.route("/api");
    "#,
    );
}

// l[verify service.http.route]
#[test]
fn http_service_route_rejects_empty() {
    let _ = run_test_script_err(
        r#"
        let h = app.service("web").http(80);
        h.route("");
    "#,
    );
}

// l[verify service.http.route]
#[test]
fn http_service_route_rejects_no_slash() {
    let _ = run_test_script_err(
        r#"
        let h = app.service("web").http(80);
        h.route("api");
    "#,
    );
}

// l[verify service.external]
#[test]
fn external_service_creates_resource() {
    let app = run_test_script_app(
        r#"
        let s = app.external_service("redis");
    "#,
    );
    let def = app.def.lock();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::ExternalService && &*id.name == "redis")
    );
}

// l[verify service.external.port]
#[test]
fn external_service_port() {
    run_test_script_app(
        r#"
        let es = app.external_service("redis");
        let sp = es.port(6379);
    "#,
    );
}

// l[verify service.external.port]
#[test]
fn external_service_port_rejects_invalid() {
    let _ = run_test_script_err(r#"app.external_service("redis").port(0);"#);
}
