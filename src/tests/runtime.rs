use super::*;

// l[verify rt.var]
// l[verify rt.type]
// l[verify rt.constructor]
#[test]
fn rt_available_in_actions() {
    exercise(
        r#"
        app.on_start(|rt| {
            let t = rt.type_of();
            if t != "RuntimeInstance" { throw "rt must be RuntimeInstance, got: " + t; }
        });
    "#,
    );
}

// l[verify rt.methods]
#[test]
fn rt_methods_are_defined() {
    exercise(
        r#"
        app.on_start(|rt| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            let started = rt.start(dep);
            started.ready();
            rt.query(dep);
            rt.stop(dep);
        });
    "#,
    );
}

// l[verify rt.lifecyle]
#[test]
fn rt_lifecycle_states_accessible() {
    exercise(
        r#"
        app.on_start(|rt| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            let started = rt.start(dep);
            started.scheduled();
            started.running();
            started.ready();
            started.terminated();
        });
    "#,
    );
}

// l[verify rt.start]
// l[verify rt.started.state-methods]
#[test]
fn exercise_start_action() {
    exercise(
        r#"
        app.on_start(|rt| {
            let svc = app.service("web");
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            rt.start(svc);
            rt.start(dep).ready();
        });
    "#,
    );
}

// l[verify rt.stop]
#[test]
fn exercise_stop() {
    exercise(
        r#"
        app.on_start(|rt| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            let started = rt.start(dep);
            started.ready();
            rt.stop(dep);
        });
    "#,
    );
}

// l[verify rt.stop]
#[test]
fn exercise_stop_with_deadline() {
    exercise(
        r#"
        app.on_start(|rt| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            rt.start(dep).ready();
            rt.stop(dep, 10);
        });
    "#,
    );
}

// l[verify rt.query]
#[test]
fn exercise_query() {
    exercise(
        r#"
        app.on_start(|rt| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            let queried = rt.query(dep);
            queried.ready();
        });
    "#,
    );
}

// l[verify rt.reconcile]
#[test]
fn exercise_reconcile() {
    exercise(
        r#"
        app.on_action("test-reconcile", |rt| {
            let svc = app.service("public");
            rt.reconcile(app, svc);
        });
    "#,
    );
}

// l[verify rt.started.type]
#[test]
fn started_is_a_collection() {
    exercise(
        r#"
        app.on_start(|rt| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            let svc = app.service("web");
            let started = rt.start(dep);
            started.one();
            started.only(svc);
            started.except(svc);
            started.select(#{});
        });
    "#,
    );
}

// l[verify rt.started.state-methods]
#[test]
fn started_state_methods_with_deadline() {
    exercise(
        r#"
        app.on_start(|rt| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            let started = rt.start(dep);
            started.scheduled(30);
            started.running(30);
            started.ready(30);
            started.terminated(30);
        });
    "#,
    );
}

// l[verify rt.started.terminated]
// l[verify rt.termination.type]
// l[verify rt.termination.ensure-success]
#[test]
fn exercise_terminated_ensure_success() {
    exercise(
        r#"
        app.on_start(|rt| {
            let job = app.job("init").image("docker.io/library/tools:latest").command("setup");
            rt.start(job).terminated().ensure_success();
        });
    "#,
    );
}

// l[verify rt.termination.type]
#[test]
fn termination_type_is_opaque() {
    exercise(
        r#"
        app.on_start(|rt| {
            let job = app.job("init").image("docker.io/library/tools:latest").command("setup");
            let term = rt.start(job).terminated();
            let t = term.type_of();
            if t != "Termination" { throw "expected Termination, got: " + t; }
        });
    "#,
    );
}
