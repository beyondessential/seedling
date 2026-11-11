use super::*;
use defs::resource::ResourceKind;

// spec: ING#declaration
// spec: ING#termination
// spec: ING#declaration
// spec: ING#terminate
// spec: ING#output
#[test]
fn ingress_builder_chain() {
    let app = run_test_script_app(
        r#"
        let domain = "example.com";
        let traffic = app.service("public")
            .ingress(domain, 443).tls(Terminate.Https, Output.Http1)
            .service()
            .http(80);
    "#,
    );
    let def = app.def.load();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Service && &*id.name == "public")
    );
}

// spec: ING#termination
// spec: ING#terminate
// spec: ING#output
#[test]
fn ingress_terminate_tls_to_tcp() {
    run_test_script_app(
        r#"
        let ing = app.service("web").ingress("example.com", 443).tls(Terminate.Tls, Output.Tcp);
    "#,
    );
}

// spec: ING#termination
// spec: ING#terminate
// spec: ING#output
#[test]
fn ingress_terminate_dtls_to_udp() {
    run_test_script_app(
        r#"
        let ing = app.service("web").ingress("example.com", 443).tls(Terminate.Dtls, Output.Udp);
    "#,
    );
}

// spec: ING#termination
// spec: ING#output
#[test]
fn ingress_terminate_https_to_http2() {
    run_test_script_app(
        r#"
        let ing = app.service("web").ingress("example.com", 443).tls(Terminate.Https, Output.Http2);
    "#,
    );
}

// spec: ING#termination
#[test]
fn invalid_termination_combo_throws() {
    // Terminate.Https + Output.Tcp is nonsense — HTTPS termination
    // implies the ingress understands HTTP, so the output must be one
    // of the HTTP variants.
    let err = run_test_script_err(
        r#"
        app.service("web").ingress("example.com", 443).tls(Terminate.Https, Output.Tcp);
    "#,
    );
    assert!(
        err.to_string()
            .contains("invalid termination/output combination"),
        "expected combo error, got: {err}"
    );
}

// spec: ING#redirect-to-https
#[test]
fn ingress_redirect_defaults() {
    run_test_script_app(
        r#"
        app.service("web").ingress("example.com", 443).tls(Terminate.Https, Output.Http1).redirect();
    "#,
    );
}

// spec: ING#redirect-to-https
#[test]
fn ingress_redirect_custom_port_and_code() {
    run_test_script_app(
        r#"
        app.service("web").ingress("example.com", 443).tls(Terminate.Https, Output.Http1).redirect(8080, 301);
    "#,
    );
}

// spec: ING#redirect-to-https
#[test]
fn ingress_redirect_without_https_throws() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("example.com", 443).tls(Terminate.Tls, Output.Tcp).redirect();
    "#,
    );
}

// spec: ING#certificates
#[test]
fn ingress_certificates_implied_by_https() {
    let _ = run_test_script_app(
        r#"
        app.service("web").ingress("example.com", 443).tls(Terminate.Https, Output.Http1);
    "#,
    );
}

// spec: ING#hostnames
#[test]
fn a_hostname_is_held_in_one_form_whatever_case_it_is_written_in() {
    let app = run_test_script_app(
        r#"
        app.service("web").ingress("Shop.Example.COM", 443).tls(Terminate.Https, Output.Http1);
    "#,
    );
    let def = app.def.load();
    let ingress = def
        .resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Ingress(i) => Some(i.clone()),
            _ => None,
        })
        .expect("the ingress was declared");
    assert_eq!(
        ingress.def.lock().hostname,
        "shop.example.com",
        "a name a handshake could never present is a name nothing decides for"
    );
    assert!(
        ingress.name.contains("shop.example.com"),
        "and what is derived from it shares the one form: {}",
        ingress.name
    );
}

// spec: ING#hostnames
#[test]
fn hostname_rejects_wildcard() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("*.example.com", 443);
    "#,
    );
}

// spec: ING#hostnames
#[test]
fn hostname_rejects_leading_hyphen_label() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("-example.com", 443);
    "#,
    );
}

// spec: ING#hostnames
#[test]
fn hostname_rejects_trailing_hyphen_label() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("example-.com", 443);
    "#,
    );
}

// spec: ING#hostnames
#[test]
fn hostname_rejects_empty_label() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("example..com", 443);
    "#,
    );
}

// spec: ING#hostnames
#[test]
fn hostname_rejects_invalid_characters() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("ex ample.com", 443);
    "#,
    );
}

// spec: ING#hostnames
#[test]
fn hostname_rejects_empty_string() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("", 443);
    "#,
    );
}

// spec: ING#hostnames
#[test]
fn hostname_accepts_valid_domains() {
    run_test_script_app(
        r#"
        let svc = app.service("web");
        svc.ingress("example.com", 443);
        svc.ingress("sub.example.com", 80);
        svc.ingress("a.b.c.d.example.com", 8080);
        svc.ingress("localhost", 9000);
    "#,
    );
}

// spec: ING#declaration
#[test]
fn multiple_ingresses_on_same_service_coexist() {
    let app = run_test_script_app(
        r#"
        let traffic = app.service("public");
        traffic.ingress("test.example.com", 443).tls(Terminate.Https, Output.Http1).redirect();
        traffic.ingress("test.localhost", 443).tls(Terminate.Https, Output.Http1).redirect();
        traffic.ingress("test2.localhost", 80);
    "#,
    );
    let def = app.def.load();
    let ingress_names: Vec<String> = def
        .resources
        .keys()
        .filter(|id| id.kind == ResourceKind::Ingress)
        .map(|id| (*id.name).clone())
        .collect();
    assert_eq!(ingress_names.len(), 3, "got {ingress_names:?}");
    assert!(ingress_names.contains(&"test.example.com:443".to_owned()));
    assert!(ingress_names.contains(&"test.localhost:443".to_owned()));
    assert!(ingress_names.contains(&"test2.localhost:80".to_owned()));
}

