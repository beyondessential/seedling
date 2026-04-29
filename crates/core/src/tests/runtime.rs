use super::*;

// l[verify rt.var]
// l[verify rt.type]
// l[verify rt.constructor]
#[test]
fn rt_available_in_actions() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
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
        app.on_start(|rt, _param| {
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
// l[verify rt.started.default-deadlines]
#[test]
fn rt_lifecycle_states_accessible() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
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
        app.on_start(|rt, _param| {
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
        app.on_start(|rt, _param| {
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
        app.on_start(|rt, _param| {
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
        app.on_start(|rt, _param| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            let queried = rt.query(dep);
            queried.ready();
        });
    "#,
    );
}

// l[verify rt.warm-certs]
#[test]
fn exercise_warm_certs() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
            let svc = app.service("public");
            let ing = svc.ingress("test.example.com", 443).tls();
            let warm = rt.warm_certs(ing);
            warm.ready();
        });
    "#,
    );
}

// l[verify rt.warm-images]
#[test]
fn exercise_warm_images() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            let warm = rt.warm_images(dep);
            warm.ready();
        });
    "#,
    );
}

// l[verify rt.warm-images]
#[test]
fn warm_images_accepts_dynamic_job_with_image() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
            let warm = rt.warm_images(app.job().image("ghcr.io/example/foo:1.2.3"));
            warm.ready();
        });
    "#,
    );
}

// l[verify rt.warm-images]
#[test]
fn warm_images_ignores_non_container_resources() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
            let svc = app.service("public");
            // Passing a non-container should not throw — just be ignored.
            let warm = rt.warm_images(svc);
            warm.ready();
        });
    "#,
    );
}

// l[verify rt.warm-certs]
#[test]
fn warm_certs_ignores_non_ingress_resources() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            // Passing a non-ingress should not throw — just be ignored.
            let warm = rt.warm_certs(dep);
            warm.ready();
        });
    "#,
    );
}

// l[verify rt.started.type]
#[test]
fn started_is_a_collection() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
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
        app.on_start(|rt, _param| {
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
        app.on_start(|rt, _param| {
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
        app.on_start(|rt, _param| {
            let job = app.job("init").image("docker.io/library/tools:latest").command("setup");
            let term = rt.start(job).terminated();
            let t = term.type_of();
            if t != "Termination" { throw "expected Termination, got: " + t; }
        });
    "#,
    );
}

// l[verify rt.started.ready-eventually]
#[test]
fn started_ready_eventually_is_callable() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            // ready_eventually should be callable; under the stubbed runtime
            // it resolves without suspending. Companion barrier-suspension
            // semantics are exercised by terminated_eventually tests.
            rt.start(dep).ready_eventually();
        });
    "#,
    );
}

// l[verify rt.restart]
#[test]
fn rt_restart_is_callable() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            rt.restart(dep);
        });
    "#,
    );
}

// l[verify rt.signal]
#[test]
fn rt_signal_is_callable() {
    exercise(
        r#"
        app.on_start(|rt, _param| {
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            let started = rt.start(dep);
            // Both canonical and bare signal names are accepted.
            rt.signal(started, "SIGTERM");
            rt.signal(started, "HUP");
        });
    "#,
    );
}
