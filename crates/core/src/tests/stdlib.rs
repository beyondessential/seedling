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

    use crate::defs::param_def::ParamKindName;

    for &kind in ParamKindName::VARIANTS {
        let name = crate::defs::stdlib::kinds::helper_name(kind);
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
            "parse_version",
            "parse_version_semver",
            "parse_purl",
            "parse_url",
            "parse_url_origin",
            "parse_hostname",
            "parse_ip_address",
            "parse_ip_subnet",
            "parse_ip_range",
            "parse_volume",
            "parse_quantity",
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
        let reason = refusal("parse_volume", &format!("{value:?}"));
        assert!(
            reason.contains("must be 3 to 63 characters"),
            "{value}: {reason}"
        );
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

fn evaluate_with(
    script: &str,
    seed: &crate::runtime::seeds::AppSeed,
) -> (
    crate::defs::app::App,
    Option<crate::runtime::apps::ScriptError>,
) {
    crate::runtime::apps::evaluate(
        &seedling_protocol::names::AppName::new_unchecked("demo"),
        &std::sync::Arc::new(seedling_bundle::Bundle::from_stored_script(script)),
        &std::collections::BTreeMap::new(),
        seed,
        &crate::ScriptLimits::default(),
    )
}

fn description_with(script: &str, seed: &crate::runtime::seeds::AppSeed) -> String {
    let (app, err) = evaluate_with(script, seed);
    assert!(err.is_none(), "script error: {err:?}");
    app.def.load().description.clone().expect("described")
}

fn seed(byte: u8) -> crate::runtime::seeds::AppSeed {
    crate::runtime::seeds::AppSeed::Ready(crate::runtime::seeds::Seed::from_bytes([byte; 32]))
}

// spec: RND#seeds
#[test]
fn the_static_context_derives_from_the_app_seed() {
    let script =
        r#"app.description(std::uuid_v4("id") + " " + std::random_string("pw", 12, "abcdef"));"#;
    assert_eq!(
        description_with(script, &seed(1)),
        description_with(script, &seed(1))
    );
    assert_ne!(
        description_with(script, &seed(1)),
        description_with(script, &seed(2))
    );
}

// spec: RND#seeds
#[test]
fn adding_a_call_leaves_the_others_values_alone() {
    let one = r#"app.description(std::uuid_v4("id"));"#;
    let two = r#"let other = std::uuid_v4("other"); app.description(std::uuid_v4("id"));"#;
    assert_eq!(
        description_with(one, &seed(1)),
        description_with(two, &seed(1))
    );
}

// spec: RND#seeds
// spec: STD#replicability
#[test]
fn a_validator_derives_what_the_top_level_derives() {
    let script = r#"
        let top = std::uuid_v4("id");
        app.param("token").required(false).validate(|value, all| {
            if std::uuid_v4("id") != top { throw "derived a different value"; }
        });
    "#;
    let proposed = std::collections::BTreeMap::from([("token".to_owned(), "x".to_owned())]);
    let (_, outcome) = crate::runtime::apps::evaluate_validated(
        &seedling_protocol::names::AppName::new_unchecked("demo"),
        &std::sync::Arc::new(seedling_bundle::Bundle::from_stored_script(script)),
        &proposed,
        &crate::runtime::apps::supplied_values(&proposed),
        &seed(1),
        &crate::ScriptLimits::default(),
    );
    let rejections = outcome.expect("evaluates");
    assert!(
        rejections.is_empty(),
        "{:?}",
        rejections.iter().map(|r| &r.reason).collect::<Vec<_>>()
    );
}

// spec: SEC#app-seeds
#[test]
fn an_unreadable_seed_fails_an_evaluation_that_derives() {
    let unreadable = crate::runtime::seeds::AppSeed::Unreadable("bad tag".into());
    let (_, err) = evaluate_with(r#"app.description(std::uuid_v4("id"));"#, &unreadable);
    let err = err.expect("fails").to_string();
    assert!(err.contains("seed cannot be read"), "{err}");
    let (_, err) = evaluate_with(r#"app.description("no randomness");"#, &unreadable);
    assert!(err.is_none(), "{err:?}");
}

// spec: SEC#redaction
#[test]
fn an_evaluation_records_what_it_derived() {
    let (app, err) = evaluate_with(
        r#"
            let pw = std::random_string("pw", 16, "abcdefghijklmnop");
            let raw = std::random_bytes("raw", 9);
            app.description(pw + " " + raw.to_string());
        "#,
        &seed(1),
    );
    assert!(err.is_none(), "{err:?}");
    let def = app.def.load();
    let description = def.description.clone().unwrap();
    let (pw, raw) = description.split_once(' ').unwrap();
    assert_eq!(
        def.derived.len(),
        8,
        "the string, and the Blob's bytes, UTF-8 reading, string form, hex, base64, base64url and percent-encoding"
    );
    assert!(def.derived.found_in(&format!("password={pw};")));
    assert!(
        def.derived.found_in(raw.trim_start_matches('[')),
        "the Blob's hex digits are found without the brackets"
    );
    assert!(!def.derived.found_in("password=abcdefghijklmnop;"));
    assert!(
        !def.derived.found_in(&pw[1..]),
        "part of a value is not the value"
    );
}

// spec: RND#helpers
#[test]
fn a_derivation_helper_checks_its_arguments() {
    for (source, expected) in [
        (r#"std::uuid_v4("")"#, "`label` must not be empty"),
        (r#"std::random_bytes("x", 0)"#, "`n` must be at least 1"),
        (
            r#"std::random_string("x", 0, "ab")"#,
            "`length` must be at least 1",
        ),
        (
            r#"std::random_string("x", 4, "a")"#,
            "at least two characters",
        ),
        (r#"std::random_string("x", 4, "aba")"#, "more than once"),
        (r#"std::uuid_v4(3)"#, "`label` must be a string"),
    ] {
        let (_, err) = evaluate_with(&format!("{source};"), &seed(1));
        let err = err.expect(source).to_string();
        assert!(err.contains(expected), "{source}: {err}");
    }
}

// spec: LIM#limits
#[test]
fn a_derived_value_beyond_the_engine_limits_fails_before_allocating() {
    let (_, err) = evaluate_with(r#"std::random_bytes("x", 9223372036854775807);"#, &seed(1));
    let err = err.expect("fails").to_string();
    assert!(err.contains("too large"), "{err}");
    let (_, err) = evaluate_with(
        r#"std::random_string("x", 9223372036854775807, "ab");"#,
        &seed(1),
    );
    assert!(err.expect("fails").to_string().contains("too large"));
}

// spec: LIM#limits
#[test]
fn a_multibyte_string_is_held_to_the_engine_limit_in_bytes() {
    let limits = crate::ScriptLimits {
        max_string_size: 64,
        ..Default::default()
    };
    let evaluate = |length: usize| {
        crate::runtime::apps::evaluate(
            &seedling_protocol::names::AppName::new_unchecked("demo"),
            &std::sync::Arc::new(seedling_bundle::Bundle::from_stored_script(&format!(
                r#"std::random_string("x", {length}, "aé");"#
            ))),
            &std::collections::BTreeMap::new(),
            &seed(1),
            &limits,
        )
        .1
    };
    assert!(
        evaluate(32).is_none(),
        "32 two-byte characters fit in 64 bytes"
    );
    let err = evaluate(33).expect("33 do not").to_string();
    assert!(err.contains("too large"), "{err}");
}

// spec: LIM#limits
#[test]
fn an_evaluation_is_held_to_its_limit_on_derived_values() {
    let evaluate = |max_derived_values: usize, calls: usize| {
        crate::runtime::apps::evaluate(
            &seedling_protocol::names::AppName::new_unchecked("demo"),
            &std::sync::Arc::new(seedling_bundle::Bundle::from_stored_script(&format!(
                r#"for i in 0..{calls} {{ std::uuid_v4(`id-${{i}}`); }}"#
            ))),
            &std::collections::BTreeMap::new(),
            &seed(1),
            &crate::ScriptLimits {
                max_derived_values,
                ..Default::default()
            },
        )
        .1
    };
    assert!(evaluate(3, 3).is_none(), "three calls fit a limit of three");
    let err = evaluate(3, 4).expect("a fourth does not").to_string();
    assert!(err.contains("too large"), "{err}");
    let err =
        run_test_script_err(r#"try { for i in 0..2000 { std::uuid_v4(`id-${i}`); } } catch { }"#);
    assert!(
        matches!(*err, rhai::EvalAltResult::ErrorDataTooLarge(..)),
        "the limit is not caught: {err:?}"
    );
    assert!(evaluate(0, 2000).is_none(), "0 removes the limit");
}

// spec: RND#helpers
#[test]
fn a_multibyte_alphabet_is_read_as_characters() {
    let value = description_with(
        r#"app.description(std::random_string("x", 30, "αβ"));"#,
        &seed(1),
    );
    assert_eq!(value.chars().count(), 30);
    assert!(value.chars().all(|c| c == 'α' || c == 'β'));
}

/// The value an action closure derived, which it throws to report.
fn thrown_by_action(source: &str) -> String {
    run_action_err(source, "call")
}

// spec: RND#seeds
#[test]
fn an_action_closure_derives_from_the_operation_seed() {
    let inside =
        thrown_by_action(r#"app.on_action("call", |rt, p| { throw std::uuid_v4("id"); });"#);
    let top = thrown_by_action(
        r#"let top = std::uuid_v4("id"); app.on_action("call", |rt, p| { throw top; });"#,
    );
    assert_ne!(
        inside, top,
        "the same call derives apart inside and outside a closure"
    );

    let static_value = description_with(
        r#"app.description(std::uuid_v4("id"));"#,
        &crate::runtime::seeds::AppSeed::for_tests(),
    );
    assert!(top.contains(&static_value), "{top} carries {static_value}");
}

// spec: RND#seeds
#[test]
fn a_captured_value_is_the_one_the_app_was_declared_with() {
    let source = r#"
        let pw = std::random_string("pw", 20, "abcdefghijklmnopqrstuvwxyz");
        app.description(pw);
        app.on_action("call", |rt, p| { throw pw; });
    "#;
    let (result, app) = run_action_on(source, &[], "call", None, ActionHooks::default());
    let crate::runtime::barrier::replay::OperationResult::Failed(err) = result else {
        panic!("expected the throw");
    };
    let declared = app.def.load().description.clone().expect("described");
    assert!(
        err.to_string().contains(&declared),
        "{err} carries {declared}"
    );
}

// spec: SEC#operation-seeds
#[test]
fn an_invoked_action_derives_from_its_callers_operation_seed() {
    let direct =
        thrown_by_action(r#"app.on_action("call", |rt, p| { throw std::uuid_v4("id"); });"#);
    let invoked = thrown_by_action(
        r#"
            app.on_action("inner", |rt, p| { throw std::uuid_v4("id"); });
            app.on_action("call", |rt, p| { app.action("inner").invoke(); });
        "#,
    );
    let uuid = |s: &str| {
        s.split_whitespace()
            .find(|w| uuid::Uuid::parse_str(w).is_ok())
            .expect("a thrown UUID")
            .to_owned()
    };
    assert_eq!(uuid(&invoked), uuid(&direct));
}

// spec: ACN#kind-parsers
#[test]
fn boolean_accepts_exactly_true_and_false() {
    assert!(
        eval(r#"std::parse_boolean("true")"#)
            .unwrap()
            .as_bool()
            .unwrap()
    );
    assert!(
        !eval(r#"std::parse_boolean("false")"#)
            .unwrap()
            .as_bool()
            .unwrap()
    );
    for value in ["True", "FALSE", "1", "0", "yes", " true", ""] {
        assert_eq!(
            refusal("parse_boolean", &format!("{value:?}")),
            "must be true or false",
            "{value:?}"
        );
    }
}

// spec: ACN#kind-parsers
#[test]
fn integer_accepts_only_the_decimal_form() {
    for (value, n) in [
        ("0", 0),
        ("42", 42),
        ("-7", -7),
        ("9223372036854775807", i64::MAX),
        ("-9223372036854775808", i64::MIN),
    ] {
        let parsed = eval(&format!("std::parse_integer({value:?})")).unwrap();
        assert_eq!(parsed.as_int().unwrap(), n, "{value}");
    }
    for value in ["+1", "01", "-0", "1.0", "1e3", " 1", "", "-", "0x10"] {
        assert_eq!(
            refusal("parse_integer", &format!("{value:?}")),
            "must be an integer in decimal form",
            "{value:?}"
        );
    }
    assert_eq!(
        refusal("parse_integer", r#""9223372036854775808""#),
        "must be an integer within the signed 64-bit range"
    );
}

// spec: ACN#kind-parsers
// spec: ACN#kind-arguments
#[test]
fn integer_takes_a_range_and_names_it_when_refusing() {
    assert_eq!(
        eval(r#"std::parse_integer("65535", 1..=65535)"#)
            .unwrap()
            .as_int()
            .unwrap(),
        65535
    );
    assert_eq!(
        refusal("parse_integer", r#""65535", 1..65535"#),
        "must be between 1 and 65534"
    );
    assert_eq!(
        refusal("parse_integer", r#""0", 1..=65535"#),
        "must be between 1 and 65535"
    );
    assert_eq!(
        refusal("parse_integer", r#""-1", ..10"#),
        "must be between 0 and 9"
    );
    assert!(refusal("parse_integer", r#""1", 5..5"#).contains("holds no integer"));
    assert!(refusal("parse_integer", r#""1", 5..=4"#).contains("holds no integer"));
    assert!(refusal("parse_integer", r#""1", [1, 2]"#).contains("must be an integer range"));
}

// spec: ACN#quantities
#[test]
fn quantity_gives_the_amount_of_a_quantity() {
    for (value, n) in [
        ("512Mi", 512 << 20),
        ("1.5G", 1_500_000_000),
        ("-2k", -2000),
    ] {
        let parsed = eval(&format!("std::parse_quantity({value:?})")).unwrap();
        assert_eq!(parsed.as_int().unwrap(), n, "{value}");
    }
    assert_eq!(
        refusal("parse_quantity", r#""512m""#),
        "m is milli; did you mean 512Mi?"
    );
    assert_eq!(
        refusal("parse_quantity", r#""1.1Ki""#),
        "must come to a whole number"
    );
}

// spec: ACN#quantities
// spec: ACN#kind-arguments
#[test]
fn quantity_takes_a_range_and_names_it_in_quantities_when_refusing() {
    assert_eq!(
        eval(r#"std::parse_quantity("64Mi", 64 * 1024 * 1024..=AVAILABLE_MEMORY)"#)
            .unwrap()
            .as_int()
            .unwrap(),
        64 << 20
    );
    assert_eq!(
        refusal("parse_quantity", r#""32Gi", 67108864..=16642998272"#),
        "must be between 64Mi and 15.5Gi"
    );
    assert!(refusal("parse_quantity", r#""1", 5..5"#).contains("holds no integer"));
    assert!(refusal("parse_quantity", r#""1", "1..2""#).contains("must be an integer range"));
}

// spec: ACN#kind-parsers
// spec: ACN#kind-arguments
#[test]
fn choice_accepts_exactly_one_of_its_options() {
    assert_eq!(
        accepts("parse_choice", r#""prod", ["dev", "prod"]"#),
        "prod"
    );
    assert_eq!(
        refusal("parse_choice", r#""Prod", ["dev", "prod"]"#),
        r#"must be one of the options "dev", "prod""#
    );
    assert!(refusal("parse_choice", r#""dev""#).contains("options"));
    for (options, reason) in [
        ("[]", "at least one option"),
        (r#"["a", ""]"#, "an empty option"),
        (r#"["a", "a"]"#, "more than once"),
        (r#"["a\nb"]"#, "U+000A"),
        (r#"["a", 1]"#, "array of strings"),
    ] {
        let refused = refusal("parse_choice", &format!(r#""a", {options}"#));
        assert!(refused.contains(reason), "{options}: {refused}");
    }
}

// spec: ACN#kind-parsers
#[test]
fn the_new_kinds_refuse_control_characters() {
    assert!(refusal("parse_boolean", r#""true\n""#).contains("U+000A"));
    assert!(refusal("parse_integer", r#""1\n""#).contains("U+000A"));
}

fn evaluate_bundle(
    bundle: seedling_bundle::Bundle,
    params: &[(&str, &str)],
    limits: &crate::ScriptLimits,
) -> (
    crate::defs::app::App,
    Option<crate::runtime::apps::ScriptError>,
) {
    crate::runtime::apps::evaluate(
        &seedling_protocol::names::AppName::new_unchecked("demo"),
        &std::sync::Arc::new(bundle),
        &params
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        &seed(1),
        limits,
    )
}

/// The description `script` gives, and whether the evaluation withholds it.
fn withheld(script: &str, params: &[(&str, &str)]) -> (String, bool) {
    let (app, err) = evaluate_bundle(
        seedling_bundle::Bundle::from_stored_script(script),
        params,
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none(), "script error: {err:?}");
    let def = app.def.load();
    let description = def.description.clone().expect("described");
    let found = def.derived.found_in(&description);
    (description, found)
}

// spec: SEC#redaction
#[test]
fn a_result_computed_from_a_derived_value_is_withheld() {
    let (header, found) = withheld(
        r#"
            let pw = std::random_string("pw", 12, "abcdef");
            app.description("Basic " + std::encode_base64("admin:" + pw));
        "#,
        &[],
    );
    assert!(found, "{header}");
}

// spec: SEC#redaction
#[test]
fn a_result_computed_from_a_secret_parameter_is_withheld_however_late_it_is_declared_secret() {
    let (_, found) = withheld(
        r#"
            let token = app.param("token");
            let hex = std::encode_hex(token.value());
            token.secret(true);
            app.description(`token=${hex}`);
        "#,
        &[("token", "s3cret")],
    );
    assert!(found);
    let (_, found) = withheld(
        r#"app.description(std::encode_hex(app.param("token").value()));"#,
        &[("token", "s3cret")],
    );
    assert!(!found, "a parameter that is not secret is shown");
}

// spec: SEC#redaction
#[test]
fn a_result_computed_from_a_secret_parameters_parsed_value_is_withheld() {
    let (_, found) = withheld(
        r#"
            let key = app.param("key").kind("weak-password");
            app.description(std::encode_hex(key.parsed()));
        "#,
        &[("key", "Moa-Kettle")],
    );
    assert!(found);
}

// spec: SEC#redaction
#[test]
fn a_chain_of_helpers_stays_withheld() {
    let (_, found) = withheld(
        r#"
            let token = app.param("token").secret(true);
            let digest = std::digest_sha256(std::encode_hex(token.value()));
            app.description(std::encode_base64url(std::hmac_sha256("k", digest)));
        "#,
        &[("token", "s3cret")],
    );
    assert!(found);
}

// spec: SEC#redaction
#[test]
fn an_empty_secret_value_withholds_nothing() {
    let (_, found) = withheld(
        r#"
            let token = app.param("token").secret(true);
            app.description(std::encode_hex("abc" + token.value()));
        "#,
        &[("token", "")],
    );
    assert!(!found);
}

// spec: SEC#redaction
#[test]
fn a_bundle_file_holds_no_withheld_value() {
    let bundle = seedling_bundle::Bundle::from_files([
        (
            "app.seed.rhai".to_owned(),
            br#"
                let token = app.param("token").secret(true);
                token.value();
                app.description(std::encode_hex(app.file("conf.txt")));
            "#
            .to_vec(),
        ),
        ("conf.txt".to_owned(), b"s3cret".to_vec()),
    ])
    .unwrap();
    let (app, err) = evaluate_bundle(bundle, &[("token", "s3cret")], &Default::default());
    assert!(err.is_none(), "{err:?}");
    let def = app.def.load();
    assert!(!def.derived.found_in(def.description.as_deref().unwrap()));
}

// spec: SEC#redaction
#[test]
fn a_derived_blob_is_withheld_in_every_encoding_bsl_offers() {
    let (app, err) = evaluate_bundle(
        seedling_bundle::Bundle::from_stored_script(
            r#"app.description(std::random_bytes("raw", 9).to_string());"#,
        ),
        &[],
        &Default::default(),
    );
    assert!(err.is_none(), "{err:?}");
    let def = app.def.load();
    let rendered = def.description.clone().unwrap();
    let bytes: Vec<u8> = rendered
        .trim_matches(|c| c == '[' || c == ']')
        .split(' ')
        .flat_map(|group| hex::decode(group).unwrap())
        .collect();
    use base64::Engine as _;
    for form in [
        hex::encode(&bytes),
        base64::engine::general_purpose::STANDARD.encode(&bytes),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes),
    ] {
        assert!(def.derived.found_in(&format!("x{form}x")), "{form}");
    }
}

// spec: LIM#limits
// spec: SEC#redaction
#[test]
fn withheld_results_count_toward_the_derived_values_limit() {
    let evaluate = |max_derived_values: usize, script: &str| {
        evaluate_bundle(
            seedling_bundle::Bundle::from_stored_script(script),
            &[("token", "s3cret")],
            &crate::ScriptLimits {
                max_derived_values,
                ..Default::default()
            },
        )
        .1
    };
    let derived = r#"let pw = std::uuid_v4("pw"); std::encode_hex(pw); std::encode_hex(pw);"#;
    assert!(evaluate(3, derived).is_none());
    let err = evaluate(2, derived)
        .expect("a third does not fit")
        .to_string();
    assert!(err.contains("too large"), "{err}");

    let from_param = r#"
        let token = app.param("token");
        std::encode_hex(token.value());
        std::encode_base64(token.value());
        token.secret(true);
    "#;
    assert!(evaluate(2, from_param).is_none());
    let err = evaluate(1, from_param)
        .expect("counted once the flag is settled")
        .to_string();
    assert!(err.contains("too large"), "{err}");
    let not_secret = from_param.replace("token.secret(true);", "");
    assert!(evaluate(1, &not_secret).is_none(), "nothing is withheld");
}

// spec: SEC#redaction
// spec: FMT#withheld-arguments
#[test]
fn an_encoding_of_a_value_holding_a_secret_is_withheld() {
    let script = r#"
        let token = app.param("token").secret(true);
        let config = #{ db: #{} };
        config.db[token.value()] = 1;
        app.description(std::encode_yaml(config));
    "#;
    let (yaml, found) = withheld(script, &[("token", "s3cret")]);
    assert!(found, "{yaml}");
    let (_, found) = withheld(
        &script.replace(".secret(true)", ""),
        &[("token", "s3cret")],
    );
    assert!(!found, "a parameter that is not secret is shown");
}

// spec: SEC#redaction
// spec: FMT#withheld-arguments
#[test]
fn each_string_decoded_from_a_secret_is_withheld() {
    let params = [("token", r#"{"user": "alice", "pin": 1234}"#)];
    let (user, found) = withheld(
        r#"
            let doc = std::decode_json(app.param("token").secret(true).value());
            app.description(doc.user);
        "#,
        &params,
    );
    assert!(found, "{user}");
    let (pin, found) = withheld(
        r#"
            let doc = std::decode_json(app.param("token").secret(true).value());
            app.description(doc.pin.to_string());
        "#,
        &params,
    );
    assert!(!found, "only strings are withheld, not {pin}");
}

// spec: LIM#limits
// spec: SEC#redaction
#[test]
fn each_decoded_string_counts_toward_the_derived_values_limit() {
    let evaluate = |max_derived_values: usize| {
        evaluate_bundle(
            crate::runtime::definition::Bundle::from_stored_script(
                r#"std::decode_json(app.param("token").secret(true).value());"#,
            ),
            &[("token", r#"{"a": "x", "b": ["x", ""]}"#)],
            &crate::ScriptLimits {
                max_derived_values,
                ..Default::default()
            },
        )
        .1
    };
    assert!(evaluate(3).is_none(), "a, b and x, the empty string withholding nothing");
    let err = evaluate(2).expect("a third does not fit").to_string();
    assert!(err.contains("too large"), "{err}");
}

// spec: RND#password-hashes
#[test]
fn a_password_hash_checks_its_password() {
    for (source, expected) in [
        (
            r#"std::password_argon2id("x", "")"#,
            "`password` must not be empty",
        ),
        (
            r#"std::password_bcrypt("x", "")"#,
            "`password` must not be empty",
        ),
        (
            r#"std::password_argon2id("", "pw")"#,
            "`label` must not be empty",
        ),
        (
            r#"std::password_bcrypt("x", std::random_string("pw", 73, "ab"))"#,
            "at most 72 bytes",
        ),
        (
            r#"std::password_bcrypt("x", std::decode_hex("610062"))"#,
            "zero byte",
        ),
        (
            r#"std::password_argon2id("x", 1)"#,
            "`password` must be a string, Blob",
        ),
    ] {
        let (_, err) = evaluate_with(&format!("{source};"), &seed(1));
        let err = err.expect(source).to_string();
        assert!(err.contains(expected), "{source}: {err}");
    }
    let (_, err) = evaluate_with(
        r#"std::password_bcrypt("x", std::random_string("pw", 72, "ab"));"#,
        &seed(1),
    );
    assert!(err.is_none(), "72 bytes fit: {err:?}");
}

// spec: RND#password-hashes
// spec: SEC#redaction
#[test]
fn a_password_hash_is_derived_and_withheld() {
    let script = r#"app.description(std::password_argon2id("admin", "hunter2"));"#;
    assert_eq!(
        description_with(script, &seed(1)),
        description_with(script, &seed(1))
    );
    assert_ne!(
        description_with(script, &seed(1)),
        description_with(script, &seed(2))
    );
    let (hash, found) = withheld(script, &[]);
    assert!(
        hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
        "{hash}"
    );
    assert!(found);
}

// spec: LIM#limits
#[test]
fn an_evaluation_is_held_to_its_limit_on_password_hashes() {
    let evaluate = |limits: crate::ScriptLimits, script: &str| {
        evaluate_bundle(
            seedling_bundle::Bundle::from_stored_script(script),
            &[],
            &limits,
        )
        .1
    };
    let two = r#"std::password_argon2id("a", "pw"); std::password_argon2id("b", "pw");"#;
    let hashes = |max_password_hashes| crate::ScriptLimits {
        max_password_hashes,
        ..Default::default()
    };
    assert!(evaluate(hashes(2), two).is_none());
    let err = evaluate(hashes(1), two)
        .expect("a second does not fit")
        .to_string();
    assert!(err.contains("too large"), "{err}");
    assert!(evaluate(hashes(0), two).is_none(), "0 removes the limit");
    let err = evaluate(
        crate::ScriptLimits {
            max_derived_values: 1,
            ..Default::default()
        },
        two,
    )
    .expect("a password hash is a derived value too")
    .to_string();
    assert!(err.contains("too large"), "{err}");
}

// spec: ENC#byte-arguments
#[test]
fn a_bytes_argument_takes_a_bundle_file_and_directory() {
    let bundle = seedling_bundle::Bundle::from_files([
        (
            "app.seed.rhai".to_owned(),
            br#"app.description(
                std::encode_hex(std::digest_sha256(app.file("conf/a.txt")))
                + " " + std::encode_hex(std::digest_sha256(app.dir("conf")))
            );"#
            .to_vec(),
        ),
        ("conf/a.txt".to_owned(), b"abc".to_vec()),
    ])
    .unwrap();
    let (app, err) = evaluate_bundle(bundle, &[], &Default::default());
    assert!(err.is_none(), "{err:?}");
    let description = app.def.load().description.clone().unwrap();
    let (file, dir) = description.split_once(' ').unwrap();
    assert_eq!(
        file,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    use sha2::Digest as _;
    let encoding = [
        &[0, 0, 0, 5][..],
        b"a.txt",
        &[0, 0, 0, 0, 0, 0, 0, 3],
        b"abc",
    ]
    .concat();
    assert_eq!(dir, hex::encode(sha2::Sha256::digest(&encoding)));
}