// spec: ING#identity-within-an-app
#[test]
fn ingress_conflict_within_app_throws() {
    // Same (hostname, port) must throw on the second declaration so
    // the first isn't silently overwritten.
    let err = run_test_script_err(
        r#"
        let svc = app.service("web");
        svc.ingress("example.com", 443).tls(Terminate.Https, Output.Http1);
        svc.ingress("example.com", 443).tls(Terminate.Https, Output.Http1);
    "#,
    );
    assert!(
        err.to_string().contains("ingress conflict"),
        "expected conflict error, got: {err}"
    );
}

// spec: ING#identity-within-an-app
#[test]
fn ingress_conflict_within_app_is_catchable() {
    // The throw is from the host function, so a try/catch can recover.
    let app = run_test_script_app(
        r#"
        let svc = app.service("web");
        svc.ingress("example.com", 443).tls(Terminate.Https, Output.Http1);
        try {
            svc.ingress("example.com", 443).tls(Terminate.Https, Output.Http1);
        } catch(err) {
            // conflict caught — the original ingress should remain.
        }
    "#,
    );
    let def = app.def.load();
    let ingresses: Vec<_> = def
        .resources
        .keys()
        .filter(|id| id.kind == ResourceKind::Ingress)
        .collect();
    assert_eq!(ingresses.len(), 1, "exactly one ingress should remain");
}

// spec: ING#declaration
#[test]
fn http_service_can_declare_ingress() {
    // ingress() called on an HttpService delegates to the underlying
    // Service — same identity, same conflict behaviour, same chain
    // ergonomics as svc.ingress(). The HttpService is a per-call view
    // of the service, not a separate resource.
    let app = run_test_script_app(
        r#"
        let api = app.service("api").http(8080);
        api.ingress("api.example.com", 443).tls(Terminate.Https, Output.Http1);
        api.ingress("api.localhost", 443).tls(Terminate.Https, Output.Http1);
    "#,
    );
    let def = app.def.load();
    let names: Vec<String> = def
        .resources
        .keys()
        .filter(|id| id.kind == ResourceKind::Ingress)
        .map(|id| (*id.name).clone())
        .collect();
    assert_eq!(names.len(), 2, "got {names:?}");
    assert!(names.contains(&"api.example.com:443".to_owned()));
    assert!(names.contains(&"api.localhost:443".to_owned()));
}

// spec: ING#declaration
#[test]
fn http_service_ingress_conflicts_with_service_ingress() {
    // The ingress declared via HttpService and the one declared via
    // Service share the same (hostname, port) keying, so duplicates
    // collide regardless of which surface declared them.
    let err = run_test_script_err(
        r#"
        let svc = app.service("web");
        svc.ingress("example.com", 443).tls(Terminate.Https, Output.Http1);
        svc.http(8080).ingress("example.com", 443).tls(Terminate.Https, Output.Http1);
    "#,
    );
    assert!(
        err.to_string().contains("ingress conflict"),
        "expected conflict error, got: {err}"
    );
}

// spec: ING#identity-within-an-app
#[test]
fn ingress_same_hostname_different_port_does_not_conflict() {
    // Same hostname, different ports — distinct ingresses, no error.
    let app = run_test_script_app(
        r#"
        let svc = app.service("web");
        svc.ingress("example.com", 80);
        svc.ingress("example.com", 443).tls(Terminate.Https, Output.Http1);
    "#,
    );
    let def = app.def.load();
    let count = def
        .resources
        .keys()
        .filter(|id| id.kind == ResourceKind::Ingress)
        .count();
    assert_eq!(count, 2);
}

// spec: BSL#arguments
// spec: ING#redirect-to-https
// The unchecked `as u16` wrapped, so 65843 became 307 — a redirect to a
// status code that is not a redirect at all.
#[test]
fn redirect_code_must_be_a_redirection_status() {
    let err = run_test_script_err(
        r#"
        app.service("web").ingress("example.com", 443)
            .tls(Terminate.Https, Output.Http1)
            .redirect(80, 200);
    "#,
    );
    let message = err.to_string();
    assert!(message.contains("`redirect code`"), "{message}");
    assert!(message.contains("300"), "{message}");
}

// spec: ING#declaration
// spec: APP#anonymous-resources
// An anonymous service belongs to no app, so an ingress declared on it used to
// succeed without registering anything and hand back a handle nothing routes to.
#[test]
fn ingress_on_anonymous_service_throws() {
    let msg = run_action_err(
        r#"
        app.on_action("run-it", |rt, _param| {
            let s = app.service();
            s.ingress("example.com", 443);
        });
    "#,
        "run-it",
    );
    assert!(msg.contains("anonymous service"), "got: {msg}");
}

// spec: ING#declaration
#[test]
fn ingress_on_anonymous_http_service_throws() {
    let msg = run_action_err(
        r#"
        app.on_action("run-it", |rt, _param| {
            app.service().http(8080).ingress("example.com", 443);
        });
    "#,
        "run-it",
    );
    assert!(msg.contains("anonymous service"), "got: {msg}");
}
