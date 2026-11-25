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

fn d(value: &str) -> String {
    format!("std::parse_duration({value:?})")
}

// spec: DUR#duration-syntax
#[test]
fn duration_accepts_compact_spelled_out_and_clock_forms() {
    for value in [
        "1h30m",
        "1h 30m",
        "1 hour, 30 minutes",
        "90 mins",
        "01:30:00",
        "1.5h",
        "5400s",
    ] {
        assert_eq!(int(&format!("{}.as_seconds()", d(value))), 5400, "{value}");
    }
}

// spec: DUR#duration-syntax
#[test]
fn duration_counts_a_day_as_24_hours_and_a_week_as_7_days() {
    assert_eq!(int(&format!("{}.as_seconds()", d("1d"))), 86_400);
    assert_eq!(int(&format!("{}.as_seconds()", d("2w 1d"))), 15 * 86_400);
    assert_eq!(int(&format!("{}.as_seconds()", d("1 day, 02:00:00"))), 93_600);
}

// spec: DUR#duration-syntax
#[test]
fn duration_refuses_a_sign_ago_and_calendar_units() {
    for (value, expected) in [
        ("-5s", "no sign"),
        ("+5s", "no sign"),
        ("5s ago", "no `ago`"),
        ("1mo", "years and months"),
        ("0mo 5d", "years and months"),
        ("1y", "years and months"),
        ("2 years", "years and months"),
    ] {
        let reason = refusal(&d(value));
        assert!(reason.contains(expected), "{value}: {reason}");
    }
}

// spec: DUR#duration-syntax
#[test]
fn duration_refuses_iso_8601_a_bare_number_and_the_empty_string() {
    for value in ["PT1H30M", "90", "", "5 parsecs", "1h1h"] {
        let reason = refusal(&d(value));
        assert!(reason.contains("not a duration"), "{value:?}: {reason}");
    }
}

// spec: ACN#kind-parsers
#[test]
fn duration_refuses_control_characters() {
    for value in ["1h\n30m", "1h\t30m"] {
        let reason = refusal(&d(value));
        assert!(reason.contains("single-line"), "{value:?}: {reason}");
    }
}

// spec: DUR#duration
#[test]
fn duration_refuses_a_length_past_the_longest() {
    assert_eq!(text(&format!("{}.to_string()", d("106751d 23h 47m 16s 854ms 775us 807ns"))), "106751d 23h 47m 16s 854ms 775us 807ns");
    let reason = refusal(&d("106752d"));
    assert!(reason.contains("longest duration"), "{reason}");
}

// spec: DUR#text-of-a-duration
#[test]
fn duration_text_runs_from_days_down_and_round_trips() {
    for (value, expected) in [
        ("90m", "1h 30m"),
        ("2w", "14d"),
        ("0s", "0s"),
        ("0d", "0s"),
        ("1500ms", "1s 500ms"),
        ("1.000001s", "1s 1us"),
        ("36h", "1d 12h"),
    ] {
        assert_eq!(text(&format!("{}.to_string()", d(value))), expected, "{value}");
        assert!(
            truth(&format!("std::parse_duration({}.to_string()) == {}", d(value), d(value))),
            "{value} round trips"
        );
    }
    assert_eq!(text(&format!("`${{{}}}`", d("90s"))), "1m 30s");
}

// spec: DUR#reading-a-duration
#[test]
fn whole_getters_throw_on_a_fraction_of_their_unit() {
    assert_eq!(int(&format!("{}.as_millis()", d("1.5s"))), 1500);
    let reason = refusal(&format!("{}.as_seconds()", d("1.5s")));
    assert!(reason.contains("whole number of seconds"), "{reason}");
    let reason = refusal(&format!("{}.as_millis()", d("1500us")));
    assert!(reason.contains("whole number of milliseconds"), "{reason}");
}

