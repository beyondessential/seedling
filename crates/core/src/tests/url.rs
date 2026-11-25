use super::*;

/// Evaluate `source` in the static context.
fn eval(source: &str) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
    let (engine, mut scope, _app) = setup(&crate::ScriptLimits::default());
    engine.eval_with_scope(&mut scope, source)
}

fn call(helper: &str, value: &str) -> String {
    format!("std::{helper}({value:?})")
}

/// What `helper` returns for `value`, each entry as a string, sorted by key.
fn texts(helper: &str, value: &str) -> Vec<(String, String)> {
    let mut texts: Vec<_> = eval(&call(helper, value))
        .unwrap_or_else(|e| panic!("{value} should be accepted, got {e}"))
        .try_cast::<rhai::Map>()
        .unwrap_or_else(|| panic!("{value}: expected a map"))
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    texts.sort();
    texts
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs: Vec<_> = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    pairs.sort();
    pairs
}

/// The reason `helper` gives for refusing `value`.
fn refusal(helper: &str, value: &str) -> String {
    match *eval(&call(helper, value)).expect_err(value) {
        rhai::EvalAltResult::ErrorRuntime(ref reason, _) => reason.to_string(),
        other => panic!("{value}: expected a script error, got {other:?}"),
    }
}

fn hostname(value: &str) -> String {
    eval(&call("parse_hostname", value))
        .unwrap_or_else(|e| panic!("{value} should be accepted, got {e}"))
        .into_string()
        .expect("a string result")
}

fn url_host(value: &str) -> Option<String> {
    texts("parse_url", value)
        .into_iter()
        .find_map(|(key, host)| (key == "host").then_some(host))
}

// spec: URL#hostnames
#[test]
fn a_hostname_comes_back_as_ascii_in_lower_case() {
    assert_eq!(hostname("Example.ORG"), "example.org");
    assert_eq!(hostname("bücher.de"), "xn--bcher-kva.de");
    assert_eq!(hostname("BÜCHER.de"), "xn--bcher-kva.de");
    assert_eq!(hostname("a-b.c9"), "a-b.c9");
}

// spec: URL#hostnames
#[test]
fn a_hostname_outside_the_ingress_rules_is_refused() {
    let long_label = "a".repeat(64);
    for value in [
        "",
        "a..b",
        "*.example.org",
        "-a.example.org",
        "a-.example.org",
        "a_b.example.org",
        &format!("{long_label}.example.org"),
        "[::1]",
        "example.org:443",
    ] {
        refusal("parse_hostname", value);
    }
}

// spec: URL#hostnames
#[test]
fn a_hostname_ending_in_a_number_is_refused() {
    for value in ["10.0.0.5", "app.0x1f", "app.1", "0x7f.1", "2130706433"] {
        let reason = refusal("parse_hostname", value);
        assert!(reason.contains("ends in a number"), "{value}: {reason}");
    }
}

// spec: URL#components
#[test]
fn a_url_parses_into_its_components() {
    assert_eq!(
        texts(
            "parse_url",
            "HTTPS://u%40x:p%3Aq@Example.org:8443/a/b?x=1&x=2#f"
        ),
        pairs(&[
            ("scheme", "https"),
            ("host", "example.org"),
            ("port", "8443"),
            ("path", "/a/b"),
            ("query", "x=1&x=2"),
            ("fragment", "f"),
            ("username", "u@x"),
            ("password", "p:q"),
        ])
    );
}

// spec: URL#components
// spec: URL#ports
#[test]
fn a_url_leaves_out_what_it_does_not_have() {
    assert_eq!(
        texts("parse_url", "https://example.org"),
        pairs(&[
            ("scheme", "https"),
            ("host", "example.org"),
            ("port", "443"),
            ("path", "/"),
        ])
    );
    assert_eq!(
        texts("parse_url", "https://h/?#"),
        pairs(&[
            ("scheme", "https"),
            ("host", "h"),
            ("port", "443"),
            ("path", "/"),
        ])
    );
    assert_eq!(
        texts("parse_url", "postgres://:@db/app"),
        pairs(&[("scheme", "postgres"), ("host", "db"), ("path", "/app")])
    );
}

