use super::*;

fn enabled(settings: CompressSettings) -> Option<CompressDecl> {
    Some(CompressDecl::Enabled(settings))
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn unset_everywhere_resolves_to_defaults() {
    let r = resolve(&ProxySettings::default(), None);
    let c = r.compress.expect("compression is on by default");
    assert_eq!(c.encodings, default_encodings());
    assert_eq!(c.minimum_length, DEFAULT_MINIMUM_LENGTH);
    assert_eq!(c.content_types, None);
    assert_eq!(r.balance.policy, LbPolicy::RoundRobin);
    assert_eq!(r.balance.try_duration_secs, DEFAULT_TRY_DURATION_SECS);
    assert_eq!(r.balance.interval_secs, DEFAULT_INTERVAL_SECS);
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn route_overrides_only_the_fields_it_names() {
    let service = ProxySettings {
        compress: enabled(CompressSettings {
            minimum_length: Some(1024),
            ..Default::default()
        }),
        balance: BalanceSettings {
            policy: Some(LbPolicy::LeastConn),
            ..Default::default()
        },
        rate_limit: None,
        headers: Default::default(),
    };
    let route = ProxySettings {
        balance: BalanceSettings {
            try_duration_secs: Some(10.0),
            ..Default::default()
        },
        ..Default::default()
    };

    let r = resolve(&service, Some(&route));
    // The route said nothing about policy, so the service's choice stands.
    assert_eq!(r.balance.policy, LbPolicy::LeastConn);
    assert_eq!(r.balance.try_duration_secs, 10.0);
    assert_eq!(r.balance.interval_secs, DEFAULT_INTERVAL_SECS);
    // Setting balance left compression alone.
    assert_eq!(r.compress.expect("still on").minimum_length, 1024);
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn service_values_apply_when_route_declares_nothing() {
    let service = ProxySettings {
        compress: enabled(CompressSettings {
            encodings: Some(vec![Encoding::Gzip]),
            ..Default::default()
        }),
        balance: BalanceSettings {
            interval_secs: Some(1.0),
            ..Default::default()
        },
        rate_limit: None,
        headers: Default::default(),
    };
    let r = resolve(&service, Some(&ProxySettings::default()));
    assert_eq!(r.compress.expect("on").encodings, vec![Encoding::Gzip]);
    assert_eq!(r.balance.interval_secs, 1.0);
}

// l[verify service.http.compress]
#[test]
fn route_can_switch_compression_off() {
    let service = ProxySettings {
        compress: enabled(CompressSettings::default()),
        ..Default::default()
    };
    let route = ProxySettings {
        compress: Some(CompressDecl::Disabled),
        ..Default::default()
    };
    assert!(resolve(&service, Some(&route)).compress.is_none());
}

// l[verify service.http.compress]
#[test]
fn route_can_switch_compression_back_on_over_a_disabling_service() {
    let service = ProxySettings {
        compress: Some(CompressDecl::Disabled),
        ..Default::default()
    };
    let route = ProxySettings {
        compress: enabled(CompressSettings::default()),
        ..Default::default()
    };
    let c = resolve(&service, Some(&route)).compress.expect("back on");
    // The disabling service carried no field values to inherit.
    assert_eq!(c.minimum_length, DEFAULT_MINIMUM_LENGTH);
}

// l[verify service.http.compress]
#[test]
fn service_level_disable_covers_a_silent_route() {
    let service = ProxySettings {
        compress: Some(CompressDecl::Disabled),
        ..Default::default()
    };
    assert!(
        resolve(&service, Some(&ProxySettings::default()))
            .compress
            .is_none()
    );
}

// l[verify service.balance]
#[test]
fn zero_try_duration_survives_resolution() {
    let service = ProxySettings {
        balance: BalanceSettings {
            try_duration_secs: Some(0.0),
            interval_secs: Some(0.0),
            ..Default::default()
        },
        ..Default::default()
    };
    let r = resolve(&service, None);
    assert_eq!(r.balance.try_duration_secs, 0.0);
    assert_eq!(r.balance.interval_secs, 0.0);
}

// l[verify service.balance]
#[test]
fn zero_interval_meeting_a_nonzero_duration_across_levels_is_not_emitted() {
    // Neither level is invalid on its own, so parsing cannot catch this.
    let service = ProxySettings {
        balance: BalanceSettings {
            try_duration_secs: Some(0.0),
            interval_secs: Some(0.0),
            ..Default::default()
        },
        ..Default::default()
    };
    let route = ProxySettings {
        balance: BalanceSettings {
            try_duration_secs: Some(10.0),
            ..Default::default()
        },
        ..Default::default()
    };
    let r = resolve(&service, Some(&route));
    assert_eq!(r.balance.try_duration_secs, 10.0);
    assert_eq!(r.balance.interval_secs, DEFAULT_INTERVAL_SECS);
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn map(pairs: Vec<(&str, Dynamic)>) -> Map {
    pairs.into_iter().map(|(k, v)| (k.into(), v)).collect()
}

// l[verify service.http.compress.fields]
#[test]
fn compress_fields_parse() {
    let m = map(vec![
        (
            "encodings",
            Dynamic::from(rhai::Array::from(vec![Dynamic::from("gzip".to_string())])),
        ),
        ("minimum_length", Dynamic::from(1024_i64)),
        (
            "content_types",
            Dynamic::from(rhai::Array::from(vec![Dynamic::from("text/*".to_string())])),
        ),
    ]);
    let parsed = parse_compress(m).expect("valid");
    assert_eq!(parsed.encodings, Some(vec![Encoding::Gzip]));
    assert_eq!(parsed.minimum_length, Some(1024));
    assert_eq!(parsed.content_types, Some(vec!["text/*".to_string()]));
}

// l[verify service.http.compress.fields]
#[test]
fn compress_rejects_bad_values() {
    let unknown_encoding = map(vec![(
        "encodings",
        Dynamic::from(rhai::Array::from(vec![Dynamic::from("br".to_string())])),
    )]);
    assert!(parse_compress(unknown_encoding).is_err());

    let empty_encodings = map(vec![("encodings", Dynamic::from(rhai::Array::new()))]);
    assert!(parse_compress(empty_encodings).is_err());

    let empty_types = map(vec![("content_types", Dynamic::from(rhai::Array::new()))]);
    assert!(parse_compress(empty_types).is_err());

    let negative = map(vec![("minimum_length", Dynamic::from(-1_i64))]);
    assert!(parse_compress(negative).is_err());

    let unknown_field = map(vec![("min_length", Dynamic::from(1_i64))]);
    assert!(parse_compress(unknown_field).is_err());
}

// l[verify service.balance]
#[test]
fn balance_fields_parse_ints_and_floats() {
    let m = map(vec![
        ("policy", Dynamic::from("least_conn".to_string())),
        ("try_duration", Dynamic::from(10_i64)),
        ("interval", Dynamic::from(0.5_f64)),
    ]);
    let parsed = parse_balance(m).expect("valid");
    assert_eq!(parsed.policy, Some(LbPolicy::LeastConn));
    assert_eq!(parsed.try_duration_secs, Some(10.0));
    assert_eq!(parsed.interval_secs, Some(0.5));
}

// l[verify service.balance]
#[test]
fn balance_rejects_bad_values() {
    let bad_policy = map(vec![("policy", Dynamic::from("sticky".to_string()))]);
    assert!(parse_balance(bad_policy).is_err());

    let negative = map(vec![("try_duration", Dynamic::from(-1_i64))]);
    assert!(parse_balance(negative).is_err());

    // Zero interval with the try duration left at its non-zero default.
    let spin = map(vec![("interval", Dynamic::from(0_i64))]);
    assert!(parse_balance(spin).is_err());

    let unknown_field = map(vec![("policy_name", Dynamic::from("first".to_string()))]);
    assert!(parse_balance(unknown_field).is_err());
}

// l[verify service.balance]
#[test]
fn balance_allows_zero_interval_when_retrying_is_off() {
    let m = map(vec![
        ("try_duration", Dynamic::from(0_i64)),
        ("interval", Dynamic::from(0_i64)),
    ]);
    assert!(parse_balance(m).is_ok());
}

fn limit(max_events: u64, window_secs: f64) -> Option<RateLimitDecl> {
    Some(RateLimitDecl::Enabled(RateLimitSettings {
        max_events,
        window_secs,
    }))
}

// l[verify service.http.rate-limit]
#[test]
fn unset_everywhere_is_not_rate_limited() {
    assert_eq!(resolve(&ProxySettings::default(), None).rate_limit, None);
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn service_rate_limit_applies_to_a_route_declaring_none() {
    let service = ProxySettings {
        rate_limit: limit(1000, 1.0),
        ..Default::default()
    };
    let r = resolve(&service, Some(&ProxySettings::default()));
    let rl = r
        .rate_limit
        .expect("the service's limit carries to the route");
    assert_eq!(rl.settings.max_events, 1000);
    assert_eq!(rl.settings.window_secs, 1.0);
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn route_rate_limit_replaces_the_services_outright() {
    let service = ProxySettings {
        rate_limit: limit(1000, 1.0),
        ..Default::default()
    };
    let route = ProxySettings {
        rate_limit: limit(10, 60.0),
        ..Default::default()
    };
    let rl = resolve(&service, Some(&route))
        .rate_limit
        .expect("the route's limit governs");
    assert_eq!(rl.scope, RateLimitScope::Route);
    // Whole-unit, not field-by-field: the window comes from the route too,
    // rather than being left at the service's 1s.
    assert_eq!(rl.settings.max_events, 10);
    assert_eq!(rl.settings.window_secs, 60.0);
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn route_can_switch_off_a_limit_the_service_declared() {
    let service = ProxySettings {
        rate_limit: limit(1000, 1.0),
        ..Default::default()
    };
    let route = ProxySettings {
        rate_limit: Some(RateLimitDecl::Disabled),
        ..Default::default()
    };
    assert_eq!(resolve(&service, Some(&route)).rate_limit, None);
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn rate_limit_does_not_disturb_compression_or_balancing() {
    let service = ProxySettings {
        rate_limit: limit(1000, 1.0),
        ..Default::default()
    };
    let r = resolve(&service, None);
    assert!(r.compress.is_some(), "compression stays on by default");
    assert_eq!(r.balance.policy, LbPolicy::RoundRobin);
}

// l[verify service.http.rate-limit.fields]
#[test]
fn rate_limit_parses_ints_and_floats() {
    let m = map(vec![
        ("max_events", Dynamic::from(10_i64)),
        ("window", Dynamic::from(1_i64)),
    ]);
    let parsed = parse_rate_limit(m).expect("valid");
    assert_eq!(parsed.max_events, 10);
    assert_eq!(parsed.window_secs, 1.0);

    let fractional = map(vec![
        ("max_events", Dynamic::from(5_i64)),
        ("window", Dynamic::from(0.5_f64)),
    ]);
    assert_eq!(
        parse_rate_limit(fractional).expect("valid").window_secs,
        0.5
    );
}

// l[verify service.http.rate-limit.fields]
#[test]
fn rate_limit_requires_both_fields() {
    let no_window = map(vec![("max_events", Dynamic::from(10_i64))]);
    assert!(parse_rate_limit(no_window).is_err());

    let no_max = map(vec![("window", Dynamic::from(1_i64))]);
    assert!(parse_rate_limit(no_max).is_err());

    assert!(parse_rate_limit(map(vec![])).is_err());
}

// l[verify service.http.rate-limit.fields]
#[test]
fn rate_limit_rejects_bad_values() {
    let zero_events = map(vec![
        ("max_events", Dynamic::from(0_i64)),
        ("window", Dynamic::from(1_i64)),
    ]);
    assert!(parse_rate_limit(zero_events).is_err());

    let negative_events = map(vec![
        ("max_events", Dynamic::from(-1_i64)),
        ("window", Dynamic::from(1_i64)),
    ]);
    assert!(parse_rate_limit(negative_events).is_err());

    let zero_window = map(vec![
        ("max_events", Dynamic::from(10_i64)),
        ("window", Dynamic::from(0_i64)),
    ]);
    assert!(parse_rate_limit(zero_window).is_err());

    let negative_window = map(vec![
        ("max_events", Dynamic::from(10_i64)),
        ("window", Dynamic::from(-1.0_f64)),
    ]);
    assert!(parse_rate_limit(negative_window).is_err());

    let infinite_window = map(vec![
        ("max_events", Dynamic::from(10_i64)),
        ("window", Dynamic::from(f64::INFINITY)),
    ]);
    assert!(parse_rate_limit(infinite_window).is_err());

    let unknown_field = map(vec![
        ("max_events", Dynamic::from(10_i64)),
        ("window", Dynamic::from(1_i64)),
        ("per", Dynamic::from("ip".to_string())),
    ]);
    assert!(parse_rate_limit(unknown_field).is_err());
}

// l[verify service.http.rate-limit.fields]
#[test]
fn rate_limit_rejects_out_of_range_values() {
    let too_many = map(vec![
        ("max_events", Dynamic::from(MAX_MAX_EVENTS as i64 + 1)),
        ("window", Dynamic::from(1_i64)),
    ]);
    assert!(parse_rate_limit(too_many).is_err());

    // Below the floor: not a rate limit any app could mean.
    let vanishing = map(vec![
        ("max_events", Dynamic::from(10_i64)),
        ("window", Dynamic::from(0.000_000_000_1_f64)),
    ]);
    assert!(parse_rate_limit(vanishing).is_err());

    // Above the ceiling.
    let geological = map(vec![
        ("max_events", Dynamic::from(10_i64)),
        ("window", Dynamic::from(1e12_f64)),
    ]);
    assert!(parse_rate_limit(geological).is_err());
}

// l[verify service.http.rate-limit.fields]
#[test]
fn rate_limit_accepts_the_bounds_themselves() {
    let at_bounds = map(vec![
        ("max_events", Dynamic::from(MAX_MAX_EVENTS as i64)),
        ("window", Dynamic::from(MAX_WINDOW_SECS)),
    ]);
    assert!(parse_rate_limit(at_bounds).is_ok());

    let at_floor = map(vec![
        ("max_events", Dynamic::from(1_i64)),
        ("window", Dynamic::from(MIN_WINDOW_SECS)),
    ]);
    assert!(parse_rate_limit(at_floor).is_ok());
}

// l[verify service.http.rate-limit.fields]
#[test]
fn rate_limit_names_a_misspelled_key_rather_than_the_field_it_displaced() {
    let typo = map(vec![
        ("max_event", Dynamic::from(10_i64)),
        ("window", Dynamic::from(1_i64)),
    ]);
    let err = parse_rate_limit(typo).expect_err("a misspelled key must throw");
    let msg = err.to_string();
    assert!(
        msg.contains("max_event") && !msg.contains("requires"),
        "the error should name the key that was not recognised, got: {msg}"
    );
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn an_inherited_limit_stays_the_services_however_many_routes_take_it() {
    let service = ProxySettings {
        rate_limit: limit(1000, 1.0),
        ..Default::default()
    };
    // Two routes inheriting the same declaration. Both report the service as
    // the declaring level, which is what names them one budget rather than
    // one each: were it otherwise, a service declaring 1000/s and serving
    // three routes would let a client spend 3000/s against its pods, and
    // adding a fourth route would raise that again.
    for route in [ProxySettings::default(), ProxySettings::default()] {
        let rl = resolve(&service, Some(&route))
            .rate_limit
            .expect("inherited");
        assert_eq!(rl.scope, RateLimitScope::Service);
        assert_eq!(rl.settings.max_events, 1000);
    }

    // A route declaring its own is that route's budget, so it is scoped to it.
    let own = ProxySettings {
        rate_limit: limit(10, 1.0),
        ..Default::default()
    };
    let rl = resolve(&service, Some(&own)).rate_limit.expect("own");
    assert_eq!(rl.scope, RateLimitScope::Route);
}

// ---------------------------------------------------------------------------
// Header manipulation
// ---------------------------------------------------------------------------

fn headers(script: &str) -> HeaderSettings {
    let map: Map = rhai::Engine::new()
        .eval::<rhai::Dynamic>(script)
        .expect("script evaluates")
        .try_cast()
        .expect("script yields a map");
    parse_headers(map).expect("declaration is accepted")
}

fn headers_err(script: &str) -> String {
    let map: Map = rhai::Engine::new()
        .eval::<rhai::Dynamic>(script)
        .expect("script evaluates")
        .try_cast()
        .expect("script yields a map");
    parse_headers(map)
        .expect_err("declaration is refused")
        .to_string()
}

fn op(rules: &HeaderRules, name: &str) -> Option<HeaderOp> {
    rules
        .0
        .iter()
        .find(|(k, _)| k.as_str().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

// l[verify service.http.headers]
#[test]
fn both_directions_carry_their_own_operations() {
    let h = headers(
        r#"#{
            request: #{ replace: #{ "Host": "central.internal" } },
            response: #{ replace: #{ "Cache-Control": "no-store" } },
        }"#,
    );
    assert_eq!(
        op(&h.request, "host"),
        Some(HeaderOp::Replace(vec!["central.internal".into()]))
    );
    // The directions are independent: an operation declared in one does not
    // leak into the other.
    assert_eq!(op(&h.request, "cache-control"), None);
    assert_eq!(
        op(&h.response, "cache-control"),
        Some(HeaderOp::Replace(vec!["no-store".into()]))
    );
}

// l[verify service.http.headers]
#[test]
fn a_declaration_naming_no_direction_is_refused() {
    assert!(headers_err(r#"#{}"#).contains("requires `request`"));
    // A typo is reported as the invented key it is, rather than as a config
    // that named no direction at all.
    let e = headers_err(r#"#{ requests: #{ remove: ["Server"] } }"#);
    assert!(e.contains("unknown") && e.contains("requests"), "{e}");
}

// l[verify service.http.headers.fields]
#[test]
fn a_direction_naming_no_operation_is_refused() {
    let e = headers_err(r#"#{ response: #{} }"#);
    assert!(e.contains("requires `replace`, `add`, or `remove`"), "{e}");
}

// l[verify service.http.headers.fields]
#[test]
fn a_value_is_a_string_or_an_array_of_them() {
    // Several values is the case `Set-Cookie` needs, since several cookies
    // cannot be folded into one header.
    let h = headers(
        r#"#{ response: #{ add: #{
            "Set-Cookie": ["a=1", "b=2"],
            "X-One": "just-one",
        } } }"#,
    );
    assert_eq!(
        op(&h.response, "set-cookie"),
        Some(HeaderOp::Add(vec!["a=1".into(), "b=2".into()]))
    );
    // A bare string is the one-element case of the same thing, so the two
    // forms produce the same shape rather than two kinds of value.
    assert_eq!(
        op(&h.response, "x-one"),
        Some(HeaderOp::Add(vec!["just-one".into()]))
    );
}

// l[verify service.http.headers.fields]
#[test]
fn an_empty_array_is_refused_rather_than_read_as_a_removal() {
    let e = headers_err(r#"#{ response: #{ replace: #{ "X-Thing": [] } } }"#);
    assert!(e.contains("use `remove`"), "{e}");
    let e = headers_err(r#"#{ response: #{ remove: [] } }"#);
    assert!(e.contains("must not be empty"), "{e}");
}

// l[verify service.http.headers.fields]
#[test]
fn a_value_carrying_a_line_ending_is_refused() {
    // Otherwise the value ends its own header and begins another of the
    // declaration's choosing, which is header injection by declaration.
    for value in [r#""a\r\nX-Evil: yes""#, r#""a\nX-Evil: yes""#] {
        let e = headers_err(&format!(
            r#"#{{ response: #{{ replace: #{{ "X-Thing": {value} }} }} }}"#
        ));
        assert!(e.contains("carriage return or line feed"), "{e}");
    }
}

// l[verify service.http.headers.fields]
#[test]
fn a_name_outside_the_http_token_characters_is_refused() {
    for name in ["X Thing", "X:Thing", "X\u{e9}"] {
        let e = headers_err(&format!(r#"#{{ response: #{{ remove: ["{name}"] }} }}"#));
        assert!(e.contains("not valid in an HTTP field name"), "{name}: {e}");
    }
    assert!(headers_err(r#"#{ response: #{ remove: [""] } }"#).contains("must not be empty"));
}

// l[verify service.http.headers.fields]
#[test]
fn the_headers_the_proxy_owns_are_refused() {
    // Keep-alive is the one an app would legitimately reach for, and is
    // served without being asked for; the rest would corrupt the exchange.
    for name in [
        "Connection",
        "keep-alive",
        "Transfer-Encoding",
        "Upgrade",
        "TE",
        "Trailer",
        "Proxy-Authenticate",
        "Proxy-Authorization",
        "Content-Length",
    ] {
        let e = headers_err(&format!(
            r#"#{{ response: #{{ replace: #{{ "{name}": "x" }} }} }}"#
        ));
        assert!(e.contains("belongs to the proxy"), "{name}: {e}");
    }
}

// l[verify service.http.headers.fields]
#[test]
fn one_name_may_not_carry_two_operations() {
    let e = headers_err(r#"#{ response: #{ replace: #{ "X-A": "1" }, remove: ["X-A"] } }"#);
    assert!(e.contains("more than one operation"), "{e}");
    // Case does not make it a different header, so the contradiction is
    // caught however each was spelled.
    let e = headers_err(r#"#{ response: #{ add: #{ "X-A": "1" }, remove: ["x-a"] } }"#);
    assert!(e.contains("more than one operation"), "{e}");
}

// l[verify service.http.headers.fields]
#[test]
fn a_name_keeps_the_spelling_it_was_given() {
    let h = headers(r#"#{ response: #{ remove: ["X-Powered-By"] } }"#);
    assert_eq!(
        h.response.grouped().remove,
        vec!["X-Powered-By".to_string()]
    );
}

fn with_headers(h: HeaderSettings) -> ProxySettings {
    ProxySettings {
        headers: h,
        ..Default::default()
    }
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn a_route_overrides_only_the_headers_it_names() {
    let service = with_headers(headers(
        r#"#{ response: #{ replace: #{
            "Cache-Control": "no-store",
            "X-Service": "kept",
        } } }"#,
    ));
    let route = with_headers(headers(
        r#"#{ response: #{ replace: #{
            "Cache-Control": "public, max-age=31536000",
        } } }"#,
    ));

    let r = resolve(&service, Some(&route)).headers;
    // The header the route named takes the route's value...
    assert_eq!(
        op(&r.response, "cache-control"),
        Some(HeaderOp::Replace(vec!["public, max-age=31536000".into()]))
    );
    // ...and the one only the service named is still in force, so a route
    // does not have to restate the service's headers to add one of its own.
    assert_eq!(
        op(&r.response, "x-service"),
        Some(HeaderOp::Replace(vec!["kept".into()]))
    );
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn a_route_may_override_with_a_different_operation() {
    // The route's operation replaces the service's outright rather than
    // combining with it: the header ends up removed, not set-then-removed.
    let service = with_headers(headers(r#"#{ response: #{ add: #{ "X-A": "1" } } }"#));
    let route = with_headers(headers(r#"#{ response: #{ remove: ["X-A"] } }"#));
    let r = resolve(&service, Some(&route)).headers;
    assert_eq!(op(&r.response, "x-a"), Some(HeaderOp::Remove));
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn an_override_is_recognised_whatever_the_case() {
    // HTTP field names are case-insensitive, so a route spelling a header
    // differently from the service must still override it rather than
    // resolving to two operations for what is one header on the wire.
    let service = with_headers(headers(r#"#{ request: #{ replace: #{ "Host": "a" } } }"#));
    let route = with_headers(headers(r#"#{ request: #{ replace: #{ "host": "b" } } }"#));
    let r = resolve(&service, Some(&route)).headers;
    assert_eq!(r.request.0.len(), 1);
    assert_eq!(
        op(&r.request, "host"),
        Some(HeaderOp::Replace(vec!["b".into()]))
    );
    // The spelling reported is the one that won, not the one it displaced.
    assert_eq!(r.request.grouped().replace.keys().next().unwrap(), "host");
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn a_route_declaring_no_headers_inherits_the_services() {
    let service = with_headers(headers(r#"#{ response: #{ remove: ["Server"] } }"#));
    let r = resolve(&service, Some(&ProxySettings::default())).headers;
    assert_eq!(op(&r.response, "server"), Some(HeaderOp::Remove));
    assert_eq!(resolve(&service, None).headers, service.headers);
}

// l[verify service.http.proxy-settings.resolution]
#[test]
fn headers_do_not_disturb_the_other_settings() {
    // Each setting resolves on its own: declaring headers on a route must not
    // reset the compression or limit it inherits.
    let service = ProxySettings {
        rate_limit: limit(10, 1.0),
        compress: enabled(CompressSettings {
            minimum_length: Some(2048),
            ..Default::default()
        }),
        ..Default::default()
    };
    let route = with_headers(headers(r#"#{ response: #{ remove: ["Server"] } }"#));
    let r = resolve(&service, Some(&route));
    assert_eq!(r.rate_limit.expect("inherited").settings.max_events, 10);
    assert_eq!(r.compress.expect("inherited").minimum_length, 2048);
    assert_eq!(op(&r.headers.response, "server"), Some(HeaderOp::Remove));
}
