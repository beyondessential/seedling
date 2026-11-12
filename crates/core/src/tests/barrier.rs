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
    let (engine, mut scope, app) =
        crate::setup_language(&crate::ScriptLimits::default(), Default::default());
    let ast = super::run_script(&engine, &mut scope, script).expect("script should parse");
    (engine, scope, app, ast)
}

fn dep(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton(
        seedling_protocol::names::AppName::new("test-app").unwrap(),
        ResourceKind::Deployment,
        name,
    )
}

fn ing(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton(
        seedling_protocol::names::AppName::new("test-app").unwrap(),
        ResourceKind::Ingress,
        name,
    )
}

fn registry() -> Arc<dyn crate::runtime::InstanceRegistry> {
    Arc::new(EphemeralInstanceRegistry::new())
}

// spec: OPS#operation
// spec: SUSP#barriers
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(matches!(result, OperationResult::Completed));
}

// spec: SUSP#barriers
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Completed));
}

// spec: SUSP#barriers
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Completed));
}

// Reproducer for the postgres-tamanu install hang: a job whose container
// is auto-removed before the observer ever caught it Running has lifecycle
// Unscheduled, which `has_reached(Terminated)`, so the barrier should
// succeed. The committed log still records `barrier_satisfied=false`
// from the original suspension, however, and earlier code consulted the
// deadline before the oracle. With deadline=0, this caused a spurious
// timeout the moment the second pass ran. The fix consults the oracle
// first, so a barrier whose world state has reached the required level
// completes regardless of how much time has elapsed.
#[test]
fn barrier_succeeds_after_deadline_when_oracle_reached() {
    use crate::defs::resource::ResourceKind;
    use crate::runtime::lifecycle::LifecycleState;

    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let bin = app.volume("bin");
        app.on_install(|rt, _param| {
            rt.start(app.job().image("docker.io/library/debian:bookworm-slim")
                .mount("/app/bin", bin)
                .command(["true"])
            ).terminated(0).ensure_success();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: oracle says Pending → barrier suspends.
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op.clone(),
            app: &app,
            action_name: "install",
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    // Tell the oracle the job has terminated successfully (mirrors the
    // observed pattern: container_removed → running → health_check_pass →
    // container_removed, which derive_container_lifecycle resolves to
    // Unscheduled).
    let entries = <InMemoryActionLog as crate::runtime::barrier::replay::ActionLog>::load(&log)
        .expect("load");
    let instance = entries
        .iter()
        .find_map(|e| e.resources.first().cloned())
        .expect("resource");
    assert_eq!(instance.kind, ResourceKind::Job);
    oracle.set(instance.clone(), LifecycleState::Unscheduled);
    oracle.set_exit_code(instance, 0);

    // Pass 2: deadline=0 has trivially elapsed since pass 1 committed the
    // unsatisfied barrier, but the oracle says reached. Must complete.
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "install",
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(
        matches!(r, OperationResult::Completed),
        "expected Completed, got {r:?}",
    );
}

// spec: SUSP#deadlines
// spec: RT#barriers
// `ready(0)` means "use the default deadline", not "fail immediately".
// This used to assert the opposite: zero mapped to `Some(0)`, which the
// `elapsed >= d` check tripped on the very next pass, so a script asking for
// the documented default got a deadline failure about two seconds later.
#[test]
fn barrier_deadline_zero_uses_the_state_default() {
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    // Pass 2: deadline 0 resolved to the 30s default, so still waiting
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(
        matches!(r, OperationResult::Suspended(_)),
        "a zero deadline takes the state default, so this is still waiting: {r:?}",
    );
}

// spec: SUSP#replay
// spec: RCN#reconciliation-loop
// spec: HIST#action-calls
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
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

// spec: SUSP#rtstop
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Completed));
}

// spec: RT#certificate-warming
// spec: OBS#ingresses
#[test]
fn warm_certs_barrier_uses_cert_oracle() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let svc = app.service("public");
        let ingress = svc.ingress("warm.example.com", 443).tls(Terminate.Tls, Output.Tcp);
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
    // Ingresses are now keyed by `<hostname>:<port>` so multiple
    // ingresses on the same service have distinct identities.
    oracle.set_cert_valid(ing("warm.example.com:443"));

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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Completed),
        "warm_certs.ready() should resolve when cert_valid_for returns true; got {result:?}"
    );
}

