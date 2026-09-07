use super::*;
use defs::resource::ResourceKind;

// l[verify service.type]
#[test]
fn service_creates_resource() {
    let app = run_test_script_app(r#"let s = app.service("web");"#);
    let def = app.def.load();
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
    let def = app.def.load();
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

// l[verify service.exported]
#[test]
fn service_exported_marks_service() {
    let app = run_test_script_app(r#"app.service("web").exported();"#);
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Service && &*id.name == "web")
        .unwrap();
    let defs::resource::Resource::Service(svc) = &def.resources[id] else {
        panic!("expected Service");
    };
    let svc_def = svc.def.lock();
    let opts = svc_def
        .exported
        .as_ref()
        .expect("service should be exported");
    assert!(opts.description.is_none());
}

// l[verify service.exported]
#[test]
fn service_exported_with_description() {
    let app =
        run_test_script_app(r#"app.service("api").exported(#{ description: "main HTTP API" });"#);
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Service && &*id.name == "api")
        .unwrap();
    let defs::resource::Resource::Service(svc) = &def.resources[id] else {
        panic!("expected Service");
    };
    let svc_def = svc.def.lock();
    let desc = svc_def
        .exported
        .as_ref()
        .and_then(|o| o.description.as_deref())
        .unwrap();
    assert_eq!(desc, "main HTTP API");
}

// l[verify service.external]
#[test]
fn external_service_creates_resource() {
    let app = run_test_script_app(r#"let s = app.external_service("api");"#);
    let def = app.def.load();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::ExternalService && &*id.name == "api")
    );
}

// l[verify service.external]
#[test]
fn external_service_rejects_invalid_name() {
    let _ = run_test_script_err(r#"app.external_service("_bad");"#);
    let _ = run_test_script_err(r#"app.external_service("a");"#);
}

// l[verify service.http.compress]
// l[verify service.http.balance]
#[test]
fn http_service_accepts_compress_and_balance() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80)
            .compress(#{ minimum_length: 1024 })
            .balance(#{ policy: "least_conn" });
        web.route("/api").balance(#{ try_duration: 10 });
        web.route("/v1").compress(false);
    "#,
    );
    let def = app.def.load();
    let svc = def
        .resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Service(s) if &*s.name == "web" => Some(s.clone()),
            _ => None,
        })
        .expect("web service");
    let http = svc.def.lock().http.clone().expect("http def");

    assert_eq!(
        http.proxy.balance.policy,
        Some(defs::service::LbPolicy::LeastConn)
    );
    assert!(http.proxy.compress.is_some());
    assert_eq!(
        http.routes
            .get("/api")
            .and_then(|r| r.balance.try_duration_secs),
        Some(10.0)
    );
    assert_eq!(
        http.routes.get("/v1").and_then(|r| r.compress.clone()),
        Some(defs::service::CompressDecl::Disabled)
    );

    // The route that named only a try duration keeps the service's policy.
    let api = defs::service::resolve(&http.proxy, http.routes.get("/api"));
    assert_eq!(api.balance.policy, defs::service::LbPolicy::LeastConn);
    assert_eq!(api.balance.try_duration_secs, 10.0);
    assert_eq!(api.compress.expect("on").minimum_length, 1024);

    // The route that switched compression off keeps the service's policy too.
    let v1 = defs::service::resolve(&http.proxy, http.routes.get("/v1"));
    assert!(v1.compress.is_none());
    assert_eq!(v1.balance.policy, defs::service::LbPolicy::LeastConn);
}

// l[verify service.http.compress.fields]
#[test]
fn compress_rejects_unknown_encoding() {
    let _ = run_test_script_err(r#"app.service("web").http(80).compress(#{ encodings: ["br"] });"#);
}

// l[verify service.http.balance]
#[test]
fn balance_rejects_unknown_policy() {
    let _ = run_test_script_err(r#"app.service("web").http(80).balance(#{ policy: "sticky" });"#);
}

// l[verify service.http.balance]
#[test]
fn balance_rejects_spinning_interval() {
    let _ = run_test_script_err(r#"app.service("web").http(80).balance(#{ interval: 0 });"#);
}
