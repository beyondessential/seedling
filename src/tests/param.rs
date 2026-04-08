use std::sync::Arc;

use crate::{
    runtime::{
        LifecycleState, TestWorldOracle,
        barrier::{
            OperationId,
            replay::{InMemoryActionLog, OperationContext, OperationResult, run_operation},
        },
    },
    tests::{run_test_script, run_test_script_app, run_test_script_err},
};

// -----------------------------------------------------------------------
// param.value — string coercions
// -----------------------------------------------------------------------

// l[verify param.value]
#[test]
fn param_value_used_in_string_interpolation() {
    let app = run_test_script_app(
        r#"
        let v = app.param("version");
        let tag = `ghcr.io/example/app:${v}`;
        app.deployment("web").image(tag);
        "#,
    );
    let def = app.def.lock();
    let dep = def.resources.values().next().expect("one resource");
    // The image is constructed during script eval; we just verify no panic.
    let _ = dep;
}

// l[verify param.value]
#[test]
fn param_to_string_returns_value() {
    let app = run_test_script_app(r#"let _host = app.param("host");"#);
    let def = app.def.lock();
    assert_eq!(
        def.params.get("host").map(String::as_str),
        Some("<placeholder>")
    );
}

// -----------------------------------------------------------------------
// param.on-change — registration
// -----------------------------------------------------------------------

// l[verify param.on-change]
#[test]
fn on_change_registers_handler_in_app_def() {
    let app = run_test_script_app(
        r#"
        let p = app.param("version");
        p.on_change(|rt| {});
        "#,
    );
    let def = app.def.lock();
    assert!(
        def.param_changes.contains("version"),
        "on_change should register handler in AppDef.param_changes",
    );
}

// l[verify param.on-change]
#[test]
fn on_change_two_arg_closure_registers() {
    let app = run_test_script_app(
        r#"
        let p = app.param("domain");
        p.on_change(|rt, old| {});
        "#,
    );
    let def = app.def.lock();
    assert!(def.param_changes.contains("domain"));
}

// l[verify param.on-change]
#[test]
fn on_change_different_params_each_register() {
    let app = run_test_script_app(
        r#"
        app.param("version").on_change(|rt| {});
        app.param("domain").on_change(|rt| {});
        "#,
    );
    let def = app.def.lock();
    assert!(def.param_changes.contains("version"));
    assert!(def.param_changes.contains("domain"));
}

// -----------------------------------------------------------------------
// param.on-change — error cases
// -----------------------------------------------------------------------

// l[verify param.on-change]
#[test]
fn on_change_twice_on_same_param_throws() {
    let err = run_test_script_err(
        r#"
        let p = app.param("version");
        p.on_change(|rt| {});
        p.on_change(|rt| {});
        "#,
    );
    let msg = err.to_string();
    assert!(
        msg.contains("on_change") && msg.contains("version"),
        "error should mention on_change and the param name, got: {msg}",
    );
}

// -----------------------------------------------------------------------
// param.store — pre-injected value overrides placeholder
// -----------------------------------------------------------------------

// i[verify param.store]
#[test]
fn pre_injected_param_value_overrides_placeholder() {
    use std::collections::BTreeMap;

    // Simulate what AppRegistry does: pre-populate app.def.params before
    // running the script so stored values win over the placeholder.
    let (engine, mut scope, app) = crate::setup_language();
    {
        let mut params = BTreeMap::new();
        params.insert("hostname".to_owned(), "injected.example.com".to_owned());
        app.def.lock().params = params;
    }
    crate::tests::run_script(&engine, &mut scope, r#"let h = app.param("hostname");"#)
        .expect("script should evaluate");

    let def = app.def.lock();
    assert_eq!(
        def.params.get("hostname").map(String::as_str),
        Some("injected.example.com"),
        "pre-injected param value should replace the <placeholder>"
    );
}

// i[verify param.store]
#[test]
fn param_used_in_closure_captures_injected_value() {
    use std::collections::BTreeMap;

    let mut params = BTreeMap::new();
    params.insert("version".to_owned(), "2.0".to_owned());

    let (engine, mut scope, app) = crate::setup_language();
    {
        app.def.lock().params = params;
    }
    let ast = crate::tests::run_script(
        &engine,
        &mut scope,
        r#"
        let ver = app.param("version");
        app.on_start(|rt| {
            rt.start(app.deployment("web").image(`myapp:${ver}`));
        });
        "#,
    )
    .expect("script should evaluate");

    let def = app.def.lock();
    assert_eq!(
        def.params.get("version").map(String::as_str),
        Some("2.0"),
        "injected version should be captured, not placeholder"
    );
    drop(def);

    // Verify the action can be invoked with the injected value in scope.
    let oracle = Arc::new(crate::runtime::TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let result = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: OperationId::new(),
            app: &app,
            action_name: "start",
            log: &log,
            world: oracle,
            registry: std::sync::Arc::new(crate::runtime::EphemeralInstanceRegistry::new()),
            active_progress: None,
            tick_notify: None,
        },
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Completed),
        "operation with injected param should complete without error"
    );
}

// l[verify param.on-change]
#[test]
fn on_change_inside_action_closure_throws() {
    let (engine, mut scope, app, ast) = run_test_script(
        r#"
        let p = app.param("version");
        app.on_start(|rt| {
            p.on_change(|rt| {});
        });
        "#,
    );
    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(
        crate::runtime::ResourceInstance::new_singleton(
            "test",
            crate::defs::resource::ResourceKind::Deployment,
            "x",
        ),
        LifecycleState::Ready,
    );
    let log = InMemoryActionLog::new();
    let result = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: OperationId::new(),
            app: &app,
            action_name: "start",
            log: &log,
            world: oracle,
            registry: std::sync::Arc::new(crate::runtime::EphemeralInstanceRegistry::new()),
            active_progress: None,
            tick_notify: None,
        },
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Failed(_)),
        "on_change inside action closure should cause the operation to fail",
    );
}
