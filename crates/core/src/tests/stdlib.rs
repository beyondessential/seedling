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
