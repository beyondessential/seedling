use super::*;
use defs::resource::ResourceKind;

// spec: APP#the-app-variable
#[test]
fn app_global_is_available_and_not_constructible() {
    run_test_script_app(
        r#"
        let t = app.type_of();
        if t != "App" { throw "app must be of type App, got: " + t; }
    "#,
    );
}

// spec: APP#the-app-variable
#[test]
fn app_methods_are_defined() {
    run_test_script_app(
        r#"
        app.service("svc");
        app.deployment("dep");
        app.job("jbs");
        app.volume("vol");
        app.external_volume("evol");
        app.param("par");
        app.on_action("act", |rt, _param| {});
        app.on_start(|rt, _param| {});
        app.on_shell("shl", |rt, shell, _param| { shell.attach(app.job("shl").command("sh")); });
        app.on_install(|rt, reqs| {});
    "#,
    );
}

// spec: APP#named-resources
#[test]
fn app_holds_resources() {
    let app = run_test_script_app(r#"let s = app.service("web");"#);
    let def = app.def.load();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Service && &*id.name == "web")
    );
}

// spec: APP#static-resource
#[test]
fn top_level_resources_are_static() {
    let app = run_test_script_app(
        r#"
        let svc = app.service("static-svc");
        let dep = app.deployment("static-dep");
    "#,
    );
    let def = app.def.load();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Service && &*id.name == "static-svc")
    );
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Deployment && &*id.name == "static-dep")
    );
}

// spec: APP#dynamic-resource-lifetime
#[test]
fn closures_create_dynamic_resources() {
    let app = run_test_script_app(
        r#"
        let make_job = || app.job("ephemeral")
            .image("docker.io/library/tools:1")
            .command("run");

        app.on_start(|rt, _param| {
            let j = make_job.call();
            rt.start(j).terminated();
        });
    "#,
    );
    let def = app.def.load();
    assert!(def.actions.contains_key("start"));
}

// spec: APP#named-resources
#[test]
fn same_name_returns_same_resource() {
    let app = run_test_script_app(
        r#"
        let a = app.service("data");
        let b = app.service("data");
    "#,
    );
    let def = app.def.load();
    let count = def
        .resources
        .keys()
        .filter(|id| id.kind == ResourceKind::Service && &*id.name == "data")
        .count();
    assert_eq!(count, 1);
}

// spec: PARAM#declaration
#[test]
fn param_declared_in_script_appears_in_params() {
    let app = run_test_script_app(r#"let x = app.param("foo");"#);
    let def = app.def.load();
    assert!(def.params.contains_key("foo"));
}

// spec: APP#action-context
// spec: ACN#action-declaration
// spec: ACN#shells
// spec: ACN#install-action
// spec: APP#app-description
// spec: PARAM#schema
#[test]
fn declarations_throw_inside_an_action_closure() {
    let cases = [
        (r#"app.on_action("later", |rt, p| {});"#, "on_action"),
        (r#"app.on_start(|rt, p| {});"#, "on_start"),
        (r#"app.on_shell("shell", |rt, sh, p| {});"#, "on_shell"),
        (r#"app.on_install(|rt, p| {});"#, "on_install"),
        (r#"app.description("x");"#, "description"),
        (r#"p.kind("text");"#, "kind"),
        (r#"p.required(true);"#, "required"),
        (r#"p.default_value("1");"#, "default_value"),
        (r#"p.description("x");"#, "description"),
        (r#"p.secret(true);"#, "secret"),
    ];
    for (call, name) in cases {
        let (engine, mut scope, app, _) = run_test_script(r#"let p = app.param("version");"#);
        let before = app.def.load_full();
        let _guard = crate::runtime::barrier::runtime::ActionClosureGuard::new(
            std::sync::Arc::clone(&app.def),
            "op".to_owned(),
            Default::default(),
        );
        let err = engine
            .run_with_scope(&mut scope, call)
            .expect_err(call);
        assert!(
            err.to_string().contains(name) && err.to_string().contains("action closure"),
            "{call}: {err}"
        );
        let after = app.def.load_full();
        assert_eq!(
            before.actions.len() + before.shells.len(),
            after.actions.len() + after.shells.len(),
            "{call} declared something before throwing"
        );
        assert_eq!(before.install.is_some(), after.install.is_some(), "{call}");
        assert_eq!(before.description, after.description, "{call}");
    }
}
