use super::*;

// l[verify action.type]
// l[verify action.option-description]
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

// l[verify action.type]
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

// l[verify action.start]
#[test]
fn on_start_registers_start_action() {
    let app = run_test_script_app(
        r#"
        app.on_start(|rt, _param| {});
    "#,
    );
    let def = app.def.load();
    assert!(def.actions.contains_key("start"));
}

// l[verify action.start]
#[test]
fn on_start_with_options() {
    let app = run_test_script_app(
        r#"
        app.on_start(|rt, _param| {}, #{
            description: "Start the application",
        });
    "#,
    );
    let def = app.def.load();
    let action = def.actions.get("start").expect("start action should exist");
    assert_eq!(action.description.as_deref(), Some("Start the application"));
}

// l[verify action.shell]
// l[verify action.shell.attach]
// l[verify action.shell.control]
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

// l[verify action.shell]
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

// l[verify action.shell]
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

// l[verify action.shell]
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

// l[verify action.install]
// l[verify action.install.requirements]
// l[verify action.install.requirements.kind-email]
// l[verify action.install.requirements.kind-password]
#[test]
fn on_install_with_requirements() {
    let app = run_test_script_app(
        r#"
        app.on_install(|rt, reqs| {}, #{
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
    let install = def.install.as_ref().expect("install should exist");
    assert_eq!(install.requirements.len(), 2);
    let email_req = &install.requirements["admin-email"];
    assert!(matches!(email_req.kind, defs::install::ParamKind::Email));
    assert_eq!(
        email_req.default_value.as_deref(),
        Some("admin@example.com")
    );
    let pw_req = &install.requirements["admin-password"];
    assert!(matches!(pw_req.kind, defs::install::ParamKind::Password));
    assert!(pw_req.default_value.is_none());
}

// l[verify action.install]
#[test]
fn on_install_without_requirements() {
    let app = run_test_script_app(
        r#"
        app.on_install(|rt, reqs| {});
    "#,
    );
    let def = app.def.load();
    let install = def.install.as_ref().expect("install should exist");
    assert!(install.requirements.is_empty());
}

// l[verify action.install.requirements.kind-text]
#[test]
fn install_requirement_kind_text() {
    let app = run_test_script_app(
        r#"
        app.on_install(|rt, reqs| {}, #{
            params: #{
                "site-name": #{
                    kind: "text",
                    description: "Site name",
                },
            },
        });
    "#,
    );
    let def = app.def.load();
    let install = def.install.as_ref().unwrap();
    let req = &install.requirements["site-name"];
    assert!(matches!(req.kind, defs::install::ParamKind::Text));
}

// l[verify action.install.requirements.kind-text]
#[test]
fn install_requirement_kind_defaults_to_text() {
    let app = run_test_script_app(
        r#"
        app.on_install(|rt, reqs| {}, #{
            params: #{
                "site-name": #{
                    description: "Site name",
                },
            },
        });
    "#,
    );
    let def = app.def.load();
    let install = def.install.as_ref().unwrap();
    let req = &install.requirements["site-name"];
    assert!(matches!(req.kind, defs::install::ParamKind::Text));
}

// l[verify action.install.requirements.kind-multiline]
#[test]
fn install_requirement_kind_multiline() {
    let app = run_test_script_app(
        r#"
        app.on_install(|rt, reqs| {}, #{
            params: #{
                "motd": #{
                    kind: "multiline",
                    description: "Message of the day",
                },
            },
        });
    "#,
    );
    let def = app.def.load();
    let install = def.install.as_ref().unwrap();
    let req = &install.requirements["motd"];
    assert!(matches!(req.kind, defs::install::ParamKind::Multiline));
}

// l[verify action.install.requirements.kind-weak-password]
#[test]
fn install_requirement_kind_weak_password() {
    let app = run_test_script_app(
        r#"
        app.on_install(|rt, reqs| {}, #{
            params: #{
                "api-key": #{
                    kind: "weak-password",
                    description: "API key",
                },
            },
        });
    "#,
    );
    let def = app.def.load();
    let install = def.install.as_ref().unwrap();
    let req = &install.requirements["api-key"];
    assert!(matches!(req.kind, defs::install::ParamKind::WeakPassword));
}

// l[verify action.install]
#[test]
fn exercise_install_action() {
    exercise(
        r#"
        app.on_install(|rt, reqs| {
            rt.start(app.deployment("web").image("docker.io/library/nginx:latest")).ready();
        }, #{
            params: #{
                "admin-email": #{
                    kind: "email",
                    description: "Admin email",
                },
            },
        });
    "#,
    );
}

// l[verify action.install.requirements.kind-unknown]
#[test]
fn install_requirement_unknown_kind_throws() {
    let _ = run_test_script_err(
        r#"
        app.on_install(|rt, reqs| {}, #{
            params: #{
                field: #{
                    kind: "banana",
                },
            },
        });
    "#,
    );
}

// l[verify action.option-params]
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
    assert!(matches!(p.kind, defs::install::ParamKind::Email));
    assert_eq!(p.default_value.as_deref(), Some("admin@example.com"));
}

// l[verify action.option-params]
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

// l[verify action.params]
#[test]
fn on_action_closure_accepts_two_args() {
    run_test_script_app(
        r#"
        app.on_action("noop", |rt, param| {});
    "#,
    );
}

// l[verify action.schedule]
#[test]
fn on_schedule_registers_cron_on_action() {
    let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
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

// l[verify action.schedule]
#[test]
fn on_schedule_chains_multiple_exprs() {
    let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
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

// l[verify action.schedule]
#[test]
fn on_schedule_rejects_start_action() {
    let err = run_test_script_err(
        r#"
        app.on_start(|rt, _param| {}).on_schedule("0 0 * * *");
    "#,
    );
    let msg = err.to_string();
    assert!(
        msg.contains("start"),
        "error should mention start action: {msg}",
    );
}

// l[verify action.schedule]
#[test]
fn on_schedule_rejects_invalid_cron() {
    let _ = run_test_script_err(
        r#"
        app.on_action("bad", |rt, _param| {}).on_schedule("not a cron expr");
    "#,
    );
}
