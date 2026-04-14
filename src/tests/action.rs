use super::*;

// l[verify action.type]
// l[verify action.option-description]
#[test]
fn on_action_registers_and_returns_action() {
    let app = run_test_script_app(
        r#"
        let a = app.on_action("migrate", |rt| {}, #{
            description: "Run migrations",
        });
    "#,
    );
    let def = app.def.lock();
    let action = def.actions.get("migrate").expect("action should exist");
    assert_eq!(action.description.as_deref(), Some("Run migrations"));
}

// l[verify action.type]
#[test]
fn on_action_without_options() {
    let app = run_test_script_app(
        r#"
        app.on_action("cleanup", |rt| {});
    "#,
    );
    let def = app.def.lock();
    let action = def.actions.get("cleanup").expect("action should exist");
    assert!(action.description.is_none());
}

// l[verify action.start]
#[test]
fn on_start_registers_start_action() {
    let app = run_test_script_app(
        r#"
        app.on_start(|rt| {});
    "#,
    );
    let def = app.def.lock();
    assert!(def.actions.contains_key("start"));
}

// l[verify action.start]
#[test]
fn on_start_with_options() {
    let app = run_test_script_app(
        r#"
        app.on_start(|rt| {}, #{
            description: "Start the application",
        });
    "#,
    );
    let def = app.def.lock();
    let action = def.actions.get("start").expect("start action should exist");
    assert_eq!(action.description.as_deref(), Some("Start the application"));
}

// l[verify action.shell]
#[test]
fn on_shell_registers_shell() {
    let app = run_test_script_app(
        r#"
        app.on_shell("node", |rt| {
            app.job("shell-node").image("docker.io/library/node:20").command("node")
        }, #{
            description: "Node REPL",
        });
    "#,
    );
    let def = app.def.lock();
    let shell = def.shells.get("node").expect("shell should exist");
    assert_eq!(shell.description.as_deref(), Some("Node REPL"));
}

// l[verify action.shell]
#[test]
fn on_shell_without_options() {
    let app = run_test_script_app(
        r#"
        app.on_shell("dbs", |rt| {
            app.job("shell-dbs").image("docker.io/library/psql:latest").command("psql")
        });
    "#,
    );
    let def = app.def.lock();
    let shell = def.shells.get("dbs").expect("shell should exist");
    assert!(shell.description.is_none());
}

// l[verify action.shell]
#[test]
fn shells_in_separate_namespace_from_actions() {
    let app = run_test_script_app(
        r#"
        app.on_action("debug", |rt| {});
        app.on_shell("debug", |rt| {
            app.job("shell-debug").image("docker.io/library/tools:latest").command("sh")
        });
    "#,
    );
    let def = app.def.lock();
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
            admin_email: #{
                kind: "email",
                description: "Admin email",
                default_value: "admin@example.com",
            },
            admin_password: #{
                kind: "password",
                description: "Admin password",
            },
        });
    "#,
    );
    let def = app.def.lock();
    let install = def.install.as_ref().expect("install should exist");
    assert_eq!(install.requirements.len(), 2);
    let email_req = &install.requirements["admin_email"];
    assert!(matches!(
        email_req.kind,
        defs::install::InstallRequirementKind::Email
    ));
    assert_eq!(
        email_req.default_value.as_deref(),
        Some("admin@example.com")
    );
    let pw_req = &install.requirements["admin_password"];
    assert!(matches!(
        pw_req.kind,
        defs::install::InstallRequirementKind::Password
    ));
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
    let def = app.def.lock();
    let install = def.install.as_ref().expect("install should exist");
    assert!(install.requirements.is_empty());
}

// l[verify action.install.requirements.kind-text]
#[test]
fn install_requirement_kind_text() {
    let app = run_test_script_app(
        r#"
        app.on_install(|rt, reqs| {}, #{
            site_name: #{
                kind: "text",
                description: "Site name",
            },
        });
    "#,
    );
    let def = app.def.lock();
    let install = def.install.as_ref().unwrap();
    let req = &install.requirements["site_name"];
    assert!(matches!(
        req.kind,
        defs::install::InstallRequirementKind::Text
    ));
}

// l[verify action.install.requirements.kind-text]
#[test]
fn install_requirement_kind_defaults_to_text() {
    let app = run_test_script_app(
        r#"
        app.on_install(|rt, reqs| {}, #{
            site_name: #{
                description: "Site name",
            },
        });
    "#,
    );
    let def = app.def.lock();
    let install = def.install.as_ref().unwrap();
    let req = &install.requirements["site_name"];
    assert!(matches!(
        req.kind,
        defs::install::InstallRequirementKind::Text
    ));
}

// l[verify action.install.requirements.kind-weak-password]
#[test]
fn install_requirement_kind_weak_password() {
    let app = run_test_script_app(
        r#"
        app.on_install(|rt, reqs| {}, #{
            api_key: #{
                kind: "weak-password",
                description: "API key",
            },
        });
    "#,
    );
    let def = app.def.lock();
    let install = def.install.as_ref().unwrap();
    let req = &install.requirements["api_key"];
    assert!(matches!(
        req.kind,
        defs::install::InstallRequirementKind::WeakPassword
    ));
}

// l[verify action.install]
#[test]
fn exercise_install_action() {
    exercise(
        r#"
        app.on_install(|rt, reqs| {
            rt.start(app.deployment("web").image("docker.io/library/nginx:latest")).ready();
        }, #{
            admin_email: #{
                kind: "email",
                description: "Admin email",
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
            field: #{
                kind: "banana",
            },
        });
    "#,
    );
}
