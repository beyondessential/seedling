use super::*;

// l[verify bsl.syntax]
#[test]
fn bsl_is_rhai() {
    run_test_script_app(
        r#"
        let x = 1 + 2;
        let s = "hello " + "world";
        let arr = [1, 2, 3];
        let map = #{ key: "value" };
    "#,
    );
}

// l[verify bsl.scope]
#[test]
fn distinct_scope_per_script() {
    let (engine1, mut scope1, _app1) = setup();
    run_script(&engine1, &mut scope1, r#"let unique_var = 42;"#).unwrap();

    let (engine2, mut scope2, _app2) = setup();
    let result = run_script(&engine2, &mut scope2, r#"let x = unique_var;"#);
    assert!(
        result.is_err(),
        "second scope must not see first scope's variables"
    );
}

// l[verify bsl.enums]
#[test]
fn enums_are_constant_object_maps() {
    run_test_script_app(
        r#"
        let rolling = OnUpdate.Rolling;
        let replace = OnUpdate.Replace;
        let recreate = OnTerminate.Recreate;
        let restart = OnExit.Restart;
        let terminate = OnExit.Terminate;
        let restart_fail = OnExit.RestartOnFailure;
        let svc_type = ResourceType.Service;
        let dep_type = ResourceType.Deployment;
    "#,
    );
}

// l[verify bsl.errors]
#[test]
fn errors_can_be_caught_with_try_catch() {
    run_test_script_app(
        r#"
        let caught = false;
        try {
            app.service("web").port(-1);
        } catch(err) {
            caught = true;
        }
        if !caught { throw "expected exception from invalid port"; }
    "#,
    );
}

// l[verify bsl.errors]
#[test]
fn uncaught_error_fails_script() {
    run_test_script_err(r#"throw "deliberate failure";"#);
}

// l[verify bsl.name]
#[test]
fn name_rules_accepted() {
    run_test_script_app(
        r#"
        app.service("web-server");
        app.service("abc");
        app.service("my-long-service-name-here");
    "#,
    );
}

// l[verify bsl.port]
#[test]
fn port_validation_rejects_zero() {
    run_test_script_err(r#"app.service("web").port(0);"#);
}

// l[verify bsl.port]
#[test]
fn port_validation_rejects_upper_bound() {
    run_test_script_err(r#"app.service("web").port(65535);"#);
}

// l[verify bsl.port]
#[test]
fn port_validation_rejects_negative() {
    run_test_script_err(r#"app.service("web").port(-1);"#);
}

// l[verify bsl.port]
#[test]
fn port_validation_accepts_valid() {
    run_test_script_app(
        r#"
        app.service("web").port(1);
        app.service("web2").port(80);
        app.service("web3").port(65534);
    "#,
    );
}

// l[verify bsl.resource]
#[test]
fn resource_types_exist() {
    use defs::resource::ResourceKind;

    let app = run_test_script_app(
        r#"
        let svc = app.service("web");
        let dep = app.deployment("web");
        let job = app.job("task");
        let vol = app.volume("data");
    "#,
    );
    let def = app.0.lock();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Service)
    );
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Deployment)
    );
    assert!(def.resources.keys().any(|id| id.kind == ResourceKind::Job));
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Volume)
    );
}

// l[verify bsl.builder]
#[test]
fn builder_methods_chain() {
    run_test_script_app(
        r#"
        app.deployment("web")
            .image("nginx:latest")
            .command("nginx")
            .arg("-g")
            .arg("daemon off;")
            .env("PORT", "80")
            .scale(3)
            .on_update(OnUpdate.Rolling)
            .on_terminate(OnTerminate.Recreate);
    "#,
    );
}

// l[verify bsl.placeholder]
#[test]
fn placeholder_string_value() {
    let app = run_test_script_app(r#"let x = app.param("foo");"#);
    let def = app.0.lock();
    assert_eq!(def.params["foo"], "<placeholder>");
}
