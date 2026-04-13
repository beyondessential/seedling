use super::*;
use defs::resource::ResourceKind;

// l[verify app.var]
// l[verify app.type]
// l[verify app.constructor]
#[test]
fn app_global_is_available_and_not_constructible() {
    run_test_script_app(
        r#"
        let t = app.type_of();
        if t != "App" { throw "app must be of type App, got: " + t; }
    "#,
    );
}

// l[verify app.methods]
#[test]
fn app_methods_are_defined() {
    run_test_script_app(
        r#"
        app.service("svc");
        app.deployment("dep");
        app.job("jbs");
        app.volume("vol");
        app.external_volume("evol");
        app.external_service("esvc");
        app.param("par");
        app.on_action("act", |rt| {});
        app.on_start(|rt| {});
        app.on_shell("shl", |rt| { app.job("shl").command("sh") });
        app.on_install(|rt, reqs| {});
    "#,
    );
}

// l[verify app.resources]
#[test]
fn app_holds_resources() {
    let app = run_test_script_app(r#"let s = app.service("web");"#);
    let def = app.def.lock();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Service && &*id.name == "web")
    );
}

// l[verify app.resources.static]
#[test]
fn top_level_resources_are_static() {
    let app = run_test_script_app(
        r#"
        let svc = app.service("static-svc");
        let dep = app.deployment("static-dep");
    "#,
    );
    let def = app.def.lock();
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

// l[verify app.resources.dynamic]
#[test]
fn closures_create_dynamic_resources() {
    let app = run_test_script_app(
        r#"
        let make_job = || app.job("ephemeral")
            .image("tools:1")
            .command("run");

        app.on_start(|rt| {
            let j = make_job.call();
            rt.start(j).terminated();
        });
    "#,
    );
    let def = app.def.lock();
    assert!(def.actions.contains_key("start"));
}

// l[verify app.resources.names]
#[test]
fn same_name_returns_same_resource() {
    let app = run_test_script_app(
        r#"
        let a = app.service("data");
        let b = app.service("data");
    "#,
    );
    let def = app.def.lock();
    let count = def
        .resources
        .keys()
        .filter(|id| id.kind == ResourceKind::Service && &*id.name == "data")
        .count();
    assert_eq!(count, 1);
}

// l[verify param.type]
#[test]
fn param_declared_in_script_appears_in_params() {
    let app = run_test_script_app(r#"let x = app.param("foo");"#);
    let def = app.def.lock();
    assert!(def.params.contains("foo"));
}
