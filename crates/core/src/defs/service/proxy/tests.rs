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

// l[verify service.http.balance]
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

// l[verify service.http.balance]
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

// l[verify service.http.balance]
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

// l[verify service.http.balance]
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

// l[verify service.http.balance]
#[test]
fn balance_allows_zero_interval_when_retrying_is_off() {
    let m = map(vec![
        ("try_duration", Dynamic::from(0_i64)),
        ("interval", Dynamic::from(0_i64)),
    ]);
    assert!(parse_balance(m).is_ok());
}
