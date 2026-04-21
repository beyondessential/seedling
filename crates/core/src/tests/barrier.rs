use std::sync::Arc;

use crate::{
    defs,
    defs::resource::ResourceKind,
    runtime::{
        ActionLog, EphemeralInstanceRegistry, LifecycleState, ResourceInstance, TestWorldOracle,
        barrier::OperationId,
        barrier::replay::{InMemoryActionLog, OperationContext, OperationResult, run_operation},
    },
};

fn setup_with_script(
    script: &str,
) -> (
    rhai::Engine,
    rhai::Scope<'static>,
    defs::app::App,
    rhai::AST,
) {
    let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
    let ast = super::run_script(&engine, &mut scope, script).expect("script should parse");
    (engine, scope, app, ast)
}

fn dep(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton("test-app", ResourceKind::Deployment, name)
}

fn ing(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton("test-app", ResourceKind::Ingress, name)
}

fn registry() -> Arc<dyn crate::runtime::InstanceRegistry> {
    Arc::new(EphemeralInstanceRegistry::new())
}

// r[verify barrier.suspension]
// r[verify barrier.condition]
#[test]
fn barrier_satisfied_on_first_pass() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.start(app.deployment("web")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Ready);

    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let result = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "start",
            log: &log,
            world: oracle,
            registry: registry(),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(result, OperationResult::Completed));
}

// r[verify barrier.suspension]
// r[verify barrier.resume]
#[test]
fn barrier_suspends_then_resumes() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.start(app.deployment("web")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();

    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: web is Pending → suspend
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op.clone(),
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    // Satisfy the condition
    oracle.set(dep("web"), LifecycleState::Ready);

    // Pass 2: barrier satisfied → complete
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Completed));
}

// r[verify barrier.suspension]
// r[verify barrier.resume]
// r[verify barrier.condition]
#[test]
fn sequential_barriers() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let frontend = app.deployment("frontend").image("docker.io/library/nginx:latest");
        let backend = app.deployment("backend").image("docker.io/library/api:latest");
        app.on_start(|rt, _param| {
            rt.start(app.deployment("frontend")).scheduled();
            rt.start(app.deployment("backend")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: frontend not Scheduled → suspend
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op.clone(),
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    oracle.set(dep("frontend"), LifecycleState::Scheduled);

    // Pass 2: first barrier ok, backend not Ready → suspend
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op.clone(),
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    oracle.set(dep("backend"), LifecycleState::Ready);

    // Pass 3: both satisfied → complete
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Completed));
}

// r[verify barrier.deadline]
#[test]
fn barrier_deadline_zero_expires_on_second_pass() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.start(app.deployment("web")).ready(0);
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: not ready → suspend
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op.clone(),
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    // Pass 2: deadline=0, time has elapsed → Failed
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Failed(_)));
}

// r[verify barrier.replay]
// r[verify reconciliation.idempotency]
// r[verify history.action-log.replay]
#[test]
fn replay_idempotency() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let a = app.deployment("aaa").image("docker.io/library/img:latest");
        let b = app.deployment("bbb").image("docker.io/library/img:latest");
        app.on_start(|rt, _param| {
            rt.start(app.deployment("aaa"));
            rt.start(app.deployment("bbb")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: bbb not ready → suspend
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op.clone(),
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    oracle.set(dep("bbb"), LifecycleState::Ready);

    // Pass 2: completes
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Completed));

    // No duplicate call_index entries in the log
    let entries = log.load().unwrap();
    let indices: Vec<usize> = entries.iter().map(|e| e.call_index).collect();
    let unique_count = {
        let mut v = indices.clone();
        v.sort();
        v.dedup();
        v.len()
    };
    assert_eq!(
        indices.len(),
        unique_count,
        "no duplicate call indices: {:?}",
        entries
    );
}

// r[verify barrier.replay.rt-stop]
#[test]
fn rt_stop_acts_as_barrier() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let old = app.deployment("old").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            let dep = app.deployment("old");
            rt.start(dep);
            rt.stop(dep);
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: dep not Terminated → stop suspends
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op.clone(),
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    oracle.set(dep("old"), LifecycleState::Terminated);

    // Pass 2: stop barrier satisfied → complete
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Completed));
}

// l[verify rt.warm-certs]
// r[verify observe.ingress.certs]
#[test]
fn warm_certs_barrier_uses_cert_oracle() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let svc = app.service("public");
        let ingress = svc.ingress("warm.example.com", 443).tls();
        app.on_start(|rt, _param| {
            let warm = rt.warm_certs(ingress);
            warm.ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    // Mark cert as valid for the ingress; the standard ingress lifecycle is
    // intentionally NOT set to Ready, so this proves .ready() consults
    // cert_valid_for rather than lifecycle_state.
    oracle.set_cert_valid(ing("public"));

    let log = InMemoryActionLog::new();
    let result = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: OperationId::new(),
            app: &app,
            action_name: "start",
            log: &log,
            world: oracle,
            registry: registry(),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Completed),
        "warm_certs.ready() should resolve when cert_valid_for returns true; got {result:?}"
    );
}

// l[verify rt.warm-certs]
#[test]
fn warm_certs_barrier_suspends_when_cert_not_valid() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let svc = app.service("public");
        let ingress = svc.ingress("warm.example.com", 443).tls();
        app.on_start(|rt, _param| {
            let warm = rt.warm_certs(ingress);
            warm.ready();
        });
    "#,
    );

    // Oracle reports no cert valid AND ingress lifecycle defaults to Pending.
    let oracle = Arc::new(TestWorldOracle::new());

    let log = InMemoryActionLog::new();
    let result = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: OperationId::new(),
            app: &app,
            action_name: "start",
            log: &log,
            world: oracle,
            registry: registry(),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Suspended(_)),
        "warm_certs.ready() should suspend when cert is not valid; got {result:?}"
    );
}