// spec: RT#certificate-warming
#[test]
fn warm_certs_barrier_suspends_when_cert_not_valid() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let svc = app.service("public");
        let ingress = svc.ingress("warm.example.com", 443).tls(Terminate.Tls, Output.Tcp);
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Suspended(_)),
        "warm_certs.ready() should suspend when cert is not valid; got {result:?}"
    );
}

// spec: RT#image-warming
// spec: IMG#image-warming
#[test]
fn warm_images_barrier_uses_image_oracle() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        app.on_start(|rt, _param| {
            let warm = rt.warm_images(
                app.job().image("ghcr.io/example/foo:1.2.3")
            );
            warm.ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    // Image already present locally — .ready() should succeed even though
    // no resource lifecycle Ready observation exists.
    oracle.set_image_present("ghcr.io/example/foo:1.2.3");

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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Completed),
        "warm_images.ready() should resolve when image_present returns true; got {result:?}"
    );
}

// spec: RT#image-warming
#[test]
fn warm_images_barrier_suspends_when_image_absent() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        app.on_start(|rt, _param| {
            let warm = rt.warm_images(
                app.job().image("ghcr.io/example/foo:1.2.3")
            );
            warm.ready();
        });
    "#,
    );

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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(
        matches!(result, OperationResult::Suspended(_)),
        "warm_images.ready() should suspend when the image is not present locally; got {result:?}"
    );
}

// spec: SUSP#barriers
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

// spec: RT#barriers
// spec: SUSP#deadlines
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
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
    let barrier = entries[0].barrier(0).expect("barrier recorded");
    assert_eq!(barrier.deadline_secs, None);

    // Pass 2: still pending and the recorded started_at is old enough that a
    // bounded 30s barrier would long since have tripped. A deadline-less
    // barrier must stay in Suspended, never Failed.
    {
        let mut es = log.load().unwrap();
        for b in &mut es[0].barriers {
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(
        matches!(r, OperationResult::Suspended(_)),
        "deadline-less barrier must not time out; got {r:?}"
    );
}

// spec: OPS#cancellation
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(
        matches!(r, OperationResult::Cancelled),
        "pre-cancelled token must produce OperationResult::Cancelled; got {r:?}"
    );
}

// spec: SUSP#re-evaluation-cadence
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

