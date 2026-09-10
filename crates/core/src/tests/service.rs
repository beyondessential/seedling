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
// l[verify service.balance]
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
    let service_def = svc.def.lock().clone();
    let http = service_def.http.clone().expect("http def");
    let service_level = service_def.proxy_settings();

    // Balancing is service-wide, not part of the HTTP surface.
    assert_eq!(
        service_def.balance.policy,
        Some(defs::service::LbPolicy::LeastConn)
    );
    assert!(http.compress.is_some());
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
    let api = defs::service::resolve(&service_level, http.routes.get("/api"));
    assert_eq!(api.balance.policy, defs::service::LbPolicy::LeastConn);
    assert_eq!(api.balance.try_duration_secs, 10.0);
    assert_eq!(api.compress.expect("on").minimum_length, 1024);

    // The route that switched compression off keeps the service's policy too.
    let v1 = defs::service::resolve(&service_level, http.routes.get("/v1"));
    assert!(v1.compress.is_none());
    assert_eq!(v1.balance.policy, defs::service::LbPolicy::LeastConn);
}

// l[verify service.http.compress.fields]
#[test]
fn compress_rejects_unknown_encoding() {
    let _ = run_test_script_err(r#"app.service("web").http(80).compress(#{ encodings: ["br"] });"#);
}

// l[verify service.balance]
#[test]
fn balance_rejects_unknown_policy() {
    let _ = run_test_script_err(r#"app.service("web").http(80).balance(#{ policy: "sticky" });"#);
}

// l[verify service.balance]
#[test]
fn balance_rejects_spinning_interval() {
    let _ = run_test_script_err(r#"app.service("web").http(80).balance(#{ interval: 0 });"#);
}

// i[verify app.describe.proxy-settings]
// r[verify service.http.route.proxy-settings.visibility]
#[test]
fn service_summary_reports_resolved_routes() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80)
            .compress(#{ minimum_length: 1024 })
            .balance(#{ policy: "least_conn" });
        web.route("/");
        web.route("/api").balance(#{ try_duration: 10 });
        web.route("/v1").compress(false);
    "#,
    );
    let def = app.def.load();
    let summary = def
        .resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Service(s) if &*s.name == "web" => Some(s.summary(&def)),
            _ => None,
        })
        .expect("web service");

    // The service-wide policy is reported on the def itself.
    assert_eq!(summary.balance.policy, "least_conn");

    let routes = summary.routes.expect("http service reports routes");
    let prefixes: Vec<&str> = routes.iter().map(|r| r.prefix.as_str()).collect();
    assert_eq!(prefixes, vec!["/", "/api", "/v1"]);

    // Defaults are reported, not omitted.
    let root = &routes[0];
    let compress = root.compress.as_ref().expect("compression on");
    assert_eq!(compress.encodings, vec!["zstd", "gzip"]);
    assert_eq!(compress.minimum_length, 1024);
    assert!(compress.content_types.contains(&"text/*".to_string()));
    assert_eq!(root.balance.policy, "least_conn");
    assert_eq!(root.balance.try_duration, 5.0);
    assert_eq!(root.balance.interval, 0.25);

    // The route that overrode one field reports the resolved result.
    assert_eq!(routes[1].balance.try_duration, 10.0);
    assert_eq!(routes[1].balance.policy, "least_conn");

    // Compression off reports as null rather than a default-valued object.
    assert!(routes[2].compress.is_none());
    assert_eq!(routes[2].balance.policy, "least_conn");
}

// i[verify app.describe.proxy-settings]
#[test]
fn http_service_without_bindings_reports_a_single_root_route() {
    let app = run_test_script_app(r#"app.service("web").http(80);"#);
    let def = app.def.load();
    let summary = def
        .resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Service(s) if &*s.name == "web" => Some(s.summary(&def)),
            _ => None,
        })
        .expect("web service");
    let routes = summary.routes.expect("http service reports routes");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].prefix, "/");
}

// i[verify app.describe.proxy-settings]
#[test]
fn non_http_service_reports_no_routes_but_still_reports_balance() {
    let app = run_test_script_app(r#"app.service("postgres").balance(#{ policy: "least_conn" });"#);
    let def = app.def.load();
    let summary = def
        .resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Service(s) if &*s.name == "postgres" => Some(s.summary(&def)),
            _ => None,
        })
        .expect("postgres service");

    // Balancing is service-wide, so a TCP-only service still reports it.
    assert_eq!(summary.balance.policy, "least_conn");
    assert!(summary.routes.is_none());
}

