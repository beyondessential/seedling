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

// l[verify service.http.headers]
// l[verify service.http.proxy-settings.resolution]
#[test]
fn http_service_and_route_accept_headers() {
    // The three requirements this surface exists for, as an app would write
    // them: a `Host` override to the pods, a cache policy on one prefix, and
    // nothing at all about keep-alive, which is served without being asked.
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80)
            .headers(#{
                request: #{ replace: #{ "Host": "central.internal" } },
                response: #{ remove: ["Server"] },
            });
        web.route("/assets").headers(#{
            response: #{ replace: #{ "Cache-Control": "public, max-age=31536000, immutable" } },
        });
        web.route("/api");
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

    let grouped = |prefix: &str| {
        let r = defs::service::resolve(&service_level, http.routes.get(prefix));
        (
            r.headers.request.into_grouped(),
            r.headers.response.into_grouped(),
        )
    };

    // The prefix with a cache policy of its own keeps the service's `Host`
    // override and its `Server` removal: a route adds to the service's
    // headers rather than replacing them.
    let (request, response) = grouped("/assets");
    assert_eq!(
        request.replace.get("Host").map(Vec::as_slice),
        Some(["central.internal".to_string()].as_slice())
    );
    assert_eq!(response.remove, vec!["Server".to_string()]);
    assert_eq!(
        response.replace.get("Cache-Control").map(Vec::as_slice),
        Some(["public, max-age=31536000, immutable".to_string()].as_slice())
    );

    // The prefix that declared nothing takes the service's headers, and does
    // not pick up the cache policy the other prefix declared.
    let (request, response) = grouped("/api");
    assert!(request.replace.contains_key("Host"));
    assert_eq!(response.remove, vec!["Server".to_string()]);
    assert!(response.replace.is_empty());
}

// l[verify service.http.headers.fields]
#[test]
fn http_service_headers_rejects_a_proxy_owned_header() {
    // Keep-alive is a platform guarantee, so an app asking for it is refused
    // rather than silently emitting a header that is illegal on HTTP/2.
    let err = run_test_script_err(
        r#"
        app.service("web").http(80).headers(#{
            response: #{ replace: #{ "Connection": "keep-alive" } },
        });
    "#,
    );
    assert!(
        format!("{err}").contains("belongs to the proxy"),
        "unexpected error: {err}"
    );
}

// i[verify app.describe.proxy-settings]
// r[verify service.http.route.proxy-settings.visibility]
#[test]
fn service_summary_reports_resolved_headers() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80)
            .headers(#{ response: #{ replace: #{ "Cache-Control": "no-store" } } });
        web.route("/");
        web.route("/assets").headers(#{
            response: #{
                replace: #{ "cache-control": "public, max-age=600" },
                add: #{ "Set-Cookie": ["a=1", "b=2"] },
            },
        });
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

    // The route that declared nothing reports what it inherits, so an
    // operator reads the headers in force rather than the ones declared here.
    let root = &routes[0];
    assert_eq!(
        root.headers.response.replace.get("Cache-Control"),
        Some(&vec!["no-store".to_string()])
    );
    // A direction with nothing declared reports its operations empty rather
    // than absent, so routes compare like with like.
    assert!(root.headers.request.replace.is_empty());
    assert!(root.headers.request.add.is_empty());
    assert!(root.headers.request.remove.is_empty());

    // The overriding route reports its own value under its own spelling, and
    // a value written as a bare string still reads back as an array.
    let assets = &routes[1];
    assert_eq!(
        assets.headers.response.replace.get("cache-control"),
        Some(&vec!["public, max-age=600".to_string()])
    );
    assert!(
        !assets
            .headers
            .response
            .replace
            .contains_key("Cache-Control")
    );
    assert_eq!(
        assets.headers.response.add.get("Set-Cookie"),
        Some(&vec!["a=1".to_string(), "b=2".to_string()])
    );
}

