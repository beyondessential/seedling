use super::*;

/// Evaluate `source` in the static context.
fn eval(source: &str) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
    let (engine, mut scope, _app) = setup(&crate::ScriptLimits::default());
    engine.eval_with_scope(&mut scope, source)
}

fn truth(source: &str) -> bool {
    eval(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .as_bool()
        .unwrap_or_else(|t| panic!("{source}: expected a boolean, got {t}"))
}

fn text(source: &str) -> String {
    eval(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .into_string()
        .unwrap_or_else(|t| panic!("{source}: expected a string, got {t}"))
}

fn int(source: &str) -> rhai::INT {
    eval(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .as_int()
        .unwrap_or_else(|t| panic!("{source}: expected an integer, got {t}"))
}

/// The reason a script error gives.
fn refusal(source: &str) -> String {
    match *eval(source).expect_err(source) {
        rhai::EvalAltResult::ErrorRuntime(ref reason, _) => reason.to_string(),
        rhai::EvalAltResult::ErrorInFunctionCall(_, _, ref inner, _) => match **inner {
            rhai::EvalAltResult::ErrorRuntime(ref reason, _) => reason.to_string(),
            ref other => panic!("{source}: expected a script error, got {other:?}"),
        },
        other => panic!("{source}: expected a script error, got {other:?}"),
    }
}

fn v(value: &str) -> String {
    format!("std::parse_version({value:?})")
}

fn sv(value: &str) -> String {
    format!("std::parse_version_semver({value:?})")
}

// spec: VER#generic-versions
#[test]
fn version_accepts_printable_ascii_with_a_digit() {
    for value in [
        "16.4",
        "16.4-bookworm",
        "1.0a",
        "sha-3f2a1c",
        "v2.71.3",
        "1:2.3~rc1+dfsg",
    ] {
        assert_eq!(text(&format!("{}.to_string()", v(value))), value);
    }
}

// spec: VER#generic-versions
#[test]
fn version_refuses_a_value_without_a_digit() {
    for value in ["latest", "stable", "v", ""] {
        let reason = refusal(&v(value));
        assert!(reason.contains("holds no digit"), "{value}: {reason}");
    }
}

// spec: VER#generic-versions
#[test]
fn version_refuses_space_and_non_ascii() {
    for value in ["1.0 beta", "1.0ā", "１.0"] {
        let reason = refusal(&v(value));
        assert!(reason.contains("printable ASCII"), "{value}: {reason}");
    }
}

// spec: VER#generic-versions
#[test]
fn version_refuses_a_number_too_large_for_an_integer() {
    let reason = refusal(&v("1.99999999999999999999"));
    assert!(reason.contains("too large"), "{reason}");
    assert_eq!(
        int(&format!("{}.segments()[1]", v("1.9223372036854775807"))),
        rhai::INT::MAX
    );
}

// spec: VER#generic-versions
#[test]
fn versions_order_as_libversion_orders_them() {
    let ascending = [
        "1.0alpha1",
        "1.0beta1",
        "1.0",
        "1.0patch1",
        "1.0.1",
        "1.0a",
        "1.0b",
        "1.1",
        "1.2",
        "1.10",
    ];
    for pair in ascending.windows(2) {
        let (a, b) = (v(pair[0]), v(pair[1]));
        assert!(truth(&format!("{a} < {b}")), "{} < {}", pair[0], pair[1]);
        assert!(truth(&format!("{a} <= {b}")), "{} <= {}", pair[0], pair[1]);
        assert!(truth(&format!("{b} > {a}")), "{} > {}", pair[1], pair[0]);
        assert!(truth(&format!("{b} >= {a}")), "{} >= {}", pair[1], pair[0]);
        assert!(truth(&format!("{a} != {b}")), "{} != {}", pair[0], pair[1]);
        assert!(!truth(&format!("{a} == {b}")), "{} == {}", pair[0], pair[1]);
    }
}

// spec: VER#generic-versions
#[test]
fn versions_equal_under_libversion_are_equal() {
    for (a, b) in [
        ("1.0", "1.0.0"),
        ("1.0alpha1", "1.0a1"),
        ("1.01", "1.1"),
        ("1-0", "1.0"),
        ("v2.0", "2.0"),
        ("V2.0", "2.0"),
    ] {
        assert!(truth(&format!("{} == {}", v(a), v(b))), "{a} == {b}");
        assert!(!truth(&format!("{} != {}", v(a), v(b))), "{a} != {b}");
    }
}

// spec: VER#generic-versions
#[test]
fn a_leading_v_before_a_digit_is_disregarded_in_ordering() {
    assert!(truth(&format!("{} > {}", v("v2.0"), v("1.0"))));
    assert!(truth(&format!("{} > {}", v("v2.0"), v("0.1"))));
    // Only a `v` directly before a digit is a prefix; `ver` is a word.
    assert!(truth(&format!("{} < {}", v("ver2.0"), v("0.1"))));
}

// spec: VER#generic-versions
#[test]
fn segments_split_runs_of_digits_and_letters() {
    let segments = eval(&format!("{}.segments()", v("v1.0a1")))
        .expect("segments")
        .into_array()
        .expect("an array");
    assert_eq!(segments.len(), 4);
    assert_eq!(segments[0].as_int().unwrap(), 1);
    assert_eq!(segments[1].as_int().unwrap(), 0);
    assert_eq!(segments[2].clone().into_string().unwrap(), "a");
    assert_eq!(segments[3].as_int().unwrap(), 1);

    assert_eq!(int(&format!("{}.segments()[1]", v("16.04"))), 4);
    assert_eq!(
        text(&format!("{}.segments()[2]", v("18.3-Bookworm"))),
        "Bookworm"
    );
    assert_eq!(text(&format!("{}.segments()[0]", v("ver2.0"))), "ver");
}

// spec: VER#semver-versions
#[test]
fn semver_accepts_a_full_version_with_an_optional_v() {
    for value in ["1.2.3", "v1.2.3", "V1.2.3-rc.1+b.5", "0.0.0"] {
        assert_eq!(text(&format!("{}.to_string()", sv(value))), value);
    }
}

// spec: VER#semver-versions
#[test]
fn semver_refuses_anything_but_a_full_version() {
    for value in [
        "2.71", "2", "vv1.2.3", "01.2.3", "1.2.3.4", "latest", "", "v",
    ] {
        let reason = refusal(&sv(value));
        assert!(
            reason.contains("is not a SemVer version"),
            "{value}: {reason}"
        );
    }
}

// spec: VER#semver-versions
#[test]
fn semver_refuses_a_number_too_large_for_an_integer() {
    let reason = refusal(&sv("1.9223372036854775808.0"));
    assert!(
        reason.contains("minor number") && reason.contains("too large"),
        "{reason}"
    );
    assert_eq!(
        int(&format!("{}.patch()", sv("1.0.9223372036854775807"))),
        rhai::INT::MAX
    );
}

// spec: VER#semver-versions
#[test]
fn semver_has_its_parts() {
    let version = sv("v2.71.3-rc.1+build.5");
    assert_eq!(int(&format!("{version}.major()")), 2);
    assert_eq!(int(&format!("{version}.minor()")), 71);
    assert_eq!(int(&format!("{version}.patch()")), 3);
    assert_eq!(text(&format!("{version}.pre_release()")), "rc.1");
    assert_eq!(text(&format!("{version}.build()")), "build.5");

    let plain = sv("2.71.3");
    assert_eq!(text(&format!("{plain}.pre_release()")), "");
    assert_eq!(text(&format!("{plain}.build()")), "");
}

// spec: VER#semver-versions
#[test]
fn semvers_order_by_precedence() {
    let ascending = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
        "2.0.0",
        "2.1.0",
        "2.1.1",
    ];
    for pair in ascending.windows(2) {
        let (a, b) = (sv(pair[0]), sv(pair[1]));
        assert!(truth(&format!("{a} < {b}")), "{} < {}", pair[0], pair[1]);
        assert!(truth(&format!("{a} <= {b}")), "{} <= {}", pair[0], pair[1]);
        assert!(truth(&format!("{b} > {a}")), "{} > {}", pair[1], pair[0]);
        assert!(truth(&format!("{b} >= {a}")), "{} >= {}", pair[1], pair[0]);
        assert!(truth(&format!("{a} != {b}")), "{} != {}", pair[0], pair[1]);
        assert!(!truth(&format!("{a} == {b}")), "{} == {}", pair[0], pair[1]);
    }
}

// spec: VER#semver-versions
#[test]
fn build_metadata_does_not_affect_precedence() {
    let (a, b) = (sv("1.0.0+a"), sv("1.0.0+b"));
    assert!(truth(&format!("{a} == {b}")));
    assert!(!truth(&format!("{a} < {b}")));
    assert!(!truth(&format!("{a} > {b}")));
    assert!(truth(&format!("{} == {}", sv("v1.0.0"), sv("1.0.0"))));
}

// spec: VER#semver-versions
// spec: BND#seedling-version-requirement
#[test]
fn semver_matches_a_seedling_version_requirement() {
    for (version, requirement, expected) in [
        ("2.71.3", "~2.71", true),
        ("v2.71.3", "~2.71", true),
        ("2.72.0", "~2.71", false),
        ("2.70.9", "~2.71", false),
        ("0.14.2", ">=0.13, <0.15", true),
        ("1.4.0", ">=0.13, <0.15 || ^1.2", true),
        ("1.1.0", ">=0.13, <0.15 || ^1.2", false),
        ("2.71.4-rc.1", "~2.71", false),
        ("2.71.4-rc.1", ">=2.71.0", false),
        ("2.71.4-rc.1", ">=2.71.4-rc.0", true),
        ("2.71.3+build.5", "=2.71.3", true),
    ] {
        assert_eq!(
            truth(&format!("{}.matches({requirement:?})", sv(version))),
            expected,
            "{version} against {requirement}",
        );
    }
}

// spec: VER#semver-versions
#[test]
fn matches_refuses_an_invalid_requirement() {
    for requirement in ["", "not a version", ">=0.13 ||"] {
        let reason = refusal(&format!("{}.matches({requirement:?})", sv("1.0.0")));
        assert!(
            reason.contains("version requirement"),
            "{requirement}: {reason}"
        );
    }
    let reason = refusal(&format!("{}.matches(2)", sv("1.0.0")));
    assert_eq!(reason, "`requirement` must be a string, got i64");
}

// spec: VER#text-of-a-version
#[test]
fn versions_print_as_written() {
    for source in [v("v16.04-bookworm"), sv("V2.71.3+b")] {
        let written = source.split('"').nth(1).unwrap().to_owned();
        assert_eq!(text(&format!("{source}.to_string()")), written);
        assert_eq!(text(&format!("let x = {source}; `${{x}}`")), written);
        assert_eq!(text(&format!("let x = {source}; \"\" + x")), written);
    }
}

// spec: VER#mixing-version-types
#[test]
fn comparing_with_another_type_raises() {
    let version = v("1.0.0");
    let semver = sv("1.0.0");
    for op in ["==", "!=", "<", "<=", ">", ">="] {
        for (left, right) in [
            (version.as_str(), semver.as_str()),
            (semver.as_str(), version.as_str()),
            (version.as_str(), "\"1.0.0\""),
            ("\"1.0.0\"", version.as_str()),
            (semver.as_str(), "\"1.0.0\""),
            ("\"1.0.0\"", semver.as_str()),
            (version.as_str(), "1"),
            ("1", semver.as_str()),
            (semver.as_str(), "()"),
        ] {
            let source = format!("{left} {op} {right}");
            let reason = refusal(&source);
            assert!(reason.contains("cannot compare"), "{source}: {reason}");
        }
    }
    assert_eq!(
        refusal(&format!("{version} == {semver}")),
        "cannot compare Version == SemVer: a version compares only with a version of its own type"
    );
}

// spec: VER#generic-versions
// spec: VER#semver-versions
#[test]
fn versions_sort_with_a_comparison_closure() {
    let sorted = text(&format!(
        r#"let xs = [{}, {}, {}]; xs.sort(|a, b| a <= b); `${{xs[0]}} ${{xs[1]}} ${{xs[2]}}`"#,
        v("1.10"),
        v("v1.2"),
        v("1.9"),
    ));
    assert_eq!(sorted, "v1.2 1.9 1.10");
}
