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
    let def = app.def.load();
    let dep = def.resources.values().next().expect("one resource");
    // The image is constructed during script eval; we just verify no panic.
    let _ = dep;
}

// l[verify param.is-set]
#[test]
fn unset_param_is_not_set() {
    let app = run_test_script_app(r#"let _host = app.param("host");"#);
    let def = app.def.load();
    assert!(
        def.params.contains_key("host"),
        "host should be in declared params"
    );
    assert!(
        app.stored.lock().get("host").is_none(),
        "host should have no stored value"
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
    let def = app.def.load();
    assert!(
        def.param_changes.contains("version"),
        "on_change should register handler in AppDef.param_changes",
    );
}

// l[verify param.on-change]
// l[verify param.on-change.old]
#[test]
fn on_change_two_arg_closure_registers() {
    let app = run_test_script_app(
        r#"
        let p = app.param("domain");
        p.on_change(|rt, old| {});
        "#,
    );
    let def = app.def.load();
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
    let def = app.def.load();
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
// param.store — stored value accessible via is_set() / value()
// -----------------------------------------------------------------------

// i[verify param.store]
#[test]
fn stored_param_is_set_returns_true() {
    use std::collections::BTreeMap;

    let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
    {
        let mut stored = BTreeMap::new();
        stored.insert("hostname".to_owned(), "injected.example.com".to_owned());
        *app.stored.lock() = stored;
    }
    crate::tests::run_script(
        &engine,
        &mut scope,
        r#"
        let h = app.param("hostname");
        if !h.is_set() { throw "expected is_set() == true"; }
        if h.value() != "injected.example.com" { throw `wrong value: ${h.value()}`; }
        "#,
    )
    .expect("script should evaluate without error");

    let def = app.def.load();
    assert!(
        def.params.contains_key("hostname"),
        "hostname should be recorded as declared"
    );
}

// i[verify param.store]
#[test]
fn unset_param_is_set_returns_false() {
    let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
    // No stored values pre-populated.
    crate::tests::run_script(
        &engine,
        &mut scope,
        r#"
        let h = app.param("hostname");
        if h.is_set() { throw "expected is_set() == false"; }
        "#,
    )
    .expect("script should evaluate without error");

    let def = app.def.load();
    assert!(
        def.params.contains_key("hostname"),
        "hostname should be recorded as declared even when unset"
    );
}

// i[verify param.store]
#[test]
fn value_throws_when_param_not_set() {
    let err = crate::tests::run_test_script_err(
        r#"
        let h = app.param("hostname");
        h.value()
        "#,
    );
    let msg = err.to_string();
    assert!(
        msg.contains("hostname") && msg.contains("not set"),
        "error should mention the param name and 'not set', got: {msg}"
    );
}

// i[verify param.store]
#[test]
fn param_used_in_closure_captures_injected_value() {
    use std::collections::BTreeMap;

    let mut stored = BTreeMap::new();
    stored.insert("version".to_owned(), "2.0".to_owned());

    let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
    *app.stored.lock() = stored;

    let ast = crate::tests::run_script(
        &engine,
        &mut scope,
        r#"
        let ver = app.param("version");
        let dep = app.deployment("web").image("docker.io/library/placeholder:latest");
        app.on_start(|rt, _param| {
            rt.start(app.deployment().image(`docker.io/library/myapp:${ver.value()}`));
        });
        "#,
    )
    .expect("script should evaluate");

    assert!(
        app.def.load().params.contains_key("version"),
        "version should be recorded as declared"
    );
    assert_eq!(
        app.stored.lock().get("version").map(String::as_str),
        Some("2.0"),
        "injected version should be in App.stored"
    );

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
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: std::sync::Arc::new(crate::runtime::barrier::CancelToken::new()),
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
        app.on_start(|rt, _param| {
            p.on_change(|rt| {});
        });
        "#,
    );
    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(
        crate::runtime::ResourceInstance::new_singleton(
            seedling_protocol::names::AppName::new("test").unwrap(),
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
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: std::sync::Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Failed(_)),
        "on_change inside action closure should cause the operation to fail",
    );
}

// -----------------------------------------------------------------------
// param.schema — builder methods
// -----------------------------------------------------------------------

// l[verify param.schema]
// l[verify param.schema.kind]
// l[verify param.schema.required]
// l[verify param.schema.default-value]
// l[verify param.schema.description]
#[test]
fn param_schema_builder_methods_set_fields() {
    let app = run_test_script_app(
        r#"
        app.param("admin-email")
            .kind("email")
            .required(true)
            .default_value("admin@example.com")
            .description("Admin email address");
        "#,
    );
    let def = app.def.load();
    let schema = def
        .params
        .get("admin-email")
        .expect("param should be declared");
    assert!(matches!(
        schema.kind,
        crate::defs::install::ParamKind::Email
    ));
    assert!(schema.required);
    assert_eq!(schema.default_value.as_deref(), Some("admin@example.com"));
    assert_eq!(schema.description.as_deref(), Some("Admin email address"));
}

// l[verify param.schema]
#[test]
fn param_schema_defaults_when_no_builder_methods() {
    let app = run_test_script_app(r#"app.param("host");"#);
    let def = app.def.load();
    let schema = def.params.get("host").expect("param should be declared");
    assert!(matches!(schema.kind, crate::defs::install::ParamKind::Text));
    assert!(!schema.required);
    assert!(schema.default_value.is_none());
    assert!(schema.description.is_none());
}

// l[verify param.schema.kind]
#[test]
fn param_schema_kind_password() {
    let app = run_test_script_app(r#"app.param("secret").kind("password");"#);
    let def = app.def.load();
    let schema = def.params.get("secret").unwrap();
    assert!(matches!(
        schema.kind,
        crate::defs::install::ParamKind::Password
    ));
}

// l[verify param.schema.kind]
#[test]
fn param_schema_kind_unknown_throws() {
    let _ = run_test_script_err(r#"app.param("field").kind("banana");"#);
}

// l[verify param.schema]
#[test]
fn param_schema_builder_methods_return_same_param_for_chaining() {
    let app = run_test_script_app(
        r#"
        let p = app.param("site-name").description("Site name").required(true);
        "#,
    );
    let def = app.def.load();
    let schema = def
        .params
        .get("site-name")
        .expect("param should be declared");
    assert_eq!(schema.description.as_deref(), Some("Site name"));
    assert!(schema.required);
}