// l[verify service.http.rate-limit]
// l[verify service.http.rate-limit.fields]
#[test]
fn http_service_and_routes_accept_rate_limit() {
    // The chained form the app definitions use: a limit declared on the route
    // returned by `route()`, which must return the route for `.http()` to bind.
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80)
            .rate_limit(#{ max_events: 1000, window: 1 });
        web.route("/api/login").rate_limit(#{ max_events: 10, window: 1 });
        web.route("/health").rate_limit(false);
        web.route("/v1");
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
    let service_def = svc.def.lock().clone();
    let http = service_def.http.clone().expect("http def");
    let service_level = service_def.proxy_settings();

    // The service's limit reaches a route that declared none.
    let v1 = defs::service::resolve(&service_level, http.routes.get("/v1"));
    let v1_limit = v1.rate_limit.expect("service limit carries to the route");
    assert_eq!(v1_limit.settings.max_events, 1000);

    // The tighter route limit replaces it outright.
    let login = defs::service::resolve(&service_level, http.routes.get("/api/login"));
    assert_eq!(
        login.rate_limit.expect("route limit").settings.max_events,
        10
    );

    // And a route can opt out of the service's limit entirely.
    let health = defs::service::resolve(&service_level, http.routes.get("/health"));
    assert_eq!(health.rate_limit, None);
}

// l[verify service.http.rate-limit]
#[test]
fn rate_limit_is_off_unless_declared() {
    let app = run_test_script_app(r#"app.service("web").http(80).route("/api");"#);
    let def = app.def.load();
    let svc = def
        .resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Service(s) if &*s.name == "web" => Some(s.clone()),
            _ => None,
        })
        .expect("web service");
    let service_def = svc.def.lock().clone();
    let http = service_def.http.clone().expect("http def");
    let resolved = defs::service::resolve(&service_def.proxy_settings(), http.routes.get("/api"));
    assert_eq!(resolved.rate_limit, None);
}

// l[verify service.http.rate-limit.fields]
#[test]
fn rate_limit_rejects_incomplete_and_invalid_declarations() {
    let _ = run_test_script_err(r#"app.service("web").http(80).rate_limit(#{ max_events: 10 });"#);
    let _ = run_test_script_err(r#"app.service("web").http(80).rate_limit(#{ window: 1 });"#);
    let _ = run_test_script_err(
        r#"app.service("web").http(80).rate_limit(#{ max_events: 0, window: 1 });"#,
    );
    let _ = run_test_script_err(
        r#"app.service("web").http(80).rate_limit(#{ max_events: 10, window: 0 });"#,
    );
    // A limit has no default, so there is nothing for `true` to turn on.
    let _ = run_test_script_err(r#"app.service("web").http(80).rate_limit(true);"#);
}

// i[verify app.describe.proxy-settings]
// r[verify service.http.route.proxy-settings.visibility]
#[test]
fn service_summary_reports_resolved_rate_limits() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80)
            .rate_limit(#{ max_events: 1000, window: 1 });
        web.route("/api");
        web.route("/api/login").rate_limit(#{ max_events: 10, window: 60 });
        web.route("/health").rate_limit(false);
    "#,
    );
    let def = app.def.load();
    let summary = def
        .resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Service(s) if &*s.name == "web" => Some(s.summary(&def)),
            _ => None,
        })
        .expect("web service");

    let routes = summary.routes.expect("http service reports routes");
    let prefixes: Vec<&str> = routes.iter().map(|r| r.prefix.as_str()).collect();
    assert_eq!(prefixes, vec!["/api", "/api/login", "/health"]);

    // Inherited from the service, reported resolved rather than absent.
    let api = routes[0].rate_limit.as_ref().expect("inherited limit");
    assert_eq!(api.max_events, 1000);
    assert_eq!(api.window, 1.0);
    assert!(
        api.shared,
        "an inherited limit is the service's budget, and an operator reading \
         the route needs to know it is shared rather than the route's own"
    );

    let login = routes[1].rate_limit.as_ref().expect("route limit");
    assert!(!login.shared, "a route's own declaration is its own budget");
    assert_eq!(login.max_events, 10);
    assert_eq!(login.window, 60.0);

    // Not limited reports as null rather than a zero-valued object.
    assert!(routes[2].rate_limit.is_none());
}

// i[verify app.describe.proxy-settings]
#[test]
fn a_route_no_pod_binds_reports_that_it_is_not_served() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80);
        app.deployment("api").http(3000, web.route("/api"));
        // Declared with a limit, but bound by nothing: requests to /api/login
        // are served by the /api route under its settings, not this one.
        web.route("/api/login").rate_limit(#{ max_events: 10, window: 1 });
    "#,
    );
    let def = app.def.load();
    let summary = def
        .resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Service(s) if &*s.name == "web" => Some(s.summary(&def)),
            _ => None,
        })
        .expect("web service");
    let routes = summary.routes.expect("http service reports routes");

    let api = routes.iter().find(|r| r.prefix == "/api").expect("/api");
    assert!(api.served, "a bound prefix is served");

    let login = routes
        .iter()
        .find(|r| r.prefix == "/api/login")
        .expect("/api/login");
    assert!(
        !login.served,
        "a limit on a prefix nothing binds must not read as a control in force"
    );
    assert!(
        login.rate_limit.is_some(),
        "the declaration is still reported"
    );
}
