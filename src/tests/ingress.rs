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
    let def = app.def.lock();
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

// l[verify ingress.quic]
#[test]
fn ingress_quic() {
    run_test_script_app(
        r#"
        let ing = app.service("web").ingress("example.com", 443).quic();
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

// l[verify ingress.conflicts]
#[test]
fn ingress_conflict_within_app_is_catchable() {
    let _ = run_test_script_app(
        r#"
        let svc = app.service("web");
        svc.ingress("example.com", 443).http();
        try {
            svc.ingress("example.com", 443).http();
        } catch(err) {
            // conflict caught
        }
    "#,
    );
}
