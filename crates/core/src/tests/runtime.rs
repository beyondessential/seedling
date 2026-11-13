use super::*;

// spec: RT#handle-availability
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

// spec: RT
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

// spec: RT#barriers
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

// spec: RT#starting-resources
// spec: RT#barriers
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

// spec: RT#stopping-resources
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

// spec: RT#stopping-resources
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

// spec: RT#querying-resources
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

// spec: RT#certificate-warming
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

// spec: RT#image-warming
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

// spec: RT#image-warming
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

// spec: RT#image-warming
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

// spec: RT#certificate-warming
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

// spec: RT#started
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

// spec: RT#barriers
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

// spec: RT#barriers
// spec: RT#termination
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

// spec: RT#termination
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

// spec: RT#barriers
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

// spec: RT#deployment-restart
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

// spec: RT#signal-delivery
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

// spec: RT#certificate-warming
// spec: RT#image-warming
#[test]
fn warm_results_refuse_barriers_other_than_ready() {
    for (warm, barrier) in [
        ("rt.warm_certs(ing)", "running()"),
        ("rt.warm_certs(ing)", "scheduled(5)"),
        ("rt.warm_images(web)", "terminated()"),
        ("rt.warm_images(web)", "terminated_eventually()"),
    ] {
        let err = run_action_err(
            &format!(
                r#"
                let ing = app.service("public").ingress("test.example.com", 443)
                    .tls(Terminate.Https, Output.Http1);
                let web = app.deployment("web").image("docker.io/library/nginx:latest");
                app.on_action("warm", |rt, _p| {{ {warm}.{barrier}; }});
                "#
            ),
            "warm",
        );
        assert!(
            err.contains("use ready() or ready_eventually()"),
            "{warm}.{barrier}: {err}"
        );
    }
}

struct FailingSignaler;

impl crate::runtime::barrier::ContainerSignaler for FailingSignaler {
    fn signal(&self, _container_name: &str, _signal: &str) -> Result<bool, String> {
        Err("no such unit".to_owned())
    }
}

// spec: RT#signal-delivery
#[test]
fn a_signal_that_cannot_be_delivered_throws_and_is_never_recorded_as_delivered() {
    use crate::runtime::barrier::replay::ActionLog as _;
    let log = crate::runtime::barrier::replay::InMemoryActionLog::new();
    let result = run_action_with(
        r#"
        app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_action("poke", |rt, _p| { rt.signal(app.deployment("web"), "HUP"); });
    "#,
        &[],
        "poke",
        Some(&log),
        ActionHooks {
            container_signaler: Some(Arc::new(FailingSignaler)),
            ..Default::default()
        },
    );
    match result {
        OperationResult::Failed(e) => assert!(e.to_string().contains("delivering"), "{e}"),
        other => panic!("expected the signal to fail, got {other:?}"),
    }
    assert!(
        log.load()
            .unwrap()
            .iter()
            .filter(|e| e.call_kind == crate::runtime::barrier::CallKind::Signal)
            .all(|e| e.begun),
        "a signal that was not delivered is never on record as completed"
    );
}

// spec: RT#signal-delivery
#[test]
fn a_signal_to_a_selection_with_no_deployment_or_job_throws() {
    let err = run_action_err(
        r#"
        let svc = app.service("public");
        app.on_action("poke", |rt, _p| { rt.signal(svc, "HUP"); });
    "#,
        "poke",
    );
    assert!(err.contains("no deployment or job"), "{err}");
}

