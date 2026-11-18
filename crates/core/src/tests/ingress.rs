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
    let message = err.to_string();
    assert!(
        message.contains("invalid termination/output combination"),
        "expected combo error, got: {err}"
    );
    assert!(
        message.contains("Terminate.Https + Output.Tcp"),
        "pairing supplied: {message}"
    );
    for valid in [
        "Terminate.Tls + Output.Tcp",
        "Terminate.Dtls + Output.Udp",
        "Terminate.Https + Output.Http1",
        "Terminate.Https + Output.Http2",
    ] {
        assert!(message.contains(valid), "valid pairing {valid}: {message}");
    }
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

fn evaluate(script: &str) -> Option<crate::runtime::apps::ScriptError> {
    crate::runtime::apps::evaluate_script(
        &seedling_protocol::names::AppName::new_unchecked("demo"),
        script,
        &Default::default(),
        &crate::ScriptLimits::default(),
    )
    .1
}

// spec: ING#redirect-to-https
#[test]
fn a_redirect_on_an_ingress_whose_final_termination_is_not_https_fails_evaluation() {
    for script in [
        r#"app.service("web").ingress("example.com", 443).tls(Terminate.Tls, Output.Tcp).redirect();"#,
        r#"app.service("web").ingress("example.com", 443).redirect().tls(Terminate.Tls, Output.Tcp);"#,
        r#"app.service("web").ingress("example.com", 443)
            .tls(Terminate.Https, Output.Http1).redirect().tls(Terminate.Tls, Output.Tcp);"#,
        r#"app.service("web").ingress("example.com", 443).redirect();"#,
    ] {
        let err = evaluate(script).expect("the definition fails to evaluate");
        assert!(
            err.0.contains("example.com:443"),
            "names the ingress: {err}"
        );
    }
}

// spec: ING#redirect-to-https
#[test]
fn a_redirect_declared_before_the_https_termination_is_accepted() {
    assert!(
        evaluate(
            r#"app.service("web").ingress("example.com", 443).redirect().tls(Terminate.Https, Output.Http1);"#
        )
        .is_none()
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
        traffic.ingress("test2.localhost", 8080);
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
    assert!(ingress_names.contains(&"test2.localhost:8080".to_owned()));
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

// spec: ING#redirect-to-https
// The unchecked `as u16` wrapped, so 65843 became 307, and the 3xx range check
// let 303 and 300 through: only 301, 302, 307 and 308 are the codes a redirect
// may carry.
#[test]
fn redirect_code_must_be_one_of_the_four_redirects() {
    for bad in [200, 300, 303, 304, 305, 399, 65843, -307] {
        let err = run_test_script_err(&format!(
            r#"
            app.service("web").ingress("example.com", 443)
                .tls(Terminate.Https, Output.Http1)
                .redirect(80, {bad});
        "#
        ));
        let message = err.to_string();
        assert!(
            message.contains("301, 302, 307, or 308"),
            "{bad}: {message}"
        );
    }
    for good in [301, 302, 307, 308] {
        run_test_script_app(&format!(
            r#"
            app.service("web").ingress("example.com", 443)
                .tls(Terminate.Https, Output.Http1)
                .redirect(80, {good});
        "#
        ));
    }
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

// spec: ING#identity-within-an-app
// spec: PRX#shared-ports
#[test]
fn ingresses_of_one_kind_for_different_hostnames_share_a_port() {
    for script in [
        r#"let s = app.service("web");
        s.ingress("a.example.com", 443).tls(Terminate.Tls, Output.Tcp);
        s.ingress("b.example.com", 443).tls(Terminate.Tls, Output.Tcp);"#,
        r#"let s = app.service("web");
        s.ingress("a.example.com", 443).tls(Terminate.Tls, Output.Tcp);
        s.ingress("b.example.com", 443).tls(Terminate.Https, Output.Http1);"#,
        r#"let s = app.service("web");
        s.ingress("a.example.com", 443).tls(Terminate.Https, Output.Http1).redirect();
        s.ingress("b.example.com", 443).tls(Terminate.Https, Output.Http1).redirect();"#,
    ] {
        assert!(evaluate(script).is_none(), "evaluates: {script}");
    }
}

// spec: ING#identity-within-an-app
// spec: PRX#shared-ports
#[test]
fn conflicting_claims_on_a_port_fail_evaluation_naming_both_ingresses() {
    for (script, first, second) in [
        // Pass-through takes its whole TCP port.
        (
            r#"let s = app.service("web");
            s.ingress("a.example.com", 443);
            s.ingress("b.example.com", 443).tls(Terminate.Tls, Output.Tcp);"#,
            "a.example.com:443",
            "b.example.com:443",
        ),
        // DTLS takes its whole UDP port, which HTTP/3 on the same port needs.
        (
            r#"let s = app.service("web");
            s.ingress("a.example.com", 443).tls(Terminate.Dtls, Output.Udp);
            s.ingress("b.example.com", 443).tls(Terminate.Https, Output.Http1);"#,
            "a.example.com:443",
            "b.example.com:443",
        ),
        // Two DTLS ingresses cannot be told apart by hostname.
        (
            r#"let s = app.service("web");
            s.ingress("a.example.com", 5684).tls(Terminate.Dtls, Output.Udp);
            s.ingress("b.example.com", 5684).tls(Terminate.Dtls, Output.Udp);"#,
            "a.example.com:5684",
            "b.example.com:5684",
        ),
        // Plaintext HTTP and TLS are different kinds of claim on a TCP port.
        (
            r#"let s = app.service("web");
            s.ingress("a.example.com", 443).tls(Terminate.Https, Output.Http1);
            s.ingress("b.example.com", 80).tls(Terminate.Https, Output.Http1).redirect(443);"#,
            "a.example.com:443",
            "b.example.com:80",
        ),
    ] {
        let err = evaluate(script).expect("the definition fails to evaluate");
        assert!(
            err.0.contains(first) && err.0.contains(second),
            "names both ingresses: {err}"
        );
    }
}

// spec: ING#identity-within-an-app
#[test]
fn a_pass_through_ingress_and_a_dtls_ingress_claim_different_transports() {
    assert!(
        evaluate(
            r#"let s = app.service("web");
            s.ingress("a.example.com", 5684);
            s.ingress("a.example.com", 5684).tls(Terminate.Dtls, Output.Udp);"#
        )
        .is_some_and(|e| e.0.contains("ingress conflict")),
        "the same hostname and port is one ingress, whatever its termination"
    );
    assert!(
        evaluate(
            r#"let s = app.service("web");
            s.ingress("a.example.com", 5684);
            s.ingress("b.example.com", 5684).tls(Terminate.Dtls, Output.Udp);"#
        )
        .is_none(),
        "TCP pass-through and DTLS on UDP are claims of different transports"
    );
}

// spec: ING#identity-within-an-app
// spec: PRX#shared-ports
#[test]
fn a_redirect_on_the_ingresses_own_port_fails_evaluation() {
    let err = evaluate(
        r#"app.service("web").ingress("example.com", 443).tls(Terminate.Https, Output.Http1).redirect(443);"#,
    )
    .expect("the definition fails to evaluate");
    assert!(
        err.0.contains("example.com:443"),
        "names the ingress: {err}"
    );
}
