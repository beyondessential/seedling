use rhai::{Dynamic, Map};

use super::{HealthcheckKind, HealthcheckOnFailure, parse_healthcheck};
use crate::defs::duration::Duration;

fn map_with(entries: &[(&str, Dynamic)]) -> Map {
    let mut m = Map::new();
    for (k, v) in entries {
        m.insert((*k).into(), v.clone());
    }
    m
}

// spec: DEP#health-checks
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

// spec: DEP#command-checks
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

// spec: DEP#check-timing
// spec: DEP#failure-response
#[test]
fn parse_explicit_timings_and_on_failure_monitor() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
        ("interval", Dynamic::from("5s")),
        ("timeout", Dynamic::from("2s")),
        ("retries", Dynamic::from(4_i64)),
        ("start_period", Dynamic::from("1m")),
        ("on_failure", Dynamic::from("monitor")),
    ]);
    let hc = parse_healthcheck(m).expect("parse");
    assert_eq!(hc.interval_secs, 5);
    assert_eq!(hc.timeout_secs, 2);
    assert_eq!(hc.retries, 4);
    assert_eq!(hc.start_period_secs, 60);
    assert_eq!(hc.on_failure, HealthcheckOnFailure::Monitor);
}

// spec: DEP#failure-response
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

// spec: DEP#health-checks
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

// spec: DEP#health-checks
#[test]
fn parse_reserved_kind_is_rejected() {
    let m = map_with(&[("kind", Dynamic::from("http"))]);
    let err = parse_healthcheck(m).expect_err("http is reserved");
    assert!(err.to_string().contains("reserved for future use"));
}

// spec: DEP#health-checks
#[test]
fn parse_unknown_kind_is_rejected() {
    let m = map_with(&[("kind", Dynamic::from("shellout"))]);
    let err = parse_healthcheck(m).expect_err("unknown kind");
    assert!(err.to_string().contains("shellout"));
}

// spec: DEP#command-checks
#[test]
fn parse_missing_cmd_is_rejected() {
    let m = map_with(&[("kind", Dynamic::from("command"))]);
    let err = parse_healthcheck(m).expect_err("command kind must have cmd");
    assert!(err.to_string().contains("cmd"));
}

// spec: DEP#command-checks
#[test]
fn parse_empty_cmd_string_is_rejected() {
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(String::new())),
    ]);
    let err = parse_healthcheck(m).expect_err("empty cmd");
    assert!(err.to_string().contains("empty"));
}

// spec: DEP#failure-response
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

// spec: DEP#failure-response
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

// spec: DEP#check-timing
#[test]
fn parse_negative_interval_is_rejected() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
        ("interval", Dynamic::from("-1s")),
    ]);
    let err = parse_healthcheck(m).expect_err("negative interval");
    assert!(err.to_string().contains("no sign"), "{err}");
}

// spec: DEP#health-checks
// spec: DUR#duration-arguments
#[test]
fn a_timing_is_a_duration_in_whole_seconds() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    for (value, expected) in [
        (Dynamic::from(5_i64), "duration"),
        (Dynamic::from("2500ms"), "whole number of seconds"),
    ] {
        let m = map_with(&[
            ("kind", Dynamic::from("command")),
            ("cmd", Dynamic::from(cmd.clone())),
            ("timeout", value),
        ]);
        let err = parse_healthcheck(m).expect_err("refused");
        assert!(err.to_string().contains(expected), "{err}");
    }
    for value in [
        Dynamic::from("1m 30s"),
        Dynamic::from(Duration::from_secs(90)),
    ] {
        let m = map_with(&[
            ("kind", Dynamic::from("command")),
            ("cmd", Dynamic::from(cmd.clone())),
            ("timeout", value),
        ]);
        assert_eq!(parse_healthcheck(m).expect("taken").timeout_secs, 90);
    }
}

// spec: DEP#check-timing
#[test]
fn a_zero_interval_or_timeout_is_rejected_and_a_zero_start_period_is_not() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    for key in ["interval", "timeout"] {
        let m = map_with(&[
            ("kind", Dynamic::from("command")),
            ("cmd", Dynamic::from(cmd.clone())),
            (key, Dynamic::from("0s")),
        ]);
        let err = parse_healthcheck(m).expect_err(key);
        assert!(err.to_string().contains(key), "{key}: {err}");
    }
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
        ("start_period", Dynamic::from("0s")),
    ]);
    assert_eq!(
        parse_healthcheck(m)
            .expect("zero start_period")
            .start_period_secs,
        0
    );
}

// spec: DEP#check-timing
#[test]
fn parse_zero_retries_is_rejected() {
    let cmd: rhai::Array = vec![Dynamic::from("/bin/ok".to_string())];
    let m = map_with(&[
        ("kind", Dynamic::from("command")),
        ("cmd", Dynamic::from(cmd)),
        ("retries", Dynamic::from(0_i64)),
    ]);
    let err = parse_healthcheck(m).expect_err("zero retries");
    let message = err.to_string();
    assert!(message.contains("healthcheck retries"), "{message}");
    assert!(message.contains("must be between 1"), "{message}");
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