// spec: SUSP#deadlines
// spec: SUSP#rtstop
#[test]
fn rt_stop_deadline_is_enforced() {
    // Previously rt.stop() stored the deadline in the BarrierRecord but
    // never actually read it; a resource that refused to terminate left the
    // closure suspended indefinitely.
    //
    // This used to force expiry with deadline=0, which `RT#stopping-resources` defines
    // as "use the default" rather than "expire at once" — so it now uses the
    // smallest real deadline and lets it actually elapse between the pass
    // that records `started_at` and the pass that checks it.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let old = app.deployment("old").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.stop(app.deployment("old"), 1);
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    // Let the 1s deadline actually elapse before the checking pass.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Pass 2: the 1s deadline has now been exceeded → Failed.
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
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

/// Run one pass of `start` for an operation, with the boilerplate of
/// `OperationContext` filled in from the pieces a replay test varies.
#[expect(
    clippy::too_many_arguments,
    reason = "one argument per piece of OperationContext a replay test varies"
)]
fn run_pass_with_db(
    engine: &rhai::Engine,
    ast: &rhai::AST,
    app: &defs::app::App,
    scope: &mut rhai::Scope<'static>,
    operation_id: OperationId,
    log: &InMemoryActionLog,
    world: Arc<TestWorldOracle>,
    registry: Arc<dyn crate::runtime::InstanceRegistry>,
    db: Option<crate::runtime::db::DbHandle>,
) -> OperationResult {
    run_operation(
        OperationContext {
            engine,
            script_ast: ast,
            operation_id,
            app,
            action_name: "start",
            log,
            world,
            registry,
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        scope,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "one argument per piece of OperationContext a replay test varies"
)]
fn run_pass(
    engine: &rhai::Engine,
    ast: &rhai::AST,
    app: &defs::app::App,
    scope: &mut rhai::Scope<'static>,
    operation_id: OperationId,
    log: &InMemoryActionLog,
    world: Arc<TestWorldOracle>,
    registry: Arc<dyn crate::runtime::InstanceRegistry>,
) -> OperationResult {
    run_pass_with_db(
        engine,
        ast,
        app,
        scope,
        operation_id,
        log,
        world,
        registry,
        None,
    )
}

fn job(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton(
        seedling_protocol::names::AppName::new("test-app").unwrap(),
        ResourceKind::Job,
        name,
    )
}

// spec: SUSP#deadlines
// spec: SUSP#rtstop
#[test]
fn rt_stop_deadline_start_is_read_from_the_calls_own_record() {
    // A named job resolves to one instance UUID for the whole operation, so
    // two stops of it carry byte-identical resources. The deadline's start
    // was found by resource equality, which handed the second stop the
    // first one's started_at and expired it on arrival. The first stop
    // here times out and is caught, leaving its unsatisfied record in the log.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let runner = app.job("runner").image("docker.io/library/busybox:latest");
        app.on_start(|rt, _param| {
            try {
                rt.stop(app.job("runner"), 1);
            } catch (e) {}
            rt.stop(app.job("runner"), 1);
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();
    oracle.set(job("runner"), LifecycleState::Running);

    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op.clone(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 1: {r:?}");

    std::thread::sleep(std::time::Duration::from_millis(1100));

    // The first stop's deadline has elapsed and is caught; the second has
    // only just started waiting, so the closure suspends rather than failing.
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op,
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(
        matches!(r, OperationResult::Suspended(_)),
        "the second stop took the first stop's start time: {r:?}"
    );
}

// spec: SUSP#barrier-anchoring
#[test]
fn terminated_barrier_does_not_short_circuit_against_an_earlier_stop() {
    // `do_stop` is the only writer of a satisfied record with
    // required_state=Terminated. A `.terminated()` barrier later in the same
    // closure resolves to the same instance UUID for a named job, so a fast
    // path matching on resources and required state alone took the stop's
    // record as its own and passed without the job having terminated.
    //
    // The deployment barrier in the middle is a suspension point, so the
    // world can move between the stop being satisfied and the later barrier
    // being checked — which is what the real system does when `rt.start`
    // brings the job back up.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        let runner = app.job("runner").image("docker.io/library/busybox:latest");
        app.on_start(|rt, _param| {
            rt.stop(app.job("runner"));
            rt.start(app.deployment("web")).ready();
            rt.start(app.job("runner")).terminated();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: the job is still running, so the stop suspends.
    oracle.set(job("runner"), LifecycleState::Running);
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op.clone(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 1: {r:?}");

    // Pass 2: the job has terminated, so the stop is satisfied and commits a
    // satisfied Terminated record. The deployment is not ready yet, so the
    // closure suspends again before reaching the `.terminated()` barrier.
    oracle.set(job("runner"), LifecycleState::Terminated);
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op.clone(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 2: {r:?}");

    // Pass 3: the deployment is ready, so the closure reaches
    // `.terminated()` — with the job running again. The barrier must consult
    // the world and suspend, not take the stop's record as its own.
    oracle.set(dep("web"), LifecycleState::Ready);
    oracle.set(job("runner"), LifecycleState::Running);
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op,
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(
        matches!(r, OperationResult::Suspended(_)),
        "the terminated() barrier took the earlier stop's record as its own: {r:?}"
    );
}

// spec: SUSP#barrier-anchoring
#[test]
fn a_barrier_that_passes_on_a_replay_pass_is_recorded_as_satisfied() {
    // Satisfaction was marked on pending entries only, and on a replay pass
    // pending is empty until the closure makes its first fresh rt.* call.
    // The first barrier's committed record therefore kept satisfied=false
    // and its original started_at for the rest of the operation, and
    // `earliest_unsatisfied_barrier_wait_secs` takes the maximum elapsed
    // across unsatisfied records — so a long first wait went on inflating
    // the re-poll cadence of every barrier after it.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        let api = app.deployment("api").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.start(app.deployment("web")).ready();
            rt.start(app.deployment("api")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: neither is ready, so the closure suspends on web's barrier.
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op.clone(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 1: {r:?}");

    // Pass 2: web is ready, so its barrier passes while the closure is
    // replaying; api is not, so the closure suspends on api's barrier.
    oracle.set(dep("web"), LifecycleState::Ready);
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op,
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 2: {r:?}");

    let entries = log.load().expect("log loads");
    let web_barrier = entries
        .iter()
        .find(|e| e.resources.iter().any(|r| r.name.as_deref() == Some("web")))
        .and_then(|e| e.barrier(0))
        .expect("web's call has a barrier record");
    assert!(
        web_barrier.satisfied,
        "web's barrier passed but its record still says it is waiting: {web_barrier:?}"
    );

    let unsatisfied: Vec<_> = entries
        .iter()
        .filter(|e| e.barrier(0).is_some_and(|b| !b.satisfied))
        .filter_map(|e| e.resources.first())
        .map(|r| r.name.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        unsatisfied,
        vec!["api".to_owned()],
        "only the barrier actually still waiting should count toward the poll cadence"
    );
}

// spec: OPS#sub-action-parameters
// spec: SUSP#positional-replay
//
// Lives here rather than beside the other `Action.invoke` tests because it
// needs the real replay context: the stub runtime those use has no action
// log to commit a call to or recover one from.
#[test]
fn a_replayed_sub_action_call_recovers_its_recorded_params() {
    // Validation answers to the world, which is why the entry carries the
    // post-validation params. Re-running it on every pass meant an update
    // that changed the sub-action's schema stopped a call that had already
    // been committed with valid params from replaying at all.
    let outer = |inner_params: &str| {
        format!(
            r#"
            let web = app.deployment("web").image("docker.io/library/nginx:latest");
            app.on_action("inner", |rt, p| {{
                if p["note"] != "hello" {{
                    throw "inner did not observe the recorded params";
                }}
            }}, #{{ params: #{{ {inner_params} }} }});
            app.on_start(|rt, _param| {{
                app.action("inner").invoke(#{{ note: "hello" }});
                rt.start(app.deployment("web")).ready();
            }});
        "#
        )
    };

    let (engine, mut scope, app, ast) = setup_with_script(&outer(r#""note": #{ kind: "text" },"#));

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: the sub-action call is committed, then the barrier suspends.
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op.clone(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 1: {r:?}");

    let recorded = log.load().expect("log loads");
    assert!(
        recorded.iter().any(|e| matches!(
            e.call_kind,
            crate::runtime::barrier::CallKind::SubAction
        ) && e.extra.as_deref().is_some_and(|x| x.contains("hello"))),
        "pass 1 should commit the call with its post-validation params: {recorded:?}"
    );

    // Pass 2: an update has given `inner` a second required param that the
    // call site does not supply, so validating again would throw. The call
    // is already committed, so it replays with the params it was committed
    // with and the operation finishes.
    let (engine, mut scope, app, ast) = setup_with_script(&outer(
        r#""note": #{ kind: "text" }, "target": #{ kind: "text" },"#,
    ));
    oracle.set(dep("web"), LifecycleState::Ready);
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op,
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(
        matches!(r, OperationResult::Completed),
        "a committed sub-action call was validated again instead of replayed: {r:?}"
    );
}

// spec: AUTO#scale
#[test]
fn an_action_addresses_every_replica_the_deployment_is_scaled_to() {
    // The replica set was sized from `scale.start`, the low end of the
    // declared range, so a deployment declared scale(2..10) and scaled to 5
    // resolved to 2 instances. A maintenance action that stopped it stopped
    // 2 of the 5 and carried on as though the workload were down.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .scale(2..=10);
        app.on_start(|rt, _param| {
            rt.start(app.deployment("web")).ready();
        });
    "#,
    );

    let db = crate::runtime::db::DbHandle::open_in_memory().expect("in-memory db");
    // The decision is keyed by the app the operation runs as, which is the
    // name on the app def rather than the one `dep()` builds instances with.
    let app_name = app.def.load().name.clone();
    db.call({
        let app_name = app_name.clone();
        move |db| crate::runtime::scaling::save_scaling_decision(db, &app_name, "web", 5)
    })
    .expect("scaling decision saves");

    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Ready);
    let log = InMemoryActionLog::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = Arc::new(
        crate::runtime::registry::DbInstanceRegistry::new(db.clone()),
    );

    let r = run_pass_with_db(
        &engine,
        &ast,
        &app,
        &mut scope,
        OperationId::new(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
        Some(db),
    );
    assert!(matches!(r, OperationResult::Completed), "{r:?}");

    let entries = log.load().expect("log loads");
    let started = entries
        .iter()
        .find(|e| matches!(e.call_kind, crate::runtime::barrier::CallKind::Start))
        .expect("the start call is recorded");
    assert_eq!(
        started.resources.len(),
        5,
        "the action reached {} of the 5 replicas the deployment is scaled to",
        started.resources.len()
    );
}

// spec: RT#started
// spec: COL#query-methods
#[test]
fn except_on_a_started_stops_waiting_for_the_excluded_resource() {
    // The collection methods returned the whole group whatever they were
    // given, so a resource the author excluded was still waited on — which
    // hung the operation, or failed its deadline, on a resource the script
    // had said to leave out.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        let optional = app.deployment("optional").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.start(app).except(app.deployment("optional")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    // `optional` never becomes Ready.
    oracle.set(dep("web"), LifecycleState::Ready);
    let log = InMemoryActionLog::new();

    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        OperationId::new(),
        &log,
        Arc::clone(&oracle),
        registry(),
    );
    assert!(
        matches!(r, OperationResult::Completed),
        "the barrier waited for the excluded resource: {r:?}"
    );
}

// spec: RT#started
// spec: COL#query-methods
// spec: COL#selection
#[test]
fn only_and_select_narrow_a_started_to_the_resources_they_name() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        let other = app.deployment("other").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            let started = rt.start(app);
            started.only(app.deployment("web")).ready();
            started.select(#{ names: ["web"] }).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Ready);
    let log = InMemoryActionLog::new();

    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        OperationId::new(),
        &log,
        Arc::clone(&oracle),
        registry(),
    );
    assert!(
        matches!(r, OperationResult::Completed),
        "only/select waited for a resource they did not name: {r:?}"
    );
}

// spec: RT#started
// spec: COL#query-methods
#[test]
fn one_on_a_started_yields_a_single_resource_and_null_when_empty() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        let other = app.deployment("other").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            let started = rt.start(app);
            if started.select(#{ names: ["web"] }).one() == () {
                throw "one() returned null for a non-empty group";
            }
            if started.select(#{ names: ["absent"] }).one() != () {
                throw "one() returned a group for an empty selection";
            }
            started.select(#{ names: ["web"] }).one().ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Ready);
    let log = InMemoryActionLog::new();

    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        OperationId::new(),
        &log,
        Arc::clone(&oracle),
        registry(),
    );
    assert!(matches!(r, OperationResult::Completed), "{r:?}");
}

// spec: SUSP#barrier-anchoring
// spec: SUSP#deadlines
#[test]
fn a_second_barrier_on_one_started_does_not_rearm_its_wait_each_pass() {
    // One log entry holds one barrier record, and both barriers on a
    // `Started` anchor to that entry. Handing the record to whichever
    // barrier is waiting rewrote its start on every pass, so the wait it
    // times restarted each time and the deadline could never expire.
    //
    // Which barrier ends up holding the record is not the point: the
    // waiting one does, so that it can be timed. The point is that its
    // start stands still.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            let started = rt.start(app.deployment("web"));
            started.running();
            started.ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Running);
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    let recorded_start = |log: &InMemoryActionLog| {
        let entries = log.load().expect("log loads");
        assert_eq!(entries.len(), 1, "one call, one entry: {entries:?}");
        entries[0].barrier(0).and_then(|b| b.started_at_secs)
    };
    let pass = |scope: &mut rhai::Scope<'static>| {
        let r = run_pass(
            &engine,
            &ast,
            &app,
            scope,
            op.clone(),
            &log,
            Arc::clone(&oracle),
            Arc::clone(&reg),
        );
        assert!(matches!(r, OperationResult::Suspended(_)), "{r:?}");
    };

    // The first pass reaches Running and records the Ready wait behind it.
    pass(&mut scope);
    let first = recorded_start(&log);
    assert!(first.is_some(), "the waiting barrier records a start");

    // A second of real time, so a start that was rewritten reads as a
    // different second rather than the same one.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    pass(&mut scope);
    assert_eq!(
        recorded_start(&log),
        first,
        "the wait was restarted on a later pass, so its deadline could never expire"
    );
}

// spec: GC#instances-started-by-actions
#[test]
fn stopping_an_already_stopped_deployment_does_not_mint_instances_to_wait_on() {
    // rt.stop addresses the instances a deployment has, so refusing to
    // reuse torn-down ones leaves it minting fresh identities and waiting
    // for them to terminate — which nothing will ever do, since the
    // reconciler desires none of them.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.stop(app.deployment("web"));
        });
    "#,
    );

    let db = crate::runtime::db::DbHandle::open_in_memory().expect("in-memory db");
    let app_name = app.def.load().name.clone();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = Arc::new(
        crate::runtime::registry::DbInstanceRegistry::new(db.clone()),
    );

    // The deployment's one replica has been torn down already.
    let existing = reg
        .ensure_scaled_group(
            &app_name,
            ResourceKind::Deployment,
            Some("web"),
            1,
            crate::runtime::registry::InstanceReuse::Any,
        )
        .expect("group resolves")
        .keep[0]
        .clone();
    db.call({
        let existing = existing.clone();
        move |db| {
            for kind in ["container_running", "container_removed"] {
                crate::runtime::history::insert_observation(
                    db,
                    &existing,
                    kind,
                    &serde_json::json!({}),
                )
                .unwrap();
            }
        }
    });

    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Unscheduled);
    let log = InMemoryActionLog::new();

    let r = run_pass_with_db(
        &engine,
        &ast,
        &app,
        &mut scope,
        OperationId::new(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
        Some(db),
    );
    assert!(matches!(r, OperationResult::Completed), "{r:?}");

    // The oracle keys on kind and name, so it cannot tell a minted
    // instance from the real one; the identity in the log can.
    let entries = log.load().expect("log loads");
    let stopped = entries
        .iter()
        .find(|e| matches!(e.call_kind, crate::runtime::barrier::CallKind::Stop))
        .expect("the stop is recorded");
    assert_eq!(
        stopped.resources.len(),
        1,
        "one replica, one instance to stop: {:?}",
        stopped.resources
    );
    assert_eq!(
        stopped.resources[0].id, existing.id,
        "the stop addressed an instance it had just minted rather than the one the deployment has"
    );
}

// spec: SUSP#deadlines
// spec: SUSP#barrier-anchoring
#[test]
fn the_second_barrier_on_one_started_still_times_out() {
    // Two barriers on one `Started` share one record. Abstaining whenever
    // the entry held anything left the second barrier with no start, so its
    // deadline could never expire; handing it the record on every pass
    // restarted the wait, which could never expire either. The record's
    // state is what tells the two apart: a satisfied record is finished
    // with, a waiting one is still timing something.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            let started = rt.start(app.deployment("web"));
            started.running();
            started.ready(1);
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Running);
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // The deployment reaches Running and stops there, so the first pass
    // records the Ready wait behind the Running barrier it satisfies.
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op.clone(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 1: {r:?}");

    // Past the one-second deadline that wait was recorded with.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op,
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    match r {
        OperationResult::Failed(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("deadline"),
                "expected the ready() deadline to expire, got: {msg}"
            );
        }
        other => panic!("the second barrier's wait never timed out: {other:?}"),
    }
}

// spec: SUSP#barrier-anchoring
#[test]
fn a_narrowed_barrier_does_not_satisfy_the_broader_one_beside_it() {
    // Narrowed groups carry the anchor of the call they came from, so a
    // barrier over part of a group wrote a satisfied record that a barrier
    // over the whole of it then read as its own.
    //
    // The two barriers need a suspension between them for it to show: in
    // one pass the broader barrier overwrites the narrowed one's record
    // before the pass commits, and the satisfied record never survives to
    // be read. Across a suspension it does.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        let api = app.deployment("api").image("docker.io/library/nginx:latest");
        let gate = app.deployment("gate").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            let started = rt.start(app);
            started.only(app.deployment("web")).ready();
            rt.start(app.deployment("gate")).ready();
            started.ready();
        });
    "#,
    );

    // `web` and `gate` come up; `api` never does, so the barrier over the
    // whole group must keep waiting however often it is replayed.
    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Ready);
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op.clone(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 1: {r:?}");

    oracle.set(dep("gate"), LifecycleState::Ready);
    for pass in 2..=3 {
        let r = run_pass(
            &engine,
            &ast,
            &app,
            &mut scope,
            op.clone(),
            &log,
            Arc::clone(&oracle),
            Arc::clone(&reg),
        );
        assert!(
            matches!(r, OperationResult::Suspended(_)),
            "pass {pass}: the barrier over the whole group passed on the narrowed one's record, \
             with api never up: {r:?}"
        );
    }
}

// spec: SUSP#positional-replay
#[test]
fn a_start_replayed_against_a_stop_entry_diverges() {
    // The rt.* surfaces advanced past the committed entry at their position
    // without checking what kind of call it recorded, so a script edit that
    // swapped the call there replayed silently against an entry describing
    // something else — taking over whatever barrier record it held.
    let stop_then_barrier = r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.stop(app.deployment("web"));
        });
    "#;
    let (engine, mut scope, app, ast) = setup_with_script(stop_then_barrier);

    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Running);
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1 records a Stop at position 0 and suspends on it.
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op.clone(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 1: {r:?}");

    // The script is edited so position 0 is now an rt.start.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.start(app.deployment("web")).ready();
        });
    "#,
    );
    oracle.set(dep("web"), LifecycleState::Ready);
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op,
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    match r {
        OperationResult::Failed(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("replay diverged"),
                "expected a divergence naming the recorded kind, got: {msg}"
            );
        }
        other => panic!("a start replayed against a stop's entry without complaint: {other:?}"),
    }
}

