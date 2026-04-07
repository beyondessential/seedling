use std::sync::Arc;

use crate::defs::resource::ResourceKind;
use crate::{
    defs,
    runtime::{
        ActionLog, EphemeralInstanceRegistry, LifecycleState, ResourceInstance, TestWorldOracle,
        barrier::OperationId,
        barrier::replay::{InMemoryActionLog, OperationResult, run_operation},
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
    let (engine, mut scope, app) = crate::setup_language();
    let ast = super::run_script(&engine, &mut scope, script).expect("script should parse");
    (engine, scope, app, ast)
}

fn dep(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton("test-app", ResourceKind::Deployment, name)
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
        app.on_start(|rt| {
            rt.start(app.deployment("web").image("nginx")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    oracle.set(dep("web"), LifecycleState::Ready);

    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let result = run_operation(
        &engine,
        &mut scope,
        &ast,
        op,
        &app,
        "start",
        &log,
        oracle,
        registry(),
    );
    assert!(matches!(result, OperationResult::Completed));
}

// r[verify barrier.suspension]
// r[verify barrier.resume]
#[test]
fn barrier_suspends_then_resumes() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        app.on_start(|rt| {
            rt.start(app.deployment("web").image("nginx")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();

    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: web is Pending → suspend
    let r = run_operation(
        &engine,
        &mut scope,
        &ast,
        op.clone(),
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    // Satisfy the condition
    oracle.set(dep("web"), LifecycleState::Ready);

    // Pass 2: barrier satisfied → complete
    let r = run_operation(
        &engine,
        &mut scope,
        &ast,
        op,
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
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
        app.on_start(|rt| {
            rt.start(app.deployment("frontend").image("nginx")).scheduled();
            rt.start(app.deployment("backend").image("api")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: frontend not Scheduled → suspend
    let r = run_operation(
        &engine,
        &mut scope,
        &ast,
        op.clone(),
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    oracle.set(dep("frontend"), LifecycleState::Scheduled);

    // Pass 2: first barrier ok, backend not Ready → suspend
    let r = run_operation(
        &engine,
        &mut scope,
        &ast,
        op.clone(),
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    oracle.set(dep("backend"), LifecycleState::Ready);

    // Pass 3: both satisfied → complete
    let r = run_operation(
        &engine,
        &mut scope,
        &ast,
        op,
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Completed));
}

// r[verify barrier.deadline]
#[test]
fn barrier_deadline_zero_expires_on_second_pass() {
    let (engine, mut scope, app, ast) = setup_with_script(
        r#"
        app.on_start(|rt| {
            rt.start(app.deployment("web").image("nginx")).ready(0);
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: not ready → suspend
    let r = run_operation(
        &engine,
        &mut scope,
        &ast,
        op.clone(),
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    // Pass 2: deadline=0, time has elapsed → Failed
    let r = run_operation(
        &engine,
        &mut scope,
        &ast,
        op,
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
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
        app.on_start(|rt| {
            rt.start(app.deployment("a").image("img"));
            rt.start(app.deployment("b").image("img")).ready();
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let reg: Arc<dyn crate::runtime::InstanceRegistry> = registry();

    // Pass 1: b not ready → suspend
    let r = run_operation(
        &engine,
        &mut scope,
        &ast,
        op.clone(),
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    oracle.set(dep("b"), LifecycleState::Ready);

    // Pass 2: completes
    let r = run_operation(
        &engine,
        &mut scope,
        &ast,
        op,
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Completed));

    // No duplicate call_index entries in the log
    let entries = log.load();
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
        app.on_start(|rt| {
            let dep = app.deployment("old").image("nginx");
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
        &engine,
        &mut scope,
        &ast,
        op.clone(),
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Suspended(_)));

    oracle.set(dep("old"), LifecycleState::Terminated);

    // Pass 2: stop barrier satisfied → complete
    let r = run_operation(
        &engine,
        &mut scope,
        &ast,
        op,
        &app,
        "start",
        &log,
        Arc::clone(&oracle),
        Arc::clone(&reg),
    );
    assert!(matches!(r, OperationResult::Completed));
}

// r[verify barrier.suspension]
#[test]
fn stub_runtime_still_passes_language_tests() {
    // The existing exercise() helper uses RuntimeInstance::stub() (no context),
    // so all existing tests continue to work.
    use super::exercise;
    exercise(
        r#"
        app.on_start(|rt| {
            rt.start(app.deployment("web").image("nginx")).ready();
        });
    "#,
    );
}
