use std::sync::Arc;

use crate::{
    runtime::barrier::replay::{InMemoryActionLog, OperationResult, run_operation},
    tests::{ActionHooks, run_test_script, run_test_script_app, run_test_script_err},
};

// -----------------------------------------------------------------------
// param.value — string coercions
// -----------------------------------------------------------------------

// spec: PARAM#values
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

// spec: PARAM#values
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

// spec: PARAM#validators
#[test]
fn validate_with_a_named_function_throws() {
    crate::tests::assert_named_handler_rejected(
        "validate",
        r#"app.param("domain").validate(HANDLER);"#,
    );
}

// -----------------------------------------------------------------------
// param.store — stored value accessible via is_set() / value()
// -----------------------------------------------------------------------

// spec: PRM#parameter-storage
#[test]
fn stored_param_is_set_returns_true() {
    use std::collections::BTreeMap;

    let (engine, mut scope, app) =
        crate::setup_language(&crate::ScriptLimits::default(), Default::default())
            .expect("the scope builds");
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

// spec: PARAM#values
#[test]
fn unset_param_value_falls_back_to_default_value() {
    let (engine, mut scope, app) =
        crate::setup_language(&crate::ScriptLimits::default(), Default::default())
            .expect("the scope builds");
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
    assert_eq!(schema.default_text().as_deref(), Some("100"));
}

// spec: PARAM#values
#[test]
fn stored_value_overrides_default_value() {
    use std::collections::BTreeMap;

    let (engine, mut scope, app) =
        crate::setup_language(&crate::ScriptLimits::default(), Default::default())
            .expect("the scope builds");
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

// spec: PARAM#values
#[test]
fn unset_param_without_default_still_throws() {
    let (engine, mut scope, _app) =
        crate::setup_language(&crate::ScriptLimits::default(), Default::default())
            .expect("the scope builds");
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

// spec: PRM#parameter-storage
#[test]
fn unset_param_is_set_returns_false() {
    let (engine, mut scope, app) =
        crate::setup_language(&crate::ScriptLimits::default(), Default::default())
            .expect("the scope builds");
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

// spec: PRM#parameter-storage
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

// spec: PRM#parameter-storage
#[test]
fn param_used_in_closure_captures_injected_value() {
    use std::collections::BTreeMap;

    let mut stored = BTreeMap::new();
    stored.insert("version".to_owned(), "2.0".to_owned());

    let (engine, mut scope, app) =
        crate::setup_language(&crate::ScriptLimits::default(), Default::default())
            .expect("the scope builds");
    *app.stored.lock() = stored;

    let ast = crate::tests::run_script(
        &engine,
        &mut scope,
        r#"
        let ver = app.param("version");
        let dep = app.deployment("web").image("docker.io/library/placeholder:latest");
        app.on_action("start", |rt, _param| {
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
        super::action_context(
            &engine,
            &ast,
            &app,
            "start",
            &log,
            oracle,
            ActionHooks::default(),
        ),
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Completed),
        "operation with injected param should complete without error"
    );
}

// -----------------------------------------------------------------------
// param.schema — builder methods
// -----------------------------------------------------------------------

// spec: PARAM#schema
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
        crate::defs::param_def::ParamKind::Email
    ));
    assert!(schema.required);
    assert_eq!(schema.default_text().as_deref(), Some("admin@example.com"));
    assert_eq!(schema.description.as_deref(), Some("Admin email address"));
}

// spec: PARAM#schema
#[test]
fn param_schema_defaults_when_no_builder_methods() {
    let app = run_test_script_app(r#"app.param("host");"#);
    let def = app.def.load();
    let schema = def.params.get("host").expect("param should be declared");
    assert!(matches!(
        schema.kind,
        crate::defs::param_def::ParamKind::Text
    ));
    assert!(!schema.required);
    assert!(schema.default_value.is_none());
    assert!(schema.description.is_none());
}

// spec: PARAM#schema
#[test]
fn param_schema_kind_password() {
    let app = run_test_script_app(r#"app.param("secret").kind("password");"#);
    let def = app.def.load();
    let schema = def.params.get("secret").unwrap();
    assert!(matches!(
        schema.kind,
        crate::defs::param_def::ParamKind::Password
    ));
}

// spec: PARAM#schema
#[test]
fn param_schema_kind_multiline() {
    let app = run_test_script_app(r#"app.param("motd").kind("multiline");"#);
    let def = app.def.load();
    let schema = def.params.get("motd").unwrap();
    assert!(matches!(
        schema.kind,
        crate::defs::param_def::ParamKind::Multiline
    ));
}

// spec: PARAM#schema
#[test]
fn param_schema_kind_unknown_throws() {
    let _ = run_test_script_err(r#"app.param("field").kind("banana");"#);
}

// spec: PARAM#schema
// spec: ACN#volume-params
#[test]
fn param_schema_kind_volume_rejected_on_static_param() {
    let _ = run_test_script_err(r#"app.param("dst").kind("volume");"#);
}

// spec: PARAM#schema
// spec: ACN#param-kinds
#[test]
fn param_schema_kind_version() {
    use crate::defs::param_def::ParamKind;

    for (name, kind) in [
        ("version", ParamKind::Version),
        ("version-semver", ParamKind::VersionSemver),
    ] {
        let app = run_test_script_app(&format!(r#"app.param("tag").kind("{name}");"#));
        let def = app.def.load();
        let schema = def.params.get("tag").expect("param declared");
        assert_eq!(schema.kind, kind, "{name}");
        assert!(!schema.is_secret(), "{name}");
    }
}

// spec: PARAM#schema
// spec: ACN#param-kinds
#[test]
fn param_schema_kind_purl() {
    let app = run_test_script_app(r#"app.param("package").kind("purl");"#);
    let def = app.def.load();
    let schema = def.params.get("package").expect("param declared");
    assert_eq!(schema.kind, crate::defs::param_def::ParamKind::Purl);
    assert!(!schema.is_secret());
}

// spec: ACN#param-kinds
#[test]
fn action_param_kind_purl_accepted() {
    let app = run_test_script_app(
        r#"
        app.on_action("fetch", |rt, _param| {}, #{
            params: #{ dependency: #{ kind: "purl" } }
        });
    "#,
    );
    let def = app.def.load();
    let action = def.actions.get("fetch").expect("fetch action");
    let param = action
        .params
        .get(&seedling_protocol::names::ParamName::new_unchecked(
            "dependency".to_owned(),
        ))
        .expect("param declared");
    assert_eq!(param.kind, crate::defs::param_def::ParamKind::Purl);
}

// spec: PARAM#schema
// spec: ACN#param-kinds
#[test]
fn param_schema_kind_url() {
    use crate::defs::param_def::ParamKind;

    for (name, kind) in [
        ("url", ParamKind::Url),
        ("url-origin", ParamKind::UrlOrigin),
        ("hostname", ParamKind::Hostname),
    ] {
        let app = run_test_script_app(&format!(
            r#"
            app.param("endpoint").kind("{name}");
            app.on_action("point", |rt, _param| {{}}, #{{
                params: #{{ endpoint: #{{ kind: "{name}" }} }}
            }});
            "#
        ));
        let def = app.def.load();
        let schema = def.params.get("endpoint").expect("param declared");
        assert_eq!(schema.kind, kind, "{name}");
        assert!(!schema.is_secret(), "{name}");
        let action = def.actions.get("point").expect("point action");
        let param = action
            .params
            .get(&seedling_protocol::names::ParamName::new_unchecked(
                "endpoint".to_owned(),
            ))
            .expect("action param declared");
        assert_eq!(param.kind, kind, "{name}");
    }
}

// spec: ACN#param-kinds
#[test]
fn action_param_kind_version_accepted() {
    let app = run_test_script_app(
        r#"
        app.on_action("warm", |rt, _param| {}, #{
            params: #{
                generic: #{ kind: "version" },
                strict: #{ kind: "version-semver" },
            }
        });
    "#,
    );
    let def = app.def.load();
    let action = def.actions.get("warm").expect("warm action");
    let kind_of = |name: &str| {
        action
            .params
            .get(&seedling_protocol::names::ParamName::new_unchecked(
                name.to_owned(),
            ))
            .expect("param declared")
            .kind
            .clone()
    };
    assert_eq!(
        kind_of("generic"),
        crate::defs::param_def::ParamKind::Version
    );
    assert_eq!(
        kind_of("strict"),
        crate::defs::param_def::ParamKind::VersionSemver
    );
}

// spec: ACN#volume-params
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
    assert_eq!(param.kind, crate::defs::param_def::ParamKind::Volume);
}

// spec: ACN#volume-params
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
    assert_eq!(param.kind, crate::defs::param_def::ParamKind::Volume);
}

// spec: PARAM#schema
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

// spec: PARAM#secret-parameters
#[test]
fn param_schema_secret_flag_sets_true() {
    let app = run_test_script_app(r#"app.param("token").secret(true);"#);
    let def = app.def.load();
    let schema = def.params.get("token").expect("param declared");
    assert_eq!(schema.secret, Some(true));
    assert!(schema.is_secret());
}

// spec: PARAM#secret-parameters
#[test]
fn param_schema_secret_defaults_false_for_plain_text() {
    let app = run_test_script_app(r#"app.param("hostname");"#);
    let def = app.def.load();
    let schema = def.params.get("hostname").expect("param declared");
    assert_eq!(schema.secret, None);
    assert!(!schema.is_secret());
}

// spec: PARAM#secret-parameters
#[test]
fn param_schema_password_kind_implicitly_secret() {
    let app = run_test_script_app(r#"app.param("password").kind("password");"#);
    let def = app.def.load();
    let schema = def.params.get("password").expect("param declared");
    // `kind` does not declare `secret`, but `is_secret()` must return true
    // because the kind implies secrecy.
    assert_eq!(schema.secret, None);
    assert!(schema.is_secret());
}

// spec: PARAM#secret-parameters
#[test]
fn param_schema_weak_password_kind_implicitly_secret() {
    let app = run_test_script_app(r#"app.param("apikey").kind("weak-password");"#);
    let def = app.def.load();
    let schema = def.params.get("apikey").expect("param declared");
    assert!(schema.is_secret());
}

// spec: PARAM#secret-parameters
#[test]
fn param_schema_text_kind_with_secret_true_is_secret() {
    let app = run_test_script_app(r#"app.param("token").kind("text").secret(true);"#);
    let def = app.def.load();
    let schema = def.params.get("token").expect("param declared");
    assert_eq!(schema.secret, Some(true));
    assert!(schema.is_secret());
}

// spec: PARAM#secret-parameters
// An explicit `false` used to be indistinguishable from never having been set,
// so a password kind stayed secret however the script opted out.
#[test]
fn param_schema_explicit_secret_false_overrides_password_kind() {
    for kind in ["password", "weak-password"] {
        for source in [
            format!(r#"app.param("pass").kind("{kind}").secret(false);"#),
            format!(r#"app.param("pass").secret(false).kind("{kind}");"#),
        ] {
            let app = run_test_script_app(&source);
            let def = app.def.load();
            let schema = def.params.get("pass").expect("param declared");
            assert_eq!(schema.secret, Some(false), "{source}");
            assert!(!schema.is_secret(), "{source}");
        }
    }
}

// spec: PARAM#secret-parameters
#[test]
fn param_schema_explicit_secret_true_still_secret_on_password_kind() {
    let app = run_test_script_app(r#"app.param("pass").kind("password").secret(true);"#);
    let def = app.def.load();
    assert!(def.params.get("pass").expect("param declared").is_secret());
}

// spec: ACN#param-kinds
#[test]
fn param_schema_kind_random() {
    let app = run_test_script_app(r#"app.param("nonce").kind("random");"#);
    let def = app.def.load();
    let schema = def.params.get("nonce").expect("param declared");
    assert!(matches!(
        schema.kind,
        crate::defs::param_def::ParamKind::Random
    ));
}

// spec: ACN#param-kinds
// spec: PARAM#secret-parameters
#[test]
fn param_schema_random_kind_not_implicitly_secret() {
    let app = run_test_script_app(r#"app.param("nonce").kind("random");"#);
    let def = app.def.load();
    let schema = def.params.get("nonce").expect("param declared");
    assert_eq!(schema.secret, None);
    assert!(!schema.is_secret());
}

// spec: ACN#param-kinds
// spec: PARAM#secret-parameters
#[test]
fn param_schema_random_kind_with_secret_true_is_secret() {
    let app = run_test_script_app(r#"app.param("auth-secret").kind("random").secret(true);"#);
    let def = app.def.load();
    let schema = def.params.get("auth-secret").expect("param declared");
    assert_eq!(schema.secret, Some(true));
    assert!(schema.is_secret());
}

// spec: PARAM#secret-parameters
#[test]
fn a_secret_boolean_or_choice_param_fails_the_evaluation_once_it_completes() {
    for script in [
        // Declared in either order, the pair is checked once both are known.
        r#"app.param("flag").kind("boolean").secret(true);"#,
        r#"app.param("flag").secret(true).kind("boolean");"#,
        r#"app.param("flag").kind(#{ name: "choice", options: ["a", "b"] }).secret(true);"#,
    ] {
        let err = evaluation_error(script, &[]).unwrap_or_else(|| panic!("{script}"));
        assert!(
            err.contains("parameter 'flag'") && err.contains("cannot be secret"),
            "{script}: {err}"
        );
    }
    for script in [
        r#"app.param("flag").secret(true).kind("boolean").secret(false);"#,
        r#"app.param("flag").kind("password").kind("boolean");"#,
        r#"app.param("pin").kind("integer").secret(true);"#,
    ] {
        assert_eq!(evaluation_error(script, &[]), None, "{script}");
    }
}

// spec: PARAM#validators
// spec: APP#validator-immutability
// Only declarations threw in a validator, so a builder call on a captured
// handle, or any other method that changes the app, went through.
#[test]
fn a_validator_cannot_change_the_app_or_its_resources() {
    let (engine, mut scope, _app, _ast) = run_test_script(
        r#"
        let dep = app.deployment("api");
        let job = app.job("migrate");
        let svc = app.service("web");
        let svc_http = svc.http(80);
        let vol = app.volume("data");
        let ext_vol = app.external_volume("shared");
        let ext_svc = app.external_service("upstream");
        let ing = svc.ingress("example.com", 443);
        let declared = app.param("declared");
        let act = app.on_action("mutate", |rt, p| {});
    "#,
    );
    let _frame = crate::defs::app::ValidatorFrame::enter();

    for statement in [
        r#"app.deployment("other")"#,
        r#"app.deployment("api")"#,
        r#"app.job("other")"#,
        r#"app.service("other")"#,
        r#"app.volume("other")"#,
        r#"app.external_volume("other")"#,
        r#"app.external_service("other")"#,
        r#"app.param("undeclared")"#,
        r#"app.description("changed")"#,
        r#"app.canopy_type("tamanu-central")"#,
        r#"app.on_action("other", |rt, p| {})"#,
        r#"app.on_shell("other", |s, p| {})"#,
        r#"dep.image("registry.example/api:1")"#,
        r#"dep.scale(2)"#,
        r#"dep.env("KEY", "value")"#,
        r#"dep.arg("--flag")"#,
        r#"dep.command("run")"#,
        r#"dep.mount("/data", vol)"#,
        r#"dep.stop_signal("SIGTERM")"#,
        r#"dep.description("changed")"#,
        r#"job.image("registry.example/migrate:1")"#,
        r#"job.deadline(60)"#,
        r#"svc.http(8080)"#,
        r#"svc.description("changed")"#,
        r#"svc.ingress("other.example.com", 443)"#,
        r#"svc.exported()"#,
        r#"svc_http.route("/api")"#,
        r#"svc_http.ingress("another.example.com", 443)"#,
        r#"vol.description("changed")"#,
        r#"vol.tmpfs()"#,
        r#"ext_vol.description("changed")"#,
        r#"ext_svc.description("changed")"#,
        r#"ing.description("changed")"#,
        r#"declared.description("changed")"#,
        r#"declared.required(true)"#,
        r#"declared.default_value("x")"#,
        r#"declared.secret(true)"#,
        r#"declared.kind("text")"#,
        r#"declared.validate(|v, all| {})"#,
        r#"act.on_schedule("0 0 * * *")"#,
    ] {
        let err = engine
            .eval_with_scope::<rhai::Dynamic>(&mut scope, statement)
            .expect_err(statement);
        assert!(
            err.to_string().contains("a validator must not change"),
            "{statement}: {err}"
        );
    }

    // Reading is not changing.
    let is_set = engine
        .eval_with_scope::<bool>(&mut scope, r#"app.param("declared").is_set()"#)
        .expect("a validator may look up a declared parameter");
    assert!(!is_set);
}

// -----------------------------------------------------------------------
// kinds that take arguments, parsed values and typed validators
// -----------------------------------------------------------------------

fn kind_of(source: &str, name: &str) -> crate::defs::param_def::ParamKind {
    let app = run_test_script_app(source);
    let def = app.def.load();
    def.params
        .get(name)
        .unwrap_or_else(|| panic!("{name} declared"))
        .kind
        .clone()
}

/// Evaluate `script` with `stored`, giving the evaluation's error, if any.
fn evaluation_error(script: &str, stored: &[(&str, &str)]) -> Option<String> {
    let stored = stored
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let (_, err) = crate::runtime::apps::evaluate_script(
        &seedling_protocol::names::AppName::new_unchecked("demo"),
        script,
        &stored,
        &crate::ScriptLimits::default(),
    );
    err.map(|e| e.to_string())
}

/// The rejections validating `proposed` against `script` gives, by name.
fn rejections(
    script: &str,
    proposed: &[(&str, crate::defs::param_def::SuppliedValue)],
) -> Vec<(String, String)> {
    let proposed: std::collections::BTreeMap<_, _> = proposed
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    let (_, outcome) = crate::runtime::apps::evaluate_validated(
        &seedling_protocol::names::AppName::new_unchecked("demo"),
        &Arc::new(crate::runtime::definition::Bundle::from_stored_script(
            script,
        )),
        &crate::runtime::apps::canonical_values(&proposed),
        &proposed,
        &crate::runtime::seeds::AppSeed::for_tests(),
        &crate::ScriptLimits::default(),
    );
    outcome
        .expect("evaluates")
        .into_iter()
        .map(|r| (r.name, r.reason))
        .collect()
}

fn text(value: &str) -> crate::defs::param_def::SuppliedValue {
    crate::defs::param_def::SuppliedValue::Text(value.to_owned())
}

// spec: PARAM#schema
// spec: ACN#kind-arguments
#[test]
fn kind_takes_a_kind_name_or_a_kind_map() {
    use crate::defs::param_def::{IntRange, ParamKind};

    assert_eq!(
        kind_of(r#"app.param("flag").kind("boolean");"#, "flag"),
        ParamKind::Boolean
    );
    assert_eq!(
        kind_of(r#"app.param("num").kind(#{ name: "boolean" });"#, "num"),
        ParamKind::Boolean
    );
    assert_eq!(
        kind_of(r#"app.param("num").kind("integer");"#, "num"),
        ParamKind::Integer { range: None }
    );
    assert_eq!(
        kind_of(
            r#"app.param("port").kind(#{ name: "integer", range: 1..=65535 });"#,
            "port"
        ),
        ParamKind::Integer {
            range: IntRange::new(1, 65535, true)
        }
    );
    assert_eq!(
        kind_of(r#"app.param("size").kind("quantity");"#, "size"),
        ParamKind::Quantity { range: None }
    );
    assert_eq!(
        kind_of(
            r#"app.param("size").kind(#{ name: "quantity", range: 1..=1024 });"#,
            "size"
        ),
        ParamKind::Quantity {
            range: IntRange::new(1, 1024, true)
        }
    );
    assert_eq!(
        kind_of(
            r#"app.param("tier").kind(#{ name: "choice", options: ["small", "large"] });"#,
            "tier"
        ),
        ParamKind::Choice {
            options: vec!["small".into(), "large".into()]
        }
    );
}

// spec: ACN#kind-arguments
#[test]
fn kind_refuses_a_kind_map_that_is_not_valid() {
    for (kind, reason) in [
        (r#"#{ name: "integer", options: ["a"] }"#, "unknown key"),
        (r#"#{ name: "text", range: 1..2 }"#, "unknown key"),
        (r#"#{ name: "integer", range: 3..3 }"#, "holds no integer"),
        (r#"#{ name: "integer", range: "1..2" }"#, "integer range"),
        (r#"#{ name: "quantity", range: 3..3 }"#, "holds no integer"),
        (r#"#{ name: "quantity", options: ["a"] }"#, "unknown key"),
        (r#""choice""#, "options"),
        (r#"#{ name: "choice" }"#, "options"),
        (r#"#{ name: "choice", options: [] }"#, "at least one option"),
        (r#"#{ name: "choice", options: [""] }"#, "an empty option"),
        (
            r#"#{ name: "choice", options: ["a", "a"] }"#,
            "more than once",
        ),
        (r#"#{ range: 1..2 }"#, "without a `name`"),
        (r#"#{ name: "integr" }"#, "unknown param kind"),
        (
            r#"#{ name: "volume" }"#,
            "not allowed on the static parameter",
        ),
    ] {
        let err = run_test_script_err(&format!(r#"app.param("prm").kind({kind});"#)).to_string();
        assert!(err.contains(reason), "{kind}: {err}");
    }
}

// spec: PARAM#values
#[test]
fn parsed_gives_the_parser_result_of_the_kind_as_declared() {
    let script = r#"
        let flag = app.param("flag").kind("boolean");
        let port = app.param("port").kind(#{ name: "integer", range: 1..=65535 });
        let tier = app.param("tier").kind(#{ name: "choice", options: ["small", "large"] });
        let contact = app.param("contact").kind("email");
        let size = app.param("size").kind("quantity");
        if flag.parsed() != true { throw "flag"; }
        if size.parsed() != 1536 { throw "size"; }
        if port.parsed() != 8080 { throw "port"; }
        if tier.parsed() != "large" { throw "tier"; }
        if contact.parsed().domain != "example.org" { throw "contact"; }
    "#;
    assert_eq!(
        evaluation_error(
            script,
            &[
                ("flag", "true"),
                ("port", "8080"),
                ("tier", "large"),
                ("contact", "ops@example.org"),
                ("size", "1.5Ki")
            ]
        ),
        None
    );
}

// spec: PARAM#values
#[test]
fn parsed_throws_where_the_kind_rejects_the_value_or_none_is_set() {
    let rejected = evaluation_error(
        r#"app.param("port").kind(#{ name: "integer", range: 1..=10 }).parsed();"#,
        &[("port", "11")],
    )
    .expect("11 is out of range");
    assert!(rejected.contains("between 1 and 10"), "{rejected}");

    let rejected = evaluation_error(
        r#"app.param("size").kind(#{ name: "quantity", range: 1024..=1048576 }).parsed();"#,
        &[("size", "2Mi")],
    )
    .expect("2Mi is out of range");
    assert!(rejected.contains("between 1Ki and 1Mi"), "{rejected}");

    let unset = evaluation_error(r#"app.param("port").kind("integer").parsed();"#, &[])
        .expect("nothing to parse");
    assert!(unset.contains("not set"), "{unset}");
}

// spec: PARAM#schema
#[test]
fn default_value_takes_a_boolean_or_integer_as_its_canonical_form() {
    let app = run_test_script_app(
        r#"
        app.param("flag").kind("boolean").default_value(true);
        app.param("num").kind("integer").default_value(-3);
        app.param("size").kind("quantity").default_value(536870912);
        app.param("cleared").default_value("x").default_value("");
        "#,
    );
    let def = app.def.load();
    let default = |name: &str| def.params.get(name).unwrap().default_text();
    assert_eq!(default("flag").as_deref(), Some("true"));
    assert_eq!(default("num").as_deref(), Some("-3"));
    assert_eq!(default("size").as_deref(), Some("536870912"));
    assert_eq!(default("cleared"), None);
    assert!(
        run_test_script_err(r#"app.param("xyz").default_value(1.5);"#)
            .to_string()
            .contains("a string, a boolean or an integer")
    );
}

// spec: PARAM#kind-checks
#[test]
fn a_default_its_kind_rejects_fails_the_evaluation_once_it_completes() {
    for script in [
        // Declared in either order, the pair is checked once both are known.
        r#"app.param("num").default_value("ten").kind("integer");"#,
        r#"app.param("num").kind("integer").default_value("ten");"#,
        r#"app.param("num").default_value(true).kind("integer");"#,
        r#"app.param("num").default_value("512m").kind("quantity");"#,
        r#"app.param("num").kind(#{ name: "quantity", range: 1..=10 }).default_value(11);"#,
        r#"app.param("num").default_value(3);"#,
    ] {
        let err = evaluation_error(script, &[]).unwrap_or_else(|| panic!("{script}"));
        assert!(err.contains("parameter 'num'"), "{script}: {err}");
    }
    assert_eq!(
        evaluation_error(
            r#"app.param("num").default_value("ten"); app.param("num").kind("integer").default_value(10);"#,
            &[]
        ),
        None,
        "a default replaced before the evaluation ends is not checked"
    );
}

// spec: ACN#param-schemas
#[test]
fn a_secret_boolean_or_choice_action_param_throws_at_declaration() {
    for params in [
        r#"#{ flag: #{ kind: "boolean", secret: true } }"#,
        r#"#{ flag: #{ kind: #{ name: "choice", options: ["a"] }, secret: true } }"#,
    ] {
        let err = run_test_script_err(&format!(
            r#"app.on_action("run", |rt, p| {{}}, #{{ params: {params} }});"#
        ))
        .to_string();
        assert!(
            err.contains("param `flag` secret") && err.contains("cannot be secret"),
            "{params}: {err}"
        );
    }
    let app = run_test_script_app(
        r#"app.on_action("run", |rt, p| {}, #{ params: #{
            flag: #{ kind: "boolean", secret: false },
            pin: #{ kind: "integer", secret: true },
        } });"#,
    );
    let def = app.def.load();
    let params = &def.actions.get("run").unwrap().params;
    assert!(params.get("pin").unwrap().is_secret());
}

// spec: ACN#param-schemas
#[test]
fn an_action_param_default_its_kind_rejects_throws_at_declaration() {
    for params in [
        r#"#{ num: #{ kind: "integer", default_value: "ten" } }"#,
        r#"#{ num: #{ kind: "text", default_value: 3 } }"#,
        r#"#{ num: #{ kind: "integer", default_value: true } }"#,
        r#"#{ num: #{ kind: #{ name: "choice", options: ["a"] }, default_value: "b" } }"#,
    ] {
        let err = run_test_script_err(&format!(
            r#"app.on_action("run", |rt, p| {{}}, #{{ params: {params} }});"#
        ))
        .to_string();
        assert!(err.contains("default_value"), "{params}: {err}");
    }
    let app = run_test_script_app(
        r#"app.on_action("run", |rt, p| {}, #{ params: #{
            flag: #{ kind: "boolean", default_value: false },
            num: #{ kind: "integer", default_value: 4 },
            empty: #{ kind: "integer", default_value: "" },
        } });"#,
    );
    let def = app.def.load();
    let params = &def.actions.get("run").unwrap().params;
    let default = |name: &str| params.get(name).unwrap().default_text();
    assert_eq!(default("flag").as_deref(), Some("false"));
    assert_eq!(default("num").as_deref(), Some("4"));
    assert_eq!(default("empty"), None);
}

// spec: PARAM#validators
#[test]
fn validators_receive_parser_results_of_declared_set_parameters() {
    let script = r#"
        app.param("port").kind("integer").validate(|value, all| {
            if type_of(value) != "i64" { throw `value is ${type_of(value)}`; }
            if type_of(all.flag) != "bool" { throw `flag is ${type_of(all.flag)}`; }
            if all.contact.domain != "example.org" { throw "contact"; }
            if "stray" in all { throw "an undeclared parameter is in all"; }
        });
        app.param("flag").kind("boolean");
        app.param("contact").kind("email");
    "#;
    assert_eq!(
        rejections(
            script,
            &[
                ("port", crate::defs::param_def::SuppliedValue::Int(80)),
                ("flag", text("true")),
                ("contact", text("ops@example.org")),
                ("stray", text("x")),
            ]
        ),
        vec![]
    );
}

// spec: PARAM#kind-checks
#[test]
fn no_validator_runs_while_any_kind_rejects_its_value() {
    let script = r#"
        app.param("name").validate(|value, all| { throw "the validator ran"; });
        app.param("flag").kind("boolean");
    "#;
    assert_eq!(
        rejections(script, &[("name", text("x")), ("flag", text("yes"))]),
        vec![("flag".to_owned(), "must be true or false".to_owned())]
    );
}

// spec: PRM#validation
#[test]
fn a_native_value_fails_validation_for_a_kind_that_is_not_its_own() {
    use crate::defs::param_def::SuppliedValue;

    let script = r#"app.param("name"); app.param("flag").kind("boolean");"#;
    assert_eq!(
        rejections(
            script,
            &[
                ("name", SuppliedValue::Bool(true)),
                ("flag", SuppliedValue::Bool(true)),
                ("ghost", SuppliedValue::Int(1)),
            ]
        ),
        vec![
            (
                "ghost".to_owned(),
                "a boolean or integer is given for a parameter the definition does not declare"
                    .to_owned()
            ),
            (
                "name".to_owned(),
                "a boolean is given for a param of kind text".to_owned()
            ),
        ]
    );
}