// spec: SUSP#deadlines
#[test]
fn a_barrier_on_a_narrowed_group_still_times_out() {
    // Keeping narrowed groups off the shared record stopped them aliasing
    // the barrier over the whole group, but it also left them recording no
    // wait — so nothing measured their deadline and they suspended for
    // good. Before narrowing did anything, the deadline applied to the
    // whole group, so an operation that used to fail now hangs.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        let optional = app.deployment("optional").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.start(app).except(app.deployment("optional")).ready(1);
        });
    "#,
    );

    // Neither comes up. The narrowed group is `web` alone, and its barrier
    // must give up rather than wait on it for ever.
    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op.clone(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 1: {r:?}");

    std::thread::sleep(std::time::Duration::from_millis(1100));

    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op,
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    match r {
        OperationResult::Failed(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("deadline"),
                "expected the narrowed barrier's deadline to expire, got: {msg}"
            );
        }
        other => panic!("a barrier on a narrowed group waited past its deadline: {other:?}"),
    }
}

// spec: SUSP#positional-replay
#[test]
fn a_stop_replayed_against_a_start_entry_diverges() {
    // rt.stop took the committed entry at its position whatever kind it
    // recorded, and its fast-skip accepted any satisfied record on it. So a
    // satisfied Ready record left by an rt.start made the stop return Ok
    // having stopped nothing.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        let gate = app.deployment("gate").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.start(app.deployment("web")).ready();
            rt.start(app.deployment("gate")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Ready);
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1 leaves a Start at position 0 with a satisfied Ready record,
    // and suspends on `gate`.
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op.clone(),
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "pass 1: {r:?}");
    let entries = log.load().expect("log loads");
    assert!(
        entries[0]
            .barrier(0)
            .as_ref()
            .is_some_and(|b| b.satisfied && b.required_state == LifecycleState::Ready),
        "pass 1 should leave a satisfied Ready record at position 0: {entries:?}"
    );

    // The script is edited so position 0 is now an rt.stop.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            rt.stop(app.deployment("web"));
        });
    "#,
    );
    let r = run_pass(
        &engine,
        &ast,
        &app,
        &mut scope,
        op,
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    match r {
        OperationResult::Failed(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("replay diverged"),
                "expected a divergence naming the recorded kind, got: {msg}"
            );
        }
        other => panic!("a stop replayed against a start's satisfied record: {other:?}"),
    }
}

