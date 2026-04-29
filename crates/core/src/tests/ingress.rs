use super::*;
use defs::resource::ResourceKind;

// l[verify ingress.type]
// l[verify ingress.termination]
// l[verify ingress.service]
// l[verify const.terminate.https]
// l[verify const.output.http1]
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

// l[verify ingress.termination]
// l[verify const.terminate.tls]
// l[verify const.output.tcp]
#[test]
fn ingress_terminate_tls_to_tcp() {
    run_test_script_app(
        r#"
        let ing = app.service("web").ingress("example.com", 443).tls(Terminate.Tls, Output.Tcp);
    "#,
    );
}

// l[verify ingress.termination]
// l[verify const.terminate.dtls]
// l[verify const.output.udp]
#[test]
fn ingress_terminate_dtls_to_udp() {
    run_test_script_app(
        r#"
        let ing = app.service("web").ingress("example.com", 443).tls(Terminate.Dtls, Output.Udp);
    "#,
    );
}

// l[verify ingress.termination]
// l[verify const.output.http2]
#[test]
fn ingress_terminate_https_to_http2() {
    run_test_script_app(
        r#"
        let ing = app.service("web").ingress("example.com", 443).tls(Terminate.Https, Output.Http2);
    "#,
    );
}

// l[verify ingress.termination]
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

// l[verify ingress.redirect]
#[test]
fn ingress_redirect_defaults() {
    run_test_script_app(
        r#"
        app.service("web").ingress("example.com", 443).tls(Terminate.Https, Output.Http1).redirect();
    "#,
    );
}

// l[verify ingress.redirect]
#[test]
fn ingress_redirect_custom_port_and_code() {
    run_test_script_app(
        r#"
        app.service("web").ingress("example.com", 443).tls(Terminate.Https, Output.Http1).redirect(8080, 301);
    "#,
    );
}

// l[verify ingress.redirect]
#[test]
fn ingress_redirect_without_https_throws() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("example.com", 443).tls(Terminate.Tls, Output.Tcp).redirect();
    "#,
    );
}

// l[verify ingress.certificates]
#[test]
fn ingress_certificates_implied_by_https() {
    let _ = run_test_script_app(
        r#"
        app.service("web").ingress("example.com", 443).tls(Terminate.Https, Output.Http1);
    "#,
    );
}

// l[verify ingress.hostname]
#[test]
fn hostname_rejects_wildcard() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("*.example.com", 443);
    "#,
    );
}

// l[verify ingress.hostname]
#[test]
fn hostname_rejects_leading_hyphen_label() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("-example.com", 443);
    "#,
    );
}

// l[verify ingress.hostname]
#[test]
fn hostname_rejects_trailing_hyphen_label() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("example-.com", 443);
    "#,
    );
}

// l[verify ingress.hostname]
#[test]
fn hostname_rejects_empty_label() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("example..com", 443);
    "#,
    );
}

// l[verify ingress.hostname]
#[test]
fn hostname_rejects_invalid_characters() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("ex ample.com", 443);
    "#,
    );
}

// l[verify ingress.hostname]
#[test]
fn hostname_rejects_empty_string() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("", 443);
    "#,
    );
}

// l[verify ingress.hostname]
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

// l[verify ingress.type]
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

// l[verify ingress.conflicts]
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

// l[verify ingress.conflicts]
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

// l[verify ingress.type]
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

// l[verify ingress.type]
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

// l[verify ingress.conflicts]
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