// spec: URL#urls
#[test]
fn a_url_without_a_host_is_accepted() {
    assert_eq!(
        texts("parse_url", "file:///etc/hosts"),
        pairs(&[("scheme", "file"), ("path", "/etc/hosts")])
    );
    assert_eq!(
        texts("parse_url", "mailto:someone@example.org"),
        pairs(&[("scheme", "mailto"), ("path", "someone@example.org")])
    );
}

// spec: URL#components
#[test]
fn a_url_keeps_its_path_query_and_fragment_encoded() {
    assert_eq!(
        texts("parse_url", "foo://h/a%2Fb?c=%2F#%2F"),
        pairs(&[
            ("scheme", "foo"),
            ("host", "h"),
            ("path", "/a%2Fb"),
            ("query", "c=%2F"),
            ("fragment", "%2F"),
        ])
    );
}

// spec: URL#hosts
#[test]
fn an_opaque_host_is_read_as_a_host_where_it_can_be() {
    assert_eq!(
        url_host("postgres://DB.Example.org/x").as_deref(),
        Some("db.example.org")
    );
    assert_eq!(url_host("postgres://0x7f.1/").as_deref(), Some("127.0.0.1"));
    assert_eq!(
        url_host("postgres://bücher.de/").as_deref(),
        Some("xn--bcher-kva.de")
    );
    assert_eq!(url_host("foo://a%zz/").as_deref(), Some("a%zz"));
    assert_eq!(url_host("https://[::1]:80/").as_deref(), Some("[::1]"));
}

// spec: URL#userinfo
#[test]
fn userinfo_that_is_not_a_single_line_of_text_is_refused() {
    for value in [
        "postgres://a%0Ab@h/",
        "postgres://u:a%E2%80%A8b@h/",
        "postgres://u:%FF@h/",
    ] {
        refusal("parse_url", value);
    }
    assert!(refusal("parse_url", "postgres://a%0Ab@h/").contains("U+000A"));
}

// spec: URL#ports
#[test]
fn a_url_with_port_zero_or_no_valid_form_is_refused() {
    for value in [
        "https://h:0/",
        "postgres://h:0/",
        "not a url",
        "https://h:99999/",
        "https://",
    ] {
        refusal("parse_url", value);
    }
}

// spec: URL#url-origins
#[test]
fn a_url_origin_gives_its_scheme_host_and_port() {
    let origin = |scheme, host, port| pairs(&[("scheme", scheme), ("host", host), ("port", port)]);
    assert_eq!(
        texts("parse_url_origin", "https://Example.org"),
        origin("https", "example.org", "443")
    );
    assert_eq!(
        texts("parse_url_origin", "http://example.org:8080/"),
        origin("http", "example.org", "8080")
    );
    assert_eq!(
        texts("parse_url_origin", "https://bücher.de:443/"),
        origin("https", "xn--bcher-kva.de", "443")
    );
}

// spec: URL#url-origins
#[test]
fn a_url_origin_naming_more_than_an_origin_is_refused() {
    for value in [
        "ftp://example.org/",
        "file:///etc/hosts",
        "https://@example.org/",
        "https://:@example.org/",
        "https://u@example.org/",
        "https:@example.org",
        "https:\\\\@example.org",
        "https://example.org/?",
        "https://example.org/#",
        "https://example.org/a",
        "https://example.org//",
        "https://10.0.0.5/",
        "https://[::1]/",
        "https://app.0x1f/",
        "https://h:0/",
        "https://*.example.org/",
    ] {
        refusal("parse_url_origin", value);
    }
}

// spec: URL#url-origins
#[test]
fn a_url_origin_is_refused_for_its_shape_before_its_parts() {
    for (value, reason) in [
        ("https://u:%FF@example.org/", "`@`"),
        ("https://u%0A@example.org/", "`@`"),
        ("ftp://example.org:0/", "http or https"),
    ] {
        let refused = refusal("parse_url_origin", value);
        assert!(refused.contains(reason), "{value}: {refused}");
    }
}