// spec: SUSP#barrier-anchoring
#[test]
fn each_barrier_on_one_call_keeps_its_own_record() {
    // Three barriers on one call: a narrowed one, and two over the whole
    // group. They held one record between them, so whichever wrote last
    // displaced the rest — which is how a barrier came to take another's
    // passing for its own, and how a wait lost the start it was timed from.
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        let web = app.deployment("web").image("docker.io/library/nginx:latest");
        let api = app.deployment("api").image("docker.io/library/nginx:latest");
        app.on_start(|rt, _param| {
            let started = rt.start(app);
            started.only(app.deployment("web")).ready();
            started.running();
            started.ready();
        });
    "#,
    );

    // `web` is up and `api` is running but never ready, so the narrowed
    // barrier and the Running one pass while the last keeps waiting.
    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Ready);
    oracle.set(dep("api"), LifecycleState::Running);
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    for pass in 1..=2 {
        let r = run_pass(
            &engine,
            &ast,
            &app,
            &mut scope,
            op.clone(),
            &log,
            Arc::clone(&oracle),
            Arc::clone(&reg),
        );
        assert!(
            matches!(r, OperationResult::Suspended(_)),
            "pass {pass}: the Ready barrier over the whole group passed on another's record: {r:?}"
        );
    }

    let entries = log.load().expect("log loads");
    assert_eq!(entries.len(), 1, "one call: {entries:?}");
    let mut barriers = entries[0].barriers.clone();
    barriers.sort_by_key(|b| b.seq);
    let seen: Vec<(usize, LifecycleState, bool)> = barriers
        .iter()
        .map(|b| (b.seq, b.required_state, b.satisfied))
        .collect();
    assert_eq!(
        seen,
        vec![
            (0, LifecycleState::Ready, true),
            (1, LifecycleState::Running, true),
            (2, LifecycleState::Ready, false),
        ],
        "each barrier keeps its own record, under its own position on the call"
    );
    assert!(
        barriers[2].started_at_secs.is_some(),
        "the barrier still waiting records the start it is timed from"
    );
}

