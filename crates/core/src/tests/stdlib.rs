use super::*;

/// Whether `err` reports that the helper `std::nope` does not exist, as
/// opposed to the `std` module itself being absent.
fn is_missing_helper(err: &rhai::EvalAltResult) -> bool {
    matches!(err, rhai::EvalAltResult::ErrorFunctionNotFound(name, _) if name.starts_with("std::nope"))
}

// spec: STD#the-std-module
#[test]
fn std_is_a_module_in_the_static_context() {
    let err = run_test_script_err("std::nope();");
    assert!(is_missing_helper(&err), "got {err:?}");

    let err = run_test_script_err("nope::nope();");
    assert!(
        matches!(*err, rhai::EvalAltResult::ErrorModuleNotFound(..)),
        "a module that is not registered reads differently, got {err:?}",
    );
}

// spec: STD#the-std-module
#[test]
fn std_is_a_module_in_the_action_context() {
    let err = run_action_err(
        r#"app.on_action("call", |rt, p| { std::nope(); });"#,
        "call",
    );
    assert!(err.contains("std::nope"), "got {err}");
    assert!(err.contains("Function not found"), "got {err}");
}

// spec: STD#the-std-module
#[test]
fn std_is_a_module_in_the_validator_context() {
    let (engine, mut scope, _app, _ast) = run_test_script("");
    let _frame = crate::defs::app::ValidatorFrame::enter();
    let err = run_script(&engine, &mut scope, "std::nope();").expect_err("no such helper");
    assert!(is_missing_helper(&err), "got {err:?}");
}

// spec: STD#the-std-module
#[test]
fn a_script_cannot_add_to_std() {
    for source in [
        "fn nope() {} std::nope();",
        "let nope = || 1; std::nope();",
        "let std = #{ nope: || 1 }; std::nope();",
    ] {
        let err = run_test_script_err(source);
        assert!(is_missing_helper(&err), "{source}: got {err:?}");
    }
}

/// Evaluate `source` in the static context and return its value.
fn eval(source: &str) -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
    let (engine, mut scope, _app) = setup(&crate::ScriptLimits::default());
    engine.eval_with_scope(&mut scope, source)
}

/// The reason `std::<helper>` gives for refusing `value`, written as a
/// script literal.
fn refusal(helper: &str, value: &str) -> String {
    match *eval(&format!("std::{helper}({value})")).expect_err(value) {
        rhai::EvalAltResult::ErrorRuntime(ref reason, _) => reason.to_string(),
        other => panic!("{helper}({value}) should raise a script error, got {other:?}"),
    }
}

fn accepts(helper: &str, value: &str) -> String {
    eval(&format!("std::{helper}({value})"))
        .unwrap_or_else(|e| panic!("{helper}({value}) should be accepted, got {e}"))
        .into_string()
        .expect("a string result")
}

// spec: ACN#kind-parsers
#[test]
fn every_kind_has_a_parser_named_for_it() {
    use strum::VariantArray;

    use crate::defs::param_def::ParamKind;

    for &kind in ParamKind::VARIANTS {
        let name = format!("parse_{}", kind.as_str().replace('-', "_"));
        let err = eval(&format!("std::{name}(())")).expect_err("a unit is not a value");
        assert!(
            !matches!(*err, rhai::EvalAltResult::ErrorFunctionNotFound(..)),
            "std::{name} should exist for kind {:?}, got {err:?}",
            kind.as_str(),
        );
    }
    assert_eq!(accepts("parse_weak_password", r#""a""#), "a");
}

// spec: ACN#kind-parsers
// spec: BSL#arguments
#[test]
fn a_parser_refuses_a_value_that_is_not_a_string() {
    assert_eq!(
        refusal("parse_text", "42"),
        "`value` must be a string, got i64"
    );
}

// spec: ACN#kind-parsers
#[test]
fn single_line_kinds_refuse_control_characters_and_separators() {
    for c in [
        '\n', '\r', '\t', '\0', '\u{1b}', '\u{7f}', '\u{85}', '\u{2028}', '\u{2029}',
    ] {
        let value = format!("\"a\\u{:04x}b\"", u32::from(c));
        for helper in [
            "parse_text",
            "parse_email",
            "parse_password",
            "parse_weak_password",
            "parse_random",
        ] {
            let reason = refusal(helper, &value);
            assert!(
                reason.contains(&format!("U+{:04X}", u32::from(c))),
                "{helper} should name U+{:04X}, got {reason}",
                u32::from(c),
            );
        }
    }
}

// spec: ACN#kind-parsers
#[test]
fn text_accepts_every_other_character() {
    assert_eq!(
        accepts("parse_text", r#""Kia ora, Aotearoa ā 🌿""#),
        "Kia ora, Aotearoa ā 🌿"
    );
    assert_eq!(accepts("parse_text", r#""""#), "");
    assert_eq!(accepts("parse_random", r#""0f 1e""#), "0f 1e");
}

// spec: ACN#kind-parsers
#[test]
fn multiline_accepts_any_string() {
    assert_eq!(
        accepts("parse_multiline", r#""a\nb\r\n\tc\u0000""#),
        "a\nb\r\n\tc\0"
    );
}

// spec: ACN#kind-parsers
#[test]
fn email_parses_into_its_local_part_and_domain() {
    let parts = eval(r#"std::parse_email("a@b@example.org")"#)
        .expect("a valid address")
        .try_cast::<rhai::Map>()
        .expect("a map");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts["local"].to_string(), "a");
    assert_eq!(parts["domain"].to_string(), "b@example.org");

    for value in ["@x.org", "a@b", "a@.b", "a@b.", "ab"] {
        assert_eq!(
            refusal("parse_email", &format!("{value:?}")),
            "invalid email address",
            "{value}",
        );
    }
}

// spec: ACN#kind-parsers
#[test]
fn password_refuses_a_weak_value() {
    assert_eq!(
        refusal("parse_password", r#""password""#),
        "password is too weak"
    );
    assert_eq!(
        accepts("parse_password", r#""correct-horse-battery-staple-9X!""#),
        "correct-horse-battery-staple-9X!"
    );
}

// spec: ACN#kind-parsers
#[test]
fn volume_follows_the_name_rules() {
    assert_eq!(accepts("parse_volume", r#""backups""#), "backups");
    for value in ["ab", "-abc", "abc-"] {
        refusal("parse_volume", &format!("{value:?}"));
    }
}

// spec: ACN#kind-parsers
// spec: STD#the-std-module
#[test]
fn parsers_are_callable_in_every_context() {
    let result = run_action(
        r#"app.on_action("call", |rt, p| { std::parse_email("a@example.org"); });"#,
        "call",
    );
    assert!(
        matches!(
            result,
            crate::runtime::barrier::replay::OperationResult::Completed
        ),
        "expected Completed, got {result:?}"
    );
    let (engine, mut scope, _app, _ast) = run_test_script("");
    let _frame = crate::defs::app::ValidatorFrame::enter();
    run_script(&engine, &mut scope, r#"std::parse_text("a");"#).expect("callable in a validator");
}