// spec: RT#selections
#[test]
fn an_array_is_a_selection_of_its_elements_and_other_values_are_not() {
    exercise(
        r#"
        let a = app.deployment("alpha").image("docker.io/library/nginx:1");
        let b = app.deployment("bravo").image("docker.io/library/nginx:1");
        app.on_start(|rt, _p| {
            rt.warm_images([a, b]);
            rt.start([a, [b]]);
        });
    "#,
    );
    for value in ["42", r#""web""#, "#{ a: 1 }"] {
        let err = run_action_err(
            &format!(r#"app.on_action("begin", |rt, _p| {{ rt.start({value}); }});"#),
            "begin",
        );
        assert!(err.contains("not a selection"), "{value}: {err}");
    }
}

// spec: ACN#start-action
#[test]
fn an_app_with_no_start_action_is_started_as_though_its_closure_called_rt_start() {
    use crate::runtime::barrier::replay::ActionLog as _;
    let log = crate::runtime::barrier::replay::InMemoryActionLog::new();
    let result = run_action_with(
        r#"
        app.deployment("web").image("docker.io/library/nginx:1");
        app.service("public");
    "#,
        &[],
        "start",
        Some(&log),
        ActionHooks::default(),
    );
    assert!(
        matches!(
            result,
            OperationResult::Completed | OperationResult::Suspended(_)
        ),
        "{result:?}"
    );
    let starts: Vec<_> = log
        .load()
        .unwrap()
        .into_iter()
        .filter(|e| e.call_kind == crate::runtime::barrier::CallKind::Start)
        .collect();
    assert_eq!(starts.len(), 1, "rt.start(app) is one call");
    assert!(
        starts[0]
            .resources
            .iter()
            .any(|r| r.name.as_deref() == Some("web")),
        "it starts the app's resources"
    );
}

/// Stands in for the host: records each effect, and whether the call's entry
/// was already on record, as begun, when the effect was made.
#[derive(Default)]
struct HostProbe {
    log: parking_lot::Mutex<Option<Arc<crate::runtime::barrier::replay::InMemoryActionLog>>>,
    effects: parking_lot::Mutex<Vec<(&'static str, bool)>>,
}

impl HostProbe {
    fn effect(&self, what: &'static str, kind: crate::runtime::barrier::CallKind) {
        use crate::runtime::barrier::replay::ActionLog as _;
        let log = self.log.lock().clone().expect("the probe is given the log");
        let begun_on_record = log
            .load()
            .unwrap()
            .iter()
            .any(|e| e.call_kind == kind && e.begun);
        self.effects.lock().push((what, begun_on_record));
    }
}

impl crate::runtime::barrier::ContainerSignaler for HostProbe {
    fn signal(&self, _container: &str, _signal: &str) -> Result<bool, String> {
        self.effect("signal", crate::runtime::barrier::CallKind::Signal);
        Ok(true)
    }
}

impl crate::runtime::barrier::VolumeWriter for HostProbe {
    fn write(
        &self,
        _app: &str,
        _target: crate::runtime::barrier::VolumeWriteTarget,
        _path: &str,
        _contents: &[u8],
    ) -> Result<(), String> {
        self.effect("write", crate::runtime::barrier::CallKind::Write);
        Ok(())
    }
}

impl crate::runtime::barrier::Executor for HostProbe {
    fn exec(
        &self,
        _target: &crate::runtime::ResourceInstance,
        _argv: &[String],
        _env: &[(String, String)],
    ) -> Result<i32, String> {
        self.effect("exec", crate::runtime::barrier::CallKind::Exec);
        Ok(3)
    }
}

const HOST_CALLS_SCRIPT: &str = r#"
    let web = app.deployment("web").image("docker.io/library/nginx:1");
    let data = app.volume("data");
    app.on_action("poke", |rt, _p| {
        rt.start(web);
        rt.signal(web, "HUP");
        rt.write(data, "/a.txt", "x");
        let out = rt.exec(web, ["echo", "hi"]);
        if out.exit_code() != 3 { throw `exit code was ${out.exit_code()}`; }
    });
"#;

fn run_poke(
    probe: &Arc<HostProbe>,
    log: &Arc<crate::runtime::barrier::replay::InMemoryActionLog>,
) -> OperationResult {
    *probe.log.lock() = Some(Arc::clone(log));
    run_action_with(
        HOST_CALLS_SCRIPT,
        &[],
        "poke",
        Some(log),
        ActionHooks {
            container_signaler: Some(Arc::clone(probe) as _),
            volume_writer: Some(Arc::clone(probe) as _),
            executor: Some(Arc::clone(probe) as _),
            ..Default::default()
        },
    )
}

// spec: SUSP#side-effecting-calls
#[test]
fn a_host_call_is_on_record_as_begun_before_its_effect_and_complete_before_the_closure_goes_on() {
    use crate::runtime::barrier::{CallKind, replay::ActionLog as _};
    let probe = Arc::new(HostProbe::default());
    let log = Arc::new(crate::runtime::barrier::replay::InMemoryActionLog::new());

    let result = run_poke(&probe, &log);
    assert!(matches!(result, OperationResult::Completed), "{result:?}");

    assert_eq!(
        *probe.effects.lock(),
        vec![("signal", true), ("write", true), ("exec", true)],
        "each call's begun entry was on record at its effect"
    );
    let entries = log.load().unwrap();
    for kind in [CallKind::Signal, CallKind::Write, CallKind::Exec] {
        let entry = entries.iter().find(|e| e.call_kind == kind).unwrap();
        assert!(!entry.begun, "{kind:?} is completed");
    }
    let exec = entries
        .iter()
        .find(|e| e.call_kind == CallKind::Exec)
        .unwrap();
    assert_eq!(exec.extra.as_deref(), Some("3"));
}

// spec: SUSP#side-effecting-calls
#[test]
fn a_replay_does_not_repeat_a_completed_call() {
    let probe = Arc::new(HostProbe::default());
    let log = Arc::new(crate::runtime::barrier::replay::InMemoryActionLog::new());
    run_poke(&probe, &log);
    probe.effects.lock().clear();

    let result = run_poke(&probe, &log);
    assert!(matches!(result, OperationResult::Completed), "{result:?}");
    assert!(probe.effects.lock().is_empty(), "nothing is repeated");
}

/// Run `poke` once, then mark the entry of `kind` as begun, as it is when the
/// runtime stops between beginning the call and recording its end.
fn replay_after_interruption(
    kind: crate::runtime::barrier::CallKind,
) -> (OperationResult, Vec<&'static str>) {
    use crate::runtime::barrier::replay::ActionLog as _;
    let probe = Arc::new(HostProbe::default());
    let log = Arc::new(crate::runtime::barrier::replay::InMemoryActionLog::new());
    run_poke(&probe, &log);

    let mut entries = log.load().unwrap();
    let at = entries
        .iter()
        .find(|x| x.call_kind == kind)
        .unwrap()
        .call_index;
    entries.retain(|e| e.call_index <= at);
    let target = entries.iter_mut().find(|e| e.call_kind == kind).unwrap();
    target.begun = true;
    if kind == crate::runtime::barrier::CallKind::Exec {
        target.extra = None;
    }
    let truncated = Arc::new(crate::runtime::barrier::replay::InMemoryActionLog::new());
    truncated.commit(&entries);
    probe.effects.lock().clear();

    let result = run_poke(&probe, &truncated);
    let effects = probe.effects.lock().iter().map(|(w, _)| *w).collect();
    (result, effects)
}

// spec: SUSP#side-effecting-calls
#[test]
fn a_replay_reaching_a_begun_signal_or_exec_throws_without_repeating_it() {
    use crate::runtime::barrier::CallKind;
    for (kind, name) in [(CallKind::Signal, "signal"), (CallKind::Exec, "exec")] {
        let (result, effects) = replay_after_interruption(kind);
        match result {
            OperationResult::Failed(e) => {
                assert!(e.to_string().contains("may already have"), "{name}: {e}");
            }
            other => panic!("{name}: expected the call to throw, got {other:?}"),
        }
        assert!(
            !effects.contains(&name),
            "{name} must not be repeated: {effects:?}"
        );
    }
}

// spec: SUSP#side-effecting-calls
#[test]
fn a_replay_reaching_a_begun_write_performs_it_again() {
    use crate::runtime::barrier::CallKind;
    let (result, effects) = replay_after_interruption(CallKind::Write);
    // The write is repeated; the exec after it is new work, not a replay.
    assert!(matches!(result, OperationResult::Completed), "{result:?}");
    assert_eq!(effects, vec!["write", "exec"]);
}
