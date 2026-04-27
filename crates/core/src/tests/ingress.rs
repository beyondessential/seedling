use super::*;
use defs::resource::ResourceKind;

// l[verify ingress.type]
// l[verify ingress.http]
// l[verify ingress.service]
#[test]
fn ingress_builder_chain() {
    let app = run_test_script_app(
        r#"
        let domain = "example.com";
        let traffic = app.service("public")
            .ingress(domain, 443).http()
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

// l[verify ingress.tls]
#[test]
fn ingress_tls() {
    run_test_script_app(
        r#"
        let ing = app.service("web").ingress("example.com", 443).tls();
    "#,
    );
}

// l[verify ingress.dtls]
#[test]
fn ingress_dtls() {
    run_test_script_app(
        r#"
        let ing = app.service("web").ingress("example.com", 443).dtls();
    "#,
    );
}

// l[verify ingress.http2]
#[test]
fn ingress_http2() {
    run_test_script_app(
        r#"
        let ing = app.service("web").ingress("example.com", 443).http2();
    "#,
    );
}

// l[verify ingress.redirect]
#[test]
fn ingress_redirect_defaults() {
    run_test_script_app(
        r#"
        app.service("web").ingress("example.com", 443).http().redirect();
    "#,
    );
}

// l[verify ingress.redirect]
#[test]
fn ingress_redirect_custom_port_and_code() {
    run_test_script_app(
        r#"
        app.service("web").ingress("example.com", 443).http().redirect(8080, 301);
    "#,
    );
}

// l[verify ingress.redirect]
#[test]
fn ingress_redirect_without_https_throws() {
    let _ = run_test_script_err(
        r#"
        app.service("web").ingress("example.com", 443).tls().redirect();
    "#,
    );
}

// l[verify ingress.certificates]
#[test]
fn ingress_certificates_implied_by_http() {
    let _ = run_test_script_app(
        r#"
        app.service("web").ingress("example.com", 443).http();
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
        traffic.ingress("test.example.com", 443).http().redirect();
        traffic.ingress("test.localhost", 443).http().redirect();
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
        svc.ingress("example.com", 443).http();
        svc.ingress("example.com", 443).http();
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
        svc.ingress("example.com", 443).http();
        try {
            svc.ingress("example.com", 443).http();
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

// l[verify ingress.conflicts]
#[test]
fn ingress_same_hostname_different_port_does_not_conflict() {
    // Same hostname, different ports — distinct ingresses, no error.
    let app = run_test_script_app(
        r#"
        let svc = app.service("web");
        svc.ingress("example.com", 80);
        svc.ingress("example.com", 443).http();
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