// l[verify service.http.headers]
#[test]
fn a_second_headers_call_layers_over_the_first() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80)
            .headers(#{ response: #{ remove: ["Server"] } })
            .headers(#{ response: #{ replace: #{ "Cache-Control": "no-store" } } });
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
    let grouped = http.headers.response.clone().into_grouped();
    // The second call added its header without discarding the first's.
    assert_eq!(grouped.remove, vec!["Server".to_string()]);
    assert!(grouped.replace.contains_key("Cache-Control"));
}

// ---------------------------------------------------------------------------
// Route redirects
// ---------------------------------------------------------------------------

fn http_def(app: &defs::app::App, service: &str) -> defs::service::HttpServiceDef {
    let def = app.def.load();
    def.resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Service(s) if *s.name == *service => {
                s.def.lock().http.clone()
            }
            defs::resource::Resource::ExternalService(e) if *e.name == *service => {
                e.def.lock().http.clone()
            }
            _ => None,
        })
        .expect("service has an http surface")
}

fn redirect_of(app: &defs::app::App, service: &str, prefix: &str) -> defs::service::RouteRedirect {
    http_def(app, service)
        .redirects
        .get(prefix)
        .cloned()
        .unwrap_or_else(|| panic!("`{prefix}` is declared as a redirect"))
}

// l[verify service.http.route.redirect]
// The card's own case: one path inside a vhost that otherwise proxies.
#[test]
fn the_positional_forms_carry_the_tail_and_query_and_default_to_307() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80);
        web.route("/v1/login").redirect("/api/login", 308);
        web.route("/v1/legacy").redirect("/api/legacy");
    "#,
    );
    use defs::service::RedirectSegment::{Literal, Query, Tail};

    let login = redirect_of(&app, "web", "/v1/login");
    assert_eq!(login.code, 308);
    assert_eq!(
        login.target,
        vec![Literal("/api/login".into()), Tail, Query],
        "the positional forms are the map form with the tail and query appended"
    );

    assert_eq!(redirect_of(&app, "web", "/v1/legacy").code, 307);
}

// l[verify service.http.route.redirect]
#[test]
fn the_map_form_is_served_exactly_as_it_was_written() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80);
        web.route("/v1/login").redirect(#{ to: "/api/login" });
        web.route("/search").redirect(#{ to: "/find<tail><query>", code: 301 });
    "#,
    );
    use defs::service::RedirectSegment::{Literal, Query, Tail};

    // Naming no token means every request under the prefix lands on exactly
    // this target, whatever followed the prefix.
    assert_eq!(
        redirect_of(&app, "web", "/v1/login").target,
        vec![Literal("/api/login".into())]
    );

    let search = redirect_of(&app, "web", "/search");
    assert_eq!(search.code, 301);
    assert_eq!(search.target, vec![Literal("/find".into()), Tail, Query]);
}

// l[verify service.http.route.redirect]
#[test]
fn a_redirect_is_declarable_on_an_external_services_routes() {
    let app = run_test_script_app(
        r#"
        let ext = app.external_service("upstream").http(80);
        ext.route("/v1/login").redirect("/api/login", 308);
    "#,
    );
    assert_eq!(redirect_of(&app, "upstream", "/v1/login").code, 308);
}

// l[verify service.http.route.redirect]
#[test]
fn a_second_redirect_replaces_the_first() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80);
        web.route("/v1/login").redirect("/api/login", 307).redirect("/api/login", 308);
    "#,
    );
    // The route is a redirect either way, so there is nothing for the two to
    // contradict each other about.
    assert_eq!(redirect_of(&app, "web", "/v1/login").code, 308);
}

// l[verify service.http.route.redirect]
#[test]
fn a_target_that_is_neither_a_path_nor_an_absolute_url_is_refused() {
    for target in ["api/login", "", "example.com/login"] {
        let e = run_test_script_err(&format!(
            r#"app.service("web").http(80).route("/v1").redirect("{target}");"#
        ))
        .to_string();
        assert!(e.contains("must be a path starting with `/`"), "{e}");
    }

    // `//host` reads as a path while naming another host, so it is refused
    // rather than quietly meaning the opposite of what it looks like.
    let e = run_test_script_err(
        r#"app.service("web").http(80).route("/v1").redirect("//evil.example.com/login");"#,
    )
    .to_string();
    assert!(e.contains("names another host"), "{e}");
}

