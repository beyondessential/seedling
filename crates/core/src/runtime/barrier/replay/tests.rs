use std::sync::Arc;

use seedling_protocol::names::{ActionName, AppName};

use super::*;
use crate::defs::resource::ResourceKind;
use crate::runtime::barrier::OperationId;
use crate::runtime::barrier::oracle::TestWorldOracle;
use crate::runtime::db::DbHandle;
use crate::runtime::identity::ResourceInstance;
use crate::runtime::lifecycle::LifecycleState;

fn app_name() -> AppName {
    AppName::new("test-app").unwrap()
}

fn action_name(s: &str) -> ActionName {
    ActionName::new(s).unwrap()
}

fn dep(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton(app_name(), ResourceKind::Deployment, name)
}

// r[verify barrier.suspension]
// r[verify barrier.resume]
#[test]
fn db_action_log_barrier_suspends_then_resumes() {
    let (engine, mut scope, app, ast) = {
        let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
        app.def.rcu(|d| {
            let mut d = (**d).clone();
            d.name = app_name();
            d
        });
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
            DbHandle::open_in_memory().expect("in-memory DB"),
            op.clone(),
            app_name(),
            action_name("start"),
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
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
            container_signaler: None,
            volume_writer: None,
            executor: None,
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

    let entries = log.load().unwrap();
    assert_eq!(entries.len(), 1, "no duplicate entries after second pass");
}

// r[barrier.replay]
// r[verify barrier.replay.determinism]
#[test]
fn db_action_log_sequential_barriers() {
    let (engine, mut scope, app, ast) = {
        let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default());
        app.def.rcu(|d| {
            let mut d = (**d).clone();
            d.name = app_name();
            d
        });
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
        DbHandle::open_in_memory().expect("in-memory DB"),
        op.clone(),
        app_name(),
        action_name("start"),
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

    let entries = log.load().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].call_index, 0);
    assert_eq!(entries[1].call_index, 1);
}

// r[verify barrier.replay.positional]
// Positional matching, not value matching. Both halves of the old
// value-scan were wrong: a second identical call matched the first entry
// and was swallowed, and a call whose arguments resolved differently
// between passes matched nothing and re-ran.
mod positional {
    use crate::runtime::barrier::{ActionLogEntry, CallKind, ReplayContext};

    use super::*;

    fn instance(name: &str) -> ResourceInstance {
        ResourceInstance {
            id: crate::runtime::identity::InstanceId::generate(),
            app: app_name(),
            kind: ResourceKind::Deployment,
            name: Some(name.to_owned()),
            variant: crate::runtime::identity::InstanceVariant::Singleton,
            display_name: format!("test-app-{name}"),
        }
    }

    fn entry(index: usize, kind: CallKind, resources: Vec<ResourceInstance>) -> ActionLogEntry {
        ActionLogEntry {
            call_index: index,
            call_kind: kind,
            resources,
            barrier: None,
            extra: Some("SIGHUP".to_owned()),
        }
    }

    fn ctx_with(committed: Vec<ActionLogEntry>) -> ReplayContext {
        ReplayContext::new(
            OperationId("op-positional".into()),
            committed,
            Arc::new(TestWorldOracle::default()),
            Arc::new(crate::runtime::barrier::CancelToken::default()),
        )
    }

    // r[verify barrier.replay.positional]
    #[test]
    fn two_identical_calls_consume_two_entries() {
        let db = instance("db");
        let mut ctx = ctx_with(vec![
            entry(0, CallKind::Signal, vec![db.clone()]),
            entry(1, CallKind::Signal, vec![db.clone()]),
        ]);

        // Both are replays of their own position, not one matching twice.
        assert!(ctx.replay_step(CallKind::Signal, None).unwrap().is_some());
        assert!(ctx.replay_step(CallKind::Signal, None).unwrap().is_some());
        // A third identical call at a position the log does not cover is new
        // and must actually run — which is what the value scan swallowed.
        assert!(ctx.replay_step(CallKind::Signal, None).unwrap().is_none());
    }

    // r[verify barrier.replay.positional]
    // The instance set can differ between passes — a replica added or
    // retired — and that does not make it a different call. Matching by value
    // found nothing here and re-delivered a signal already delivered.
    #[test]
    fn a_changed_instance_set_is_still_the_same_call() {
        let before = vec![instance("db")];
        let after = vec![instance("db"), instance("db")];
        let mut ctx = ctx_with(vec![entry(0, CallKind::Signal, before)]);

        let replayed = ctx.replay_step(CallKind::Signal, None).unwrap();
        assert!(
            replayed.is_some(),
            "the call at this position was already made, whatever it resolved to"
        );
        assert_ne!(replayed.unwrap().resources, after);
    }

    // r[verify barrier.replay.positional]
    #[test]
    fn a_diverged_log_fails_rather_than_guessing() {
        let mut ctx = ctx_with(vec![entry(0, CallKind::Exec, vec![instance("db")])]);
        let err = ctx.replay_step(CallKind::Signal, None).unwrap_err();
        assert_eq!(err.call_index(), 0);
        assert!(
            matches!(
                err,
                crate::runtime::barrier::ReplayMismatch::Kind {
                    expected: CallKind::Signal,
                    found: CallKind::Exec,
                    ..
                }
            ),
            "{err}"
        );
        // The message is the operator's only account of why an operation
        // refused to resume, so it has to name the position and both kinds.
        let message = err.to_string();
        assert!(message.contains("call 0"), "{message}");
        assert!(message.contains("Exec"), "{message}");
        assert!(message.contains("Signal"), "{message}");
    }

    // r[verify barrier.replay.positional]
    // Position alone is not enough for an argument that comes from the script
    // text. A script edited from SIGHUP to SIGTERM at the same position would
    // otherwise be treated as already replayed, and the new signal never
    // delivered. The resolved instance set is deliberately *not* checked this
    // way — it legitimately varies between passes.
    #[test]
    fn a_changed_signal_name_is_a_divergence() {
        let mut ctx = ctx_with(vec![entry(0, CallKind::Signal, vec![instance("db")])]);
        let err = ctx
            .replay_step(CallKind::Signal, Some("SIGTERM"))
            .unwrap_err();
        assert!(
            matches!(
                err,
                crate::runtime::barrier::ReplayMismatch::Extra { ref expected, .. }
                    if expected == "SIGTERM"
            ),
            "{err}"
        );
        let message = err.to_string();
        assert!(message.contains("SIGTERM"), "{message}");
        assert!(message.contains("SIGHUP"), "{message}");

        // The recorded name replays cleanly.
        let mut ctx = ctx_with(vec![entry(0, CallKind::Signal, vec![instance("db")])]);
        assert!(
            ctx.replay_step(CallKind::Signal, Some("SIGHUP"))
                .unwrap()
                .is_some()
        );
    }
}