// r[verify barrier.suspension]
#[test]
fn stub_runtime_still_passes_language_tests() {
    // The existing exercise() helper uses RuntimeInstance::stub() (no context),
    // so all existing tests continue to work.
    use super::exercise;
    exercise(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.start(app.deployment("web")).ready();
        });
    "#,
    );
}

// l[verify rt.started.terminated-eventually]
// r[verify barrier.deadline]
#[test]
fn terminated_eventually_never_times_out() {
    // Even with a very large elapsed time, a deadline-less barrier must
    // stay in Suspended rather than failing with a deadline-exceeded error.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let j = app.job("worker").image("docker.io/library/busybox:latest").command(["sleep", "1"]);
        app.on_start(|rt, _param| {
            rt.start(app.job("worker")).terminated_eventually().ensure_success();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    // Leave the job in the default Pending state; the barrier must suspend.
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: suspend.
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op.clone(),
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    match r {
        OperationResult::Suspended(cond) => {
            assert_eq!(
                cond.deadline_secs, None,
                "terminated_eventually must record None deadline on the condition"
            );
        }
        other => panic!("expected Suspended with None deadline, got {other:?}"),
    }

    // The persisted log entry must also carry None, so replay after a
    // restart still produces a deadline-less barrier.
    let entries = log.load().unwrap();
    let barrier = entries[0].barrier.as_ref().expect("barrier recorded");
    assert_eq!(barrier.deadline_secs, None);

    // Pass 2: still pending and the recorded started_at is old enough that a
    // bounded 30s barrier would long since have tripped. A deadline-less
    // barrier must stay in Suspended, never Failed.
    {
        let mut es = log.load().unwrap();
        if let Some(b) = es[0].barrier.as_mut() {
            b.started_at_secs = Some(0); // epoch — "waited" for >50 years
        }
        log.commit(&es);
    }
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(
        matches!(r, OperationResult::Suspended(_)),
        "deadline-less barrier must not time out; got {r:?}"
    );
}

// r[verify operation.cancel]
#[test]
fn cancel_aborts_with_cancelled_result() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let j = app.job("worker").image("docker.io/library/busybox:latest").command(["sleep", "1"]);
        app.on_start(|rt, _param| {
            rt.start(app.job("worker")).terminated_eventually().ensure_success();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();
    // Pre-cancelled: any barrier entry must return Cancelled immediately.
    let cancel_token = Arc::new(crate::runtime::barrier::CancelToken::pre_cancelled());

    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token,
        },
        &mut scope,
    );
    assert!(
        matches!(r, OperationResult::Cancelled),
        "pre-cancelled token must produce OperationResult::Cancelled; got {r:?}"
    );
}

// r[verify barrier.suspension.poll-backoff]
#[test]
fn dynamic_poll_interval_follows_piecewise_schedule() {
    use crate::oi::handler::actions::lifecycle::dynamic_poll_interval;
    use std::time::Duration;
    // Lower band: 2s for small waits.
    assert_eq!(dynamic_poll_interval(0), Duration::from_secs(2));
    assert_eq!(dynamic_poll_interval(60), Duration::from_secs(2));
    assert_eq!(dynamic_poll_interval(120), Duration::from_secs(2));
    // Middle band: ramps up from 2s at 2 min to ~30s at 1 hour.
    assert!(dynamic_poll_interval(600) > Duration::from_secs(2));
    assert!(dynamic_poll_interval(600) < Duration::from_secs(30));
    // Near 1 hour: ~30s.
    let h1 = dynamic_poll_interval(3600).as_secs();
    assert!((28..=32).contains(&h1), "1h should be ~30s, got {h1}");
    // Higher band: ramps from 30s at 1h to ~300s at 6h.
    let h3 = dynamic_poll_interval(3 * 3600).as_secs();
    assert!((120..=180).contains(&h3), "3h should be 120-180s, got {h3}");
    // Cap: 300s beyond 6h.
    assert_eq!(dynamic_poll_interval(6 * 3600), Duration::from_secs(300));
    assert_eq!(dynamic_poll_interval(24 * 3600), Duration::from_secs(300));
}

// r[verify barrier.deadline]
// r[verify barrier.replay.rt-stop]
#[test]
fn rt_stop_deadline_is_enforced() {
    // Previously rt.stop() stored the deadline in the BarrierRecord but
    // never actually read it; a resource that refused to terminate left the
    // closure suspended indefinitely. Passing deadline=0 makes the second
    // pass fail immediately after the first pass records started_at, which
    // is the same shape as `barrier_deadline_zero_expires_on_second_pass`
    // uses for .ready().
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let old = app.deployment("old").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.stop(app.deployment("old"), 0);
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: the deployment is Pending → suspend, recording started_at.
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op.clone(),
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    // Pass 2: deadline=0 is exceeded immediately → Failed.
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::clone(&oracle),
            registry: Arc::clone(&reg),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        },
        &mut scope,
    );
    match r {
        OperationResult::Failed(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("deadline") && msg.to_lowercase().contains("terminated"),
                "expected deadline-exceeded error, got: {msg}"
            );
        }
        other => panic!("expected Failed with deadline message, got {other:?}"),
    }
}