// l[verify service.http.route.redirect]
// The proxy substitutes a braced word from its own state, its environment
// among it, and the target is served as a `Location`. A header value is
// refused a brace for that same reason.
#[test]
fn a_target_written_in_the_proxys_placeholder_syntax_is_refused() {
    let e = run_test_script_err(
        r#"app.service("web").http(80).route("/v1").redirect("/api/{env.SECRET}");"#,
    )
    .to_string();
    assert!(e.contains("must not contain `{`"), "{e}");
    assert!(
        e.contains("<tail>"),
        "the error names what to write instead: {e}"
    );
}

// l[verify service.http.route.redirect]
#[test]
fn a_token_naming_nothing_is_refused_rather_than_served_to_a_client() {
    let e = run_test_script_err(
        r#"app.service("web").http(80).route("/v1").redirect(#{ to: "/api/<path>" });"#,
    )
    .to_string();
    assert!(e.contains("<path>"), "{e}");
    assert!(e.contains("<tail>") && e.contains("<query>"), "{e}");

    // A stray `<` is a character a URL carries percent-encoded, so there is
    // nothing for it to have meant either.
    let e = run_test_script_err(
        r#"app.service("web").http(80).route("/v1").redirect(#{ to: "/api/a<b" });"#,
    )
    .to_string();
    assert!(e.contains("%3C"), "{e}");
}

// l[verify service.http.route.redirect]
#[test]
fn a_code_that_is_not_a_redirect_a_browser_follows_is_refused() {
    for code in ["200", "303", "404"] {
        let e = run_test_script_err(&format!(
            r#"app.service("web").http(80).route("/v1").redirect("/api", {code});"#
        ))
        .to_string();
        assert!(e.contains("301, 302, 307, or 308"), "{e}");
    }
}

