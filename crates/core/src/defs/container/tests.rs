use rhai::{Dynamic, Map};

use super::{HealthcheckKind, HealthcheckOnFailure, parse_healthcheck};

fn map_with(entries: &[(&str, Dynamic)]) -> Map {
    let mut m = Map::new();
    for (k, v) in entries {
        m.insert((*k).into(), v.clone());
    }
    m
}

// l[verify deployment.healthcheck]
// l[verify deployment.healthcheck.kind]
#[test]
fn parse_command_healthcheck_with_defaults() {
    let cmd: rhai::Array = vec![
        Dynamic::from("curl".to_string()),
        Dynamic::from("-fsS".to_string()),
        Dynamic::from("http://localhost/health".to_string()),
    ];
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
    ]);

    let hc = parse_healthcheck(m).expect("parse");
    let HealthcheckKind::Command { cmd } = hc.kind;
    assert_eq!(cmd, vec!["curl", "-fsS", "http://localhost/health"]);
    assert_eq!(hc.interval_secs, 30);
    assert_eq!(hc.timeout_secs, 30);
    assert_eq!(hc.retries, 3);
    assert_eq!(hc.start_period_secs, 0);
    assert_eq!(hc.on_failure, HealthcheckOnFailure::Replace);
}

// l[verify deployment.healthcheck.command]
#[test]
fn parse_command_cmd_string_wraps_in_shell() {
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from("curl -fsS /health".to_string())),
    ]);
    let hc = parse_healthcheck(m).expect("parse");
    let HealthcheckKind::Command { cmd } = hc.kind;
    assert_eq!(
        cmd,
        vec!["CMD-SHELL".to_string(), "curl -fsS /health".into()]
    );
}

// l[verify deployment.healthcheck.timings]
// l[verify deployment.healthcheck.on-failure]
#[test]
fn parse_explicit_timings_and_on_failure_monitor() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
        ("interval", Dynamic::from(5_i64)),
        ("timeout", Dynamic::from(2_i64)),
        ("retries", Dynamic::from(4_i64)),
        ("start_period", Dynamic::from(60_i64)),
        ("on_failure", Dynamic::from("monitor")),
    ]);
    let hc = parse_healthcheck(m).expect("parse");
    assert_eq!(hc.interval_secs, 5);
    assert_eq!(hc.timeout_secs, 2);
    assert_eq!(hc.retries, 4);
    assert_eq!(hc.start_period_secs, 60);
    assert_eq!(hc.on_failure, HealthcheckOnFailure::Monitor);
}

// l[verify deployment.healthcheck.on-failure]
#[test]
fn parse_on_failure_replace_explicit() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
        ("on_failure", Dynamic::from("replace")),
    ]);
    let hc = parse_healthcheck(m).expect("parse");
    assert_eq!(hc.on_failure, HealthcheckOnFailure::Replace);
}

// l[verify deployment.healthcheck.kind]
#[test]
fn parse_missing_kind_errors() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    let m = map_with(&[("cmd", Dynamic::from(cmd))]);
    let err = parse_healthcheck(m).expect_err("must require kind");
    assert!(
        err.to_string().contains("kind"),
        "error mentions kind: {err}",
    );
}

// l[verify deployment.healthcheck.kind]
#[test]
fn parse_reserved_kind_is_rejected() {
    let m = map_with(&[("kind", Dynamic::from("http"))]);
    let err = parse_healthcheck(m).expect_err("http is reserved");
    assert!(err.to_string().contains("reserved for future use"));
}

// l[verify deployment.healthcheck.kind]
#[test]
fn parse_unknown_kind_is_rejected() {
    let m = map_with(&[("kind", Dynamic::from("shellout"))]);
    let err = parse_healthcheck(m).expect_err("unknown kind");
    assert!(err.to_string().contains("shellout"));
}

// l[verify deployment.healthcheck.command]
#[test]
fn parse_missing_cmd_is_rejected() {
    let m = map_with(&[("kind", Dynamic::from("command"))]);
    let err = parse_healthcheck(m).expect_err("command kind must have cmd");
    assert!(err.to_string().contains("cmd"));
}

// l[verify deployment.healthcheck.command]
#[test]
fn parse_empty_cmd_string_is_rejected() {
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(String::new())),
    ]);
    let err = parse_healthcheck(m).expect_err("empty cmd");
    assert!(err.to_string().contains("empty"));
}

// l[verify deployment.healthcheck.on-failure]
#[test]
fn parse_unknown_on_failure_is_rejected() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
        ("on_failure", Dynamic::from("explode")),
    ]);
    let err = parse_healthcheck(m).expect_err("explode is not valid");
    assert!(err.to_string().contains("explode"));
}

// l[verify deployment.healthcheck.on-failure]
// The previous podman-verb values are no longer valid — only replace/monitor.
#[test]
fn parse_old_podman_verb_on_failure_is_rejected() {
    for old in ["kill", "restart", "stop", "none"] {
        let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
        let m = map_with(&[
            ("kind", Dynamic::from("command")),
            ("cmd", Dynamic::from(cmd)),
            ("on_failure", Dynamic::from(old)),
        ]);
        let err = parse_healthcheck(m).expect_err("podman verb no longer valid");
        assert!(
            err.to_string().contains(old),
            "error mentions '{old}': {err}",
        );
    }
}

// l[verify deployment.healthcheck.timings]
#[test]
fn parse_negative_interval_is_rejected() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
        ("interval", Dynamic::from(-1_i64)),
    ]);
    let err = parse_healthcheck(m).expect_err("negative interval");
    assert!(err.to_string().contains("negative"));
}

// l[verify deployment.healthcheck.timings]
#[test]
fn parse_zero_retries_is_rejected() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
        ("retries", Dynamic::from(0_i64)),
    ]);
    let err = parse_healthcheck(m).expect_err("zero retries");
    assert!(err.to_string().contains("positive"));
}

#[test]
fn parse_unknown_keys_are_rejected() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
        ("surprise", Dynamic::from("oops")),
    ]);
    let err = parse_healthcheck(m).expect_err("unknown key");
    assert!(err.to_string().contains("surprise"));
}
