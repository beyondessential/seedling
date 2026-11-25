use super::*;

// spec: ACN#action-declaration
#[test]
fn on_action_registers_and_returns_action() {
    let app = run_test_script_app(
        r#"
        let a = app.on_action("migrate", |rt, _param| {}, #{
            description: "Run migrations",
        });
    "#,
    );
    let def = app.def.load();
    let action = def.actions.get("migrate").expect("action should exist");
    assert_eq!(action.description.as_deref(), Some("Run migrations"));
}

// spec: ACN#action-declaration
#[test]
fn on_action_without_options() {
    let app = run_test_script_app(
        r#"
        app.on_action("cleanup", |rt, _param| {});
    "#,
    );
    let def = app.def.load();
    let action = def.actions.get("cleanup").expect("action should exist");
    assert!(action.description.is_none());
}

// spec: ACN#shells
// spec: ACN#shell-control
#[test]
fn on_shell_registers_shell() {
    let app = run_test_script_app(
        r#"
        app.on_shell("node", |rt, shell, _param| {
            shell.attach(app.job("shell-node").image("docker.io/library/node:20").command("node"));
        }, #{
            description: "Node REPL",
        });
    "#,
    );
    let def = app.def.load();
    let shell = def.shells.get("node").expect("shell should exist");
    assert_eq!(shell.description.as_deref(), Some("Node REPL"));
}

// spec: ACN#shells
#[test]
fn on_shell_without_options() {
    let app = run_test_script_app(
        r#"
        app.on_shell("dbs", |rt, shell, _param| {
            shell.attach(app.job("shell-dbs").image("docker.io/library/psql:latest").command("psql"));
        });
    "#,
    );
    let def = app.def.load();
    let shell = def.shells.get("dbs").expect("shell should exist");
    assert!(shell.description.is_none());
}

// spec: ACN#shells
#[test]
fn shells_in_separate_namespace_from_actions() {
    let app = run_test_script_app(
        r#"
        app.on_action("debug", |rt, _param| {});
        app.on_shell("debug", |rt, shell, _param| {
            shell.attach(app.job("shell-debug").image("docker.io/library/tools:latest").command("sh"));
        });
    "#,
    );
    let def = app.def.load();
    assert!(def.actions.contains_key("debug"));
    assert!(def.shells.contains_key("debug"));
}

// spec: ACN#shells
#[test]
fn exercise_shell_return_job() {
    exercise(
        r#"
        app.on_shell("node", |rt| {
            app.job("shell-node").image("docker.io/library/node:20").command("node")
        });
    "#,
    );
}

// spec: ACN#param-schemas
// spec: ACN#param-kinds
#[test]
fn an_action_param_schema_declares_kinds_and_defaults() {
    let app = run_test_script_app(
        r#"
        app.on_action("setup", |rt, params| {}, #{
            params: #{
                "admin-email": #{
                    kind: "email",
                    description: "Admin email",
                    default_value: "admin@example.com",
                },
                "admin-password": #{
                    kind: "password",
                    description: "Admin password",
                },
            },
        });
    "#,
    );
    let def = app.def.load();
    let params = &def.actions["setup"].params;
    assert_eq!(params.len(), 2);
    let email = &params["admin-email"];
    assert!(matches!(email.kind, defs::param_def::ParamKind::Email));
    assert_eq!(email.default_text().as_deref(), Some("admin@example.com"));
    let pw = &params["admin-password"];
    assert!(matches!(pw.kind, defs::param_def::ParamKind::Password));
    assert!(pw.default_value.is_none());
}

// spec: ACN#param-schemas
// A `secret` the schema states wins over the kind's implication, in either
// direction, and one it leaves out defers to the kind.
#[test]
fn an_action_param_secret_overrides_its_kind() {
    let app = run_test_script_app(
        r#"
        app.on_action("setup", |rt, params| {}, #{
            params: #{
                "plain": #{ kind: "password" },
                "opted-out": #{ kind: "password", secret: false },
                "opted-out-weak": #{ kind: "weak-password", secret: false },
                "opted-in": #{ kind: "text", secret: true },
                "text": #{ kind: "text" },
            },
        });
    "#,
    );
    let def = app.def.load();
    let params = &def.actions["setup"].params;
    assert!(params["plain"].is_secret());
    assert!(!params["opted-out"].is_secret());
    assert!(!params["opted-out-weak"].is_secret());
    assert!(params["opted-in"].is_secret());
    assert!(!params["text"].is_secret());
}

