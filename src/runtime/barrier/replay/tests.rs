use std::sync::Arc;

use parking_lot::Mutex;

use super::*;
use crate::defs::resource::ResourceKind;
use crate::runtime::barrier::OperationId;
use crate::runtime::barrier::oracle::TestWorldOracle;
use crate::runtime::db::Db;
use crate::runtime::identity::ResourceInstance;
use crate::runtime::lifecycle::LifecycleState;

fn dep(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton("test-app", ResourceKind::Deployment, name)
}

// r[barrier.suspension]
// r[barrier.resume]
#[test]
fn db_action_log_barrier_suspends_then_resumes() {
    let (engine, mut scope, app, ast) = {
        let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
        let ast = crate::tests::run_script(
            &engine,
            &mut scope,
            r#"
            let dep = app.deployment("web").image("docker.io/library/nginx:latest");
            app.on_start(|rt, _param| {
                rt.start(app.deployment("web")).ready();
            });
            "#,
        )
        .expect("script should parse");
        (engine, scope, app, ast)
    };

    let oracle = Arc::new(TestWorldOracle::new());
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::registry::InstanceRegistry> =
        Arc::new(crate::runtime::registry::EphemeralInstanceRegistry::new());

    let make_log = || {
        DbActionLog::new(
            Arc::new(Mutex::new(Db::open_in_memory().expect("in-memory DB"))),
            op.clone(),
            "test-app",
            "start",
        )
    };

    // Pass 1: web is Pending -> suspend
    let log = make_log();
    let result = run_operation(
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
            operation_volume_bindings: std::collections::HashMap::new(),
        },
        &mut scope,
    );
    assert!(matches!(result, OperationResult::Suspended(_)));

    let entries = log.load().unwrap();
    assert_eq!(entries.len(), 1, "one entry after first pass");
    let barrier = entries[0]
        .barrier
        .as_ref()
        .expect("barrier should be recorded");
    assert!(!barrier.satisfied, "barrier not yet satisfied");

    oracle.set(dep("web"), LifecycleState::Ready);

    // Pass 2: same DB log, barrier satisfied -> complete
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
            operation_volume_bindings: std::collections::HashMap::new(),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Completed));

    let entries = log.load().unwrap();
    assert_eq!(entries.len(), 1, "no duplicate entries after second pass");
}

// r[barrier.replay]
#[test]
fn db_action_log_sequential_barriers() {
    let (engine, mut scope, app, ast) = {
        let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
        let ast = crate::tests::run_script(
            &engine,
            &mut scope,
            r#"
            let fe = app.deployment("frontend").image("docker.io/library/nginx:latest");
            let be = app.deployment("backend").image("docker.io/library/api:latest");
            app.on_start(|rt, _param| {
                rt.start(app.deployment("frontend")).scheduled();
                rt.start(app.deployment("backend")).ready();
            });
            "#,
        )
        .expect("script should parse");
        (engine, scope, app, ast)
    };

    let oracle = Arc::new(TestWorldOracle::new());
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::registry::InstanceRegistry> =
        Arc::new(crate::runtime::registry::EphemeralInstanceRegistry::new());
    let log = DbActionLog::new(
        Arc::new(Mutex::new(Db::open_in_memory().expect("in-memory DB"))),
        op.clone(),
        "test-app",
        "start",
    );

    // Pass 1: frontend not Scheduled -> suspend
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
            operation_volume_bindings: std::collections::HashMap::new(),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    oracle.set(dep("frontend"), LifecycleState::Scheduled);

    // Pass 2: frontend ok, backend not Ready -> suspend
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
            operation_volume_bindings: std::collections::HashMap::new(),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    oracle.set(dep("backend"), LifecycleState::Ready);

    // Pass 3: both satisfied -> complete
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
            operation_volume_bindings: std::collections::HashMap::new(),
        },
        &mut scope,
    );
    assert!(matches!(r, OperationResult::Completed));

    let entries = log.load().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].call_index, 0);
    assert_eq!(entries[1].call_index, 1);
}