// spec: DUR#reading-a-duration
#[test]
fn float_getters_never_throw() {
    assert!(truth(&format!("{}.as_seconds_f() == 1.5", d("1500ms"))));
    assert!(truth(&format!("{}.as_millis_f() == 1.5", d("1500us"))));
}

// spec: DUR#comparing-durations
#[test]
fn durations_compare_by_length() {
    assert!(truth(&format!("{} == {}", d("90m"), d("1h 30m"))));
    assert!(truth(&format!("{} < {}", d("59s"), d("1m"))));
    assert!(truth(&format!("{} >= {}", d("1d"), d("24h"))));
    assert!(truth(&format!("{} != {}", d("1s"), d("1ms"))));
}

// spec: DUR#comparing-durations
#[test]
fn comparing_a_duration_with_another_type_throws() {
    for other in [r#""90s""#, "90", "std::parse_version(\"1.0\")"] {
        let reason = refusal(&format!("{} == {other}", d("90s")));
        assert!(reason.contains("compares only with a Duration"), "{other}: {reason}");
        let reason = refusal(&format!("{other} < {}", d("90s")));
        assert!(reason.contains("cannot compare"), "{other}: {reason}");
    }
}

// spec: DUR#arithmetic-on-durations
#[test]
fn durations_add_subtract_multiply_and_divide() {
    assert_eq!(text(&format!("({} + {}).to_string()", d("1h"), d("30m"))), "1h 30m");
    assert_eq!(text(&format!("({} - {}).to_string()", d("1h"), d("30m"))), "30m");
    assert_eq!(text(&format!("({} * 3).to_string()", d("20m"))), "1h");
    assert_eq!(text(&format!("(3 * {}).to_string()", d("20m"))), "1h");
    assert_eq!(text(&format!("({} / 4).to_string()", d("1h"))), "15m");
    assert_eq!(text(&format!("({} / 3).to_string()", d("1ns"))), "0s");
}

// spec: DUR#arithmetic-on-durations
#[test]
fn arithmetic_throws_rather_than_go_negative_overflow_or_divide_by_zero() {
    for (expression, expected) in [
        (format!("{} - {}", d("1s"), d("2s")), "negative"),
        (format!("{} * -1", d("1s")), "negative"),
        (format!("{} / -1", d("1s")), "negative"),
        (format!("{} / 0", d("1s")), "by zero"),
        (format!("{} * 2", d("100000d")), "longest duration"),
        (format!("{} + {}", d("100000d"), d("100000d")), "longest duration"),
    ] {
        let reason = refusal(&expression);
        assert!(reason.contains(expected), "{expression}: {reason}");
    }
}

// spec: DUR#bounds
#[test]
fn the_parser_takes_bounds_and_states_the_one_a_value_breaks() {
    assert_eq!(
        int(r#"std::parse_duration("90s", #{ min: "1s", max: std::parse_duration("1h"), precision: "s" }).as_seconds()"#),
        90
    );
    for (value, expected) in [
        ("500ms", "at least 1s"),
        ("2h", "at most 1h"),
        ("1500ms", "whole number of seconds"),
    ] {
        let reason = refusal(&format!(
            r#"std::parse_duration({value:?}, #{{ min: "1s", max: "1h", precision: "s" }})"#
        ));
        assert!(reason.contains(expected), "{value}: {reason}");
    }
}

// spec: DUR#bounds
#[test]
fn the_parser_refuses_arguments_no_duration_satisfies() {
    for (arguments, expected) in [
        (r#"#{ min: "2s", max: "1s" }"#, "longer than"),
        (r#"#{ min: "1500ms", max: "1900ms", precision: "s" }"#, "whole number of seconds"),
        (r#"#{ precision: "w" }"#, "designators"),
        (r#"#{ min: 1 }"#, "`arguments.min`"),
        (r#"#{ step: "1s" }"#, "step"),
    ] {
        let reason = refusal(&format!(r#"std::parse_duration("1s", {arguments})"#));
        assert!(reason.contains(expected), "{arguments}: {reason}");
    }
}