/// The deadline `rt.stop(<stopped>)` records when called with no deadline.
fn default_stop_deadline_of(declarations: &str, stopped: &str) -> Option<u64> {
    let (engine, mut scope, app, ast) = setup_with_script(&format!(
        r#"
        {declarations}
        app.on_start(|rt, _param| {{ rt.start({stopped}); rt.stop({stopped}); }});
    "#
    ));
    let log = InMemoryActionLog::new();
    let r = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: OperationId::new(),
            app: &app,
            action_name: "start",
            log: &log,
            world: Arc::new(TestWorldOracle::new()),
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
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)), "{r:?}");
    let entries = log.load().unwrap();
    let stop = entries
        .iter()
        .find(|e| e.call_kind == crate::runtime::barrier::CallKind::Stop)
        .expect("the stop is recorded");
    stop.barriers[0].deadline_secs
}

// spec: RT#barriers
#[test]
fn rt_stops_default_deadline_follows_the_longest_stop_timeout() {
    let declarations = r#"
        let slow = app.deployment("slow").image("docker.io/library/postgres:17").stop_timeout(120);
        let quick = app.deployment("quick").image("docker.io/library/nginx:1").stop_timeout(5);
        let unset = app.deployment("unset").image("docker.io/library/nginx:1");
        let svc = app.service("public");
    "#;
    assert_eq!(
        default_stop_deadline_of(declarations, "col([slow, quick])"),
        Some(150)
    );
    assert_eq!(default_stop_deadline_of(declarations, "quick"), Some(35));
    // A container that sets no stop timeout has the supervisor's.
    assert_eq!(default_stop_deadline_of(declarations, "unset"), Some(120));
}