// spec: ACN#param-kinds
#[test]
fn an_action_param_kind_is_text_unless_declared() {
    let app = run_test_script_app(
        r#"
        app.on_action("setup", |rt, params| {}, #{
            params: #{
                "site-name": #{ kind: "text" },
                "default-kind": #{ description: "Site name" },
                "motd": #{ kind: "multiline" },
                "api-key": #{ kind: "weak-password" },
            },
        });
    "#,
    );
    let def = app.def.load();
    let params = &def.actions["setup"].params;
    assert!(matches!(
        params["site-name"].kind,
        defs::param_def::ParamKind::Text
    ));
    assert!(matches!(
        params["default-kind"].kind,
        defs::param_def::ParamKind::Text
    ));
    assert!(matches!(
        params["motd"].kind,
        defs::param_def::ParamKind::Multiline
    ));
    assert!(matches!(
        params["api-key"].kind,
        defs::param_def::ParamKind::WeakPassword
    ));
}

// spec: ACN#param-schemas
#[test]
fn on_action_with_params() {
    let app = run_test_script_app(
        r#"
        app.on_action("maintenance", |rt, params| {}, #{
            params: #{
                "contact-email": #{
                    kind: "email",
                    description: "Contact email during maintenance",
                    default_value: "admin@example.com",
                },
            },
        });
    "#,
    );
    let def = app.def.load();
    let action = &def.actions["maintenance"];
    assert_eq!(action.params.len(), 1);
    let p = &action.params["contact-email"];
    assert!(matches!(p.kind, defs::param_def::ParamKind::Email));
    assert_eq!(p.default_text().as_deref(), Some("admin@example.com"));
}

// spec: ACN#param-schemas
#[test]
fn on_action_unknown_param_kind_throws() {
    let _ = run_test_script_err(
        r#"
        app.on_action("maintenance", |rt, params| {}, #{
            params: #{
                field: #{
                    kind: "banana",
                },
            },
        });
    "#,
    );
}

// spec: ACN#param-delivery
#[test]
fn on_action_closure_accepts_two_args() {
    run_test_script_app(
        r#"
        app.on_action("noop", |rt, param| {});
    "#,
    );
}

// spec: ACN#scheduled-actions
#[test]
fn on_schedule_registers_cron_on_action() {
    let (engine, mut scope, app) =
        crate::setup_language(&crate::ScriptLimits::default(), Default::default())
            .expect("the scope builds");
    crate::defs::app::set_appdef_holder(&app.def);
    super::run_script(
        &engine,
        &mut scope,
        r#"app.on_action("cleanup", |rt, _param| {}).on_schedule("H 2 * * *");"#,
    )
    .expect("script should evaluate");
    crate::defs::app::clear_appdef_holder();

    let def = app.def.load();
    let action = def.actions.get("cleanup").expect("action exists");
    assert_eq!(action.schedules, vec!["H 2 * * *".to_owned()]);
}

// spec: ACN#scheduled-actions
#[test]
fn schedule_hash_is_pinned() {
    use crate::defs::action::schedule_hash;
    use seedling_protocol::names::{ActionName, AppName};

    // These values are a contract with every deployed daemon: changing them
    // moves every `H` schedule. They must not follow the toolchain.
    let hash = |app: &str, action: &str| {
        schedule_hash(
            &AppName::new_unchecked(app),
            &ActionName::new_unchecked(action),
        )
    };
    assert_eq!(hash("tamanu", "backup"), 10722289103843889808);
    assert_eq!(hash("a", "b"), 17343052842552570643);
    assert_eq!(hash("", ""), 2147057924765956750);
    assert_ne!(hash("ab", "c"), hash("a", "bc"));
}

// spec: ACN#scheduled-actions
#[test]
fn on_schedule_chains_multiple_exprs() {
    let (engine, mut scope, app) =
        crate::setup_language(&crate::ScriptLimits::default(), Default::default())
            .expect("the scope builds");
    crate::defs::app::set_appdef_holder(&app.def);
    super::run_script(
        &engine,
        &mut scope,
        r#"
        app.on_action("heartbeat", |rt, _param| {})
            .on_schedule("*/5 * * * *")
            .on_schedule("0 0 * * *");
        "#,
    )
    .expect("script should evaluate");
    crate::defs::app::clear_appdef_holder();

    let def = app.def.load();
    let action = def.actions.get("heartbeat").expect("action exists");
    assert_eq!(
        action.schedules,
        vec!["*/5 * * * *".to_owned(), "0 0 * * *".to_owned()]
    );
}