// l[verify service.http.route.redirect]
#[test]
fn the_map_form_refuses_a_field_it_does_not_carry() {
    let e = run_test_script_err(
        r#"app.service("web").http(80).route("/v1").redirect(#{ to: "/api", status: 308 });"#,
    )
    .to_string();
    assert!(e.contains("unknown") && e.contains("status"), "{e}");

    let e = run_test_script_err(r#"app.service("web").http(80).route("/v1").redirect(#{});"#)
        .to_string();
    assert!(e.contains("requires `to`"), "{e}");
}

// l[verify service.http.route.redirect]
// Retiring a hostname is a site ingress redirect attachment, an operator's to
// make rather than an app's.
#[test]
fn a_redirect_on_the_root_prefix_is_refused() {
    let e = run_test_script_err(r#"app.service("web").http(80).route("/").redirect("/api");"#)
        .to_string();
    assert!(e.contains("whole hostname"), "{e}");
}

// l[verify service.http.route.redirect]
// A prefix is either redirected or proxied, and which of the two was written
// first must not decide whether the clash is caught.
#[test]
fn a_prefix_cannot_be_both_redirected_and_bound_by_a_pod() {
    let redirect_first = run_test_script_err(
        r#"
        let web = app.service("web").http(80);
        web.route("/v1/login").redirect("/api/login");
        app.deployment("api").image("ghcr.io/example/api:1").http(8080, web.route("/v1/login"));
    "#,
    )
    .to_string();
    assert!(
        redirect_first.contains("either redirected or proxied"),
        "{redirect_first}"
    );

    let binding_first = run_test_script_err(
        r#"
        let web = app.service("web").http(80);
        app.deployment("api").image("ghcr.io/example/api:1").http(8080, web.route("/v1/login"));
        web.route("/v1/login").redirect("/api/login");
    "#,
    )
    .to_string();
    assert!(
        binding_first.contains("either redirected or proxied"),
        "{binding_first}"
    );
}

// l[verify service.http.route.redirect]
#[test]
fn a_setting_a_redirect_leaves_nothing_to_act_on_is_refused_either_way_round() {
    for setting in [
        "compress(true)",
        "compress(#{ minimum_length: 1024 })",
        "balance(#{ policy: \"least_conn\" })",
        "rate_limit(#{ max_events: 10, window: 1 })",
        "rate_limit(false)",
    ] {
        let after = run_test_script_err(&format!(
            r#"app.service("web").http(80).route("/v1").redirect("/api").{setting};"#
        ))
        .to_string();
        assert!(
            after.contains("has nothing to act on"),
            "{setting}: {after}"
        );

        let before = run_test_script_err(&format!(
            r#"app.service("web").http(80).route("/v1").{setting}.redirect("/api");"#
        ))
        .to_string();
        assert!(
            before.contains("has nothing to act on"),
            "{setting}: {before}"
        );
    }
}

// l[verify service.http.route.redirect]
// The same setting declared on the Service is ignored rather than refused, so
// a service-wide declaration need not be written around the redirect routes.
#[test]
fn a_service_wide_setting_is_ignored_on_a_redirect_route_rather_than_refused() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80)
            .compress(true)
            .rate_limit(#{ max_events: 10, window: 1 })
            .headers(#{ request: #{ replace: #{ "Host": "internal" } } });
        web.route("/v1/login").redirect("/api/login", 308);
    "#,
    );
    assert_eq!(redirect_of(&app, "web", "/v1/login").code, 308);
}

// l[verify service.http.route.redirect]
#[test]
fn a_redirect_route_takes_response_header_operations_and_no_others() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80);
        web.route("/v1/login")
            .redirect("/api/login", 308)
            .headers(#{ response: #{ replace: #{ "Cache-Control": "no-store" } } });
    "#,
    );
    let http = http_def(&app, "web");
    let grouped = http.routes["/v1/login"]
        .headers
        .response
        .clone()
        .into_grouped();
    assert!(grouped.replace.contains_key("Cache-Control"));

    // A redirect sends no request onward, so there is nothing to shape.
    for script in [
        r#"app.service("web").http(80).route("/v1").redirect("/api")
               .headers(#{ request: #{ replace: #{ "Host": "internal" } } });"#,
        r#"app.service("web").http(80).route("/v1")
               .headers(#{ request: #{ replace: #{ "Host": "internal" } } }).redirect("/api");"#,
    ] {
        let e = run_test_script_err(script).to_string();
        assert!(e.contains("nothing to shape"), "{e}");
    }

    // `Location` is the target the redirect computed, so an operation naming
    // it would leave the route not serving what `to` declares.
    for script in [
        r#"app.service("web").http(80).route("/v1").redirect("/api")
               .headers(#{ response: #{ replace: #{ "location": "/elsewhere" } } });"#,
        r#"app.service("web").http(80).route("/v1")
               .headers(#{ response: #{ remove: ["Location"] } }).redirect("/api");"#,
    ] {
        let e = run_test_script_err(script).to_string();
        assert!(e.contains("Location"), "{e}");
    }
}

// i[verify app.describe.proxy-settings]
// r[verify service.http.route.redirect]
#[test]
fn a_redirect_route_reports_its_target_and_carries_no_settings() {
    let app = run_test_script_app(
        r#"
        let web = app.service("web").http(80)
            .rate_limit(#{ max_events: 10, window: 1 });
        web.route("/v1/login").redirect("/api/login", 308);
        app.deployment("api").image("ghcr.io/example/api:1").http(8080, web.route("/"));
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
    let login = routes
        .iter()
        .find(|r| r.prefix == "/v1/login")
        .expect("the redirect route is reported");

    let redirect = login.redirect.as_ref().expect("reports its redirect");
    assert_eq!(redirect.to, "/api/login<tail><query>");
    assert_eq!(redirect.code, 308);
    // Served without a pod binding it.
    assert!(login.served);
    // The service's limit reaches no pod here, and reporting it would read as
    // a control that is in force.
    assert!(login.rate_limit.is_none());
    assert!(login.compress.is_none());

    // The proxied route alongside it still reports the service's limit.
    let root = routes.iter().find(|r| r.prefix == "/").expect("root route");
    assert!(root.redirect.is_none());
    assert!(root.rate_limit.is_some());
}