/// Run the `greeting` change handler of `script` at generation 2, with the
/// app's history in `db` and `previous` as the generation before.
fn run_change_handler(
    previous: Option<&str>,
    script: &str,
    corrupt_history: bool,
) -> OperationResult {
    let (engine, mut scope, app, ast) = setup_with_script(script);
    let name = seedling_protocol::names::AppName::new("test-app").unwrap();
    app.def.rcu(|d| {
        let mut d = (**d).clone();
        d.name = name.clone();
        d
    });
    let db =
        crate::runtime::db::DbHandle::from_db(crate::runtime::db::Db::open_in_memory().unwrap());
    if let Some(previous) = previous {
        let (n, previous) = (name.clone(), previous.to_owned());
        db.call(move |db| {
            crate::runtime::generations::register_script(db, &n, &previous).unwrap();
            crate::runtime::generations::update_script(db, &n, "").unwrap();
        });
    }
    if corrupt_history {
        db.call(|db| db.conn.execute_batch("DROP TABLE generations;").unwrap());
    }
    let oracle = Arc::new(TestWorldOracle::new());
    run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: OperationId::new(),
            app: &app,
            action_name: "greeting",
            log: &InMemoryActionLog::new(),
            world: oracle,
            registry: registry(),
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: Some(db),
            source_generation: 1,
            target_generation: 2,
            script_limits: Some(crate::ScriptLimits::default()),
            cipher: Some(Arc::new(crate::runtime::secrets::Cipher::for_tests())),
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    )
}