// spec: ACN#scheduled-actions
#[test]
fn on_schedule_rejects_invalid_cron() {
    let _ = run_test_script_err(
        r#"
        app.on_action("bad", |rt, _param| {}).on_schedule("not a cron expr");
    "#,
    );
}

// spec: ACN#action-declaration
#[test]
fn install_and_start_are_ordinary_action_names() {
    let app = run_test_script_app(
        r#"
        app.on_action("install", |rt, _p| {});
        app.on_action("start", |rt, _p| {});
    "#,
    );
    let def = app.def.load();
    assert!(def.actions.contains_key("install"));
    assert!(def.actions.contains_key("start"));
}

// spec: ACN#action-declaration
#[test]
fn declaring_an_action_twice_throws() {
    let err = run_test_script_err(
        r#"app.on_action("migrate", |rt, _p| {}); app.on_action("migrate", |rt, _p| {});"#,
    );
    assert!(err.to_string().contains("already declared"), "{err}");
}

// spec: ACN#shells
#[test]
fn declaring_a_shell_twice_throws() {
    let err = run_test_script_err(
        r#"app.on_shell("psql", |rt, sh, _p| {}); app.on_shell("psql", |rt, sh, _p| {}, #{});"#,
    );
    assert!(err.to_string().contains("already declared"), "{err}");
}

// spec: ACN#action-declaration
// spec: ACN#shells
// spec: ACN#param-schemas
#[test]
fn unknown_option_and_param_definition_keys_throw() {
    for script in [
        r#"app.on_action("a", |rt, _p| {}, #{ descripton: "typo" });"#,
        r#"app.on_shell("s", |rt, sh, _p| {}, #{ param: #{} });"#,
        r#"app.on_action("a", |rt, _p| {}, #{ params: #{ x: #{ kinds: "text" } } });"#,
        r#"app.on_action("a", |rt, _p| {}, #{ params: "not a map" });"#,
        r#"app.on_action("a", |rt, _p| {}, #{ params: #{ x: "not a map" } });"#,
    ] {
        let err = run_test_script_err(script);
        assert!(!err.to_string().is_empty(), "{script} must throw");
    }
}

// spec: VOL#export
// spec: SVC#export
#[test]
fn unknown_keys_in_exported_options_throw() {
    for script in [
        r#"app.volume("data").exported(#{ descriptions: "x" });"#,
        r#"app.service("api").exported(#{ note: "x" });"#,
    ] {
        let err = run_test_script_err(script);
        assert!(err.to_string().contains("unknown key"), "{script}: {err}");
    }
}

// spec: CTR#environment
#[test]
fn an_env_entry_with_an_extra_key_throws() {
    let err = run_test_script_err(
        r#"app.deployment("web").image("docker.io/library/nginx:1")
            .env([#{ name: "A", value: "1", extra: "x" }]);"#,
    );
    assert!(err.to_string().contains("unknown key"), "{err}");
}

// spec: ACN#action-declaration
#[test]
fn on_action_with_a_named_function_throws() {
    assert_named_handler_rejected("on_action", r#"app.on_action("migrate", HANDLER);"#);
    assert_named_handler_rejected(
        "on_action",
        r#"app.on_action("migrate", HANDLER, #{ description: "x" });"#,
    );
}

// spec: ACN#shells
#[test]
fn on_shell_with_a_named_function_throws() {
    assert_named_handler_rejected("on_shell", r#"app.on_shell("bash", HANDLER);"#);
    assert_named_handler_rejected(
        "on_shell",
        r#"app.on_shell("bash", HANDLER, #{ description: "x" });"#,
    );
}

// spec: ACN#action-declaration
#[test]
fn a_closure_wrapping_a_named_function_registers_and_runs() {
    let (engine, mut scope, app, ast) = run_test_script(
        r#"
        fn handler(rt, param) { throw "handler ran"; }
        app.on_action("migrate", |rt, param| handler(rt, param));
        "#,
    );
    let log = InMemoryActionLog::new();
    let result = run_operation(
        action_context(
            &engine,
            &ast,
            &app,
            "migrate",
            &log,
            Arc::new(crate::runtime::TestWorldOracle::new()),
            ActionHooks::default(),
        ),
        &mut scope,
    );
    assert!(
        matches!(&result, OperationResult::Failed(e) if e.to_string().contains("handler ran")),
        "the wrapped handler should have been called"
    );
}
