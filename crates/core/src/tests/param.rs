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
// l[verify param.on-change.constraints]
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

// l[verify param.value]
#[test]
fn unset_param_value_falls_back_to_default_value() {
    let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
    crate::tests::run_script(
        &engine,
        &mut scope,
        r#"
        let p = app.param("max-connections").default_value("100");
        if p.is_set() { throw "expected is_set() == false when only a default is declared"; }
        if p.value() != "100" { throw `expected default "100", got: ${p.value()}`; }
        "#,
    )
    .expect("script should evaluate without error");

    let def = app.def.load();
    let schema = def.params.get("max-connections").unwrap();
    assert_eq!(schema.default_value.as_deref(), Some("100"));
}

// l[verify param.value]
#[test]
fn stored_value_overrides_default_value() {
    use std::collections::BTreeMap;

    let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
    {
        let mut stored = BTreeMap::new();
        stored.insert("max-connections".to_owned(), "500".to_owned());
        *app.stored.lock() = stored;
    }
    crate::tests::run_script(
        &engine,
        &mut scope,
        r#"
        let p = app.param("max-connections").default_value("100");
        if p.value() != "500" { throw `stored value should win over default, got: ${p.value()}`; }
        "#,
    )
    .expect("script should evaluate without error");
}

// l[verify param.value]
#[test]
fn unset_param_without_default_still_throws() {
    let (engine, mut scope, _app) = crate::setup_language(&crate::ScriptLimits::default());
    let res = crate::tests::run_script(
        &engine,
        &mut scope,
        r#"
        let p = app.param("hostname");
        p.value();
        "#,
    );
    assert!(
        res.is_err(),
        "value() should throw when unset and no default"
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
            container_signaler: None,
        },
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Completed),
        "operation with injected param should complete without error"
    );
}

// l[verify param.on-change]
// l[verify param.on-change.constraints]
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
            container_signaler: None,
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
fn param_schema_kind_multiline() {
    let app = run_test_script_app(r#"app.param("motd").kind("multiline");"#);
    let def = app.def.load();
    let schema = def.params.get("motd").unwrap();
    assert!(matches!(
        schema.kind,
        crate::defs::install::ParamKind::Multiline
    ));
}

// l[verify param.schema.kind]
#[test]
fn param_schema_kind_unknown_throws() {
    let _ = run_test_script_err(r#"app.param("field").kind("banana");"#);
}

// l[verify param.schema.kind]
// l[verify action.params.volume]
#[test]
fn param_schema_kind_volume_rejected_on_static_param() {
    let _ = run_test_script_err(r#"app.param("dst").kind("volume");"#);
}

// l[verify action.params.volume]
#[test]
fn install_param_kind_volume_rejected() {
    let _ = run_test_script_err(
        r#"
        app.on_install(|rt, _param| {}, #{
            params: #{
                target: #{ kind: "volume" }
            }
        });
    "#,
    );
}

// l[verify action.params.volume]
#[test]
fn action_param_kind_volume_accepted() {
    let app = run_test_script_app(
        r#"
        app.on_action("dump", |rt, _param| {}, #{
            params: #{
                output: #{ kind: "volume", description: "Where to write" }
            }
        });
    "#,
    );
    let def = app.def.load();
    let action = def.actions.get("dump").expect("dump action");
    let param = action
        .params
        .get(&seedling_protocol::names::ParamName::new_unchecked(
            "output".to_owned(),
        ))
        .expect("output param");
    assert_eq!(param.kind, crate::defs::install::ParamKind::Volume);
}

// l[verify action.params.volume]
#[test]
fn shell_param_kind_volume_accepted() {
    let app = run_test_script_app(
        r#"
        app.on_shell("inspect", |_rt, _shell, _param| {}, #{
            params: #{
                target: #{ kind: "volume" }
            }
        });
    "#,
    );
    let def = app.def.load();
    let shell = def.shells.get("inspect").expect("inspect shell");
    let param = shell
        .params
        .get(&seedling_protocol::names::ParamName::new_unchecked(
            "target".to_owned(),
        ))
        .expect("target param");
    assert_eq!(param.kind, crate::defs::install::ParamKind::Volume);
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

// l[verify param.schema.secret]
#[test]
fn param_schema_secret_flag_sets_true() {
    let app = run_test_script_app(r#"app.param("token").secret(true);"#);
    let def = app.def.load();
    let schema = def.params.get("token").expect("param declared");
    assert!(schema.secret);
    assert!(schema.is_secret());
}

// l[verify param.schema.secret]
#[test]
fn param_schema_secret_defaults_false_for_plain_text() {
    let app = run_test_script_app(r#"app.param("hostname");"#);
    let def = app.def.load();
    let schema = def.params.get("hostname").expect("param declared");
    assert!(!schema.secret);
    assert!(!schema.is_secret());
}

// l[verify param.schema.secret-from-kind]
#[test]
fn param_schema_password_kind_implicitly_secret() {
    let app = run_test_script_app(r#"app.param("password").kind("password");"#);
    let def = app.def.load();
    let schema = def.params.get("password").expect("param declared");
    // The `secret` flag itself is not flipped by `kind`, but `is_secret()`
    // must return true because the kind implies secrecy.
    assert!(!schema.secret);
    assert!(schema.is_secret());
}

// l[verify param.schema.secret-from-kind]
#[test]
fn param_schema_weak_password_kind_implicitly_secret() {
    let app = run_test_script_app(r#"app.param("apikey").kind("weak-password");"#);
    let def = app.def.load();
    let schema = def.params.get("apikey").expect("param declared");
    assert!(schema.is_secret());
}

// l[verify param.schema.secret]
#[test]
fn param_schema_text_kind_with_secret_true_is_secret() {
    let app = run_test_script_app(r#"app.param("token").kind("text").secret(true);"#);
    let def = app.def.load();
    let schema = def.params.get("token").expect("param declared");
    assert!(schema.secret);
    assert!(schema.is_secret());
}