const CHANGE_HANDLER_SCRIPT: &str = r#"
    app.deployment("web").image("docker.io/library/nginx:latest");
    app.param("greeting").on_change(|rt, old| {
        old.deployment("legacy");
    });
"#;

// spec: PARAM#change-handlers
#[test]
fn old_resolves_resources_from_the_previous_generation() {
    // `legacy` exists only in the previous definition. Resource lookups on
    // `old` were answered from the current generation, so this threw.
    let result = run_change_handler(
        Some(r#"app.deployment("legacy").image("docker.io/library/nginx:1.0");"#),
        CHANGE_HANDLER_SCRIPT,
        false,
    );
    assert!(
        matches!(result, OperationResult::Completed),
        "old did not reflect the previous generation: {result:?}"
    );
}

// spec: PARAM#change-handlers
#[test]
fn a_change_handler_is_not_run_when_the_previous_generation_cannot_be_rebuilt() {
    // The handler used to run with an empty `old`.
    let result = run_change_handler(
        Some(r#"app.deployment("legacy").image("docker.io/library/nginx:1.0");"#),
        CHANGE_HANDLER_SCRIPT,
        true,
    );
    match result {
        OperationResult::Failed(e) => {
            assert!(e.to_string().contains("cannot be rebuilt"), "{e}");
        }
        other => panic!("the handler ran without a faithful old: {other:?}"),
    }
}
