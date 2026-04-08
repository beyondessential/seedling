use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use rhai::{AST, Dynamic, Engine, EvalAltResult, Scope};
use tokio::sync::Notify;

use crate::runtime::db::Db;
use crate::runtime::desired::OperationProgress;
use crate::runtime::history;

use crate::defs::app::{App, AppDef, begin_closure_capture, end_closure_capture};
use crate::runtime::barrier::oracle::WorldStateOracle;
use crate::runtime::barrier::runtime::{
    ActionClosureGuard, RuntimeInstance, clear_barrier_hit, extract_barrier_hit,
};
use crate::runtime::barrier::{ActionLogEntry, BarrierCondition, OperationId, ReplayContext};
use crate::runtime::registry::InstanceRegistry;

// ---------------------------------------------------------------------------
// ActionLog trait
// ---------------------------------------------------------------------------

/// Persistent store for the action execution log of a lifecycle operation.
///
/// The in-memory implementation is used during early development and in tests.
/// A SQLite-backed implementation will be added as part of the persistent
/// history work (items 1–6 of the runtime foundation plan).
pub trait ActionLog: Send + Sync {
    /// Load all committed entries for replay.
    fn load(&self) -> Vec<ActionLogEntry>;

    /// Commit new or updated entries. Entries are identified by `call_index`;
    /// committing an entry that already exists updates its barrier satisfaction
    /// status rather than creating a duplicate.
    fn commit(&self, entries: &[ActionLogEntry]);
}

// ---------------------------------------------------------------------------
// InMemoryActionLog
// ---------------------------------------------------------------------------

// r[impl history.action-log]
#[derive(Debug, Default)]
pub struct InMemoryActionLog {
    entries: Mutex<Vec<ActionLogEntry>>,
}

impl InMemoryActionLog {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ActionLog for InMemoryActionLog {
    fn load(&self) -> Vec<ActionLogEntry> {
        self.entries.lock().clone()
    }

    fn commit(&self, new_entries: &[ActionLogEntry]) {
        let mut entries = self.entries.lock();
        // r[impl reconciliation.idempotency]
        for entry in new_entries {
            if let Some(existing) = entries
                .iter_mut()
                .find(|e| e.call_index == entry.call_index)
            {
                // Update satisfaction status if it changed.
                if let (Some(nb), Some(eb)) = (&entry.barrier, &mut existing.barrier)
                    && nb.satisfied
                {
                    eb.satisfied = true;
                    eb.started_at_secs = nb.started_at_secs.or(eb.started_at_secs);
                }
            } else {
                entries.push(entry.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DbActionLog
// ---------------------------------------------------------------------------

// r[impl history.action-log]
pub struct DbActionLog {
    db: Mutex<Db>,
    operation_id: super::OperationId,
    app: String,
    action_name: String,
}

impl DbActionLog {
    pub fn new(
        db: Db,
        operation_id: super::OperationId,
        app: impl Into<String>,
        action_name: impl Into<String>,
    ) -> Self {
        Self {
            db: Mutex::new(db),
            operation_id,
            app: app.into(),
            action_name: action_name.into(),
        }
    }
}

impl ActionLog for DbActionLog {
    fn load(&self) -> Vec<super::ActionLogEntry> {
        let db = self.db.lock();
        history::load_action_log(&db, &self.operation_id).unwrap_or_default()
    }

    fn commit(&self, entries: &[super::ActionLogEntry]) {
        let db = self.db.lock();
        // r[impl reconciliation.idempotency]
        for entry in entries {
            let _ = history::insert_action_log_entry(
                &db,
                &self.operation_id,
                &self.app,
                &self.action_name,
                entry,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// OperationResult
// ---------------------------------------------------------------------------

// r[impl operation.lifecycle]
pub enum OperationResult {
    /// The closure ran to completion.
    Completed,
    /// A barrier was hit; the caller must satisfy the condition and retry.
    Suspended(BarrierCondition),
    /// The closure threw a genuine BSL error.
    Failed(Box<EvalAltResult>),
}

// ---------------------------------------------------------------------------
// run_operation
// ---------------------------------------------------------------------------

/// Execute one pass of a lifecycle operation's action closure.
///
/// Returns:
/// - `Completed` if the closure finished.
/// - `Suspended(condition)` if a barrier was hit; call again after the world
///   state changes.
/// - `Failed(err)` if the closure threw a genuine BSL error.
///
/// All inputs to a single pass of a lifecycle operation.
pub struct OperationContext<'a, W: WorldStateOracle + 'static> {
    pub engine: &'a Engine,
    pub script_ast: &'a AST,
    pub operation_id: OperationId,
    pub app: &'a App,
    pub action_name: &'a str,
    pub log: &'a dyn ActionLog,
    pub world: Arc<W>,
    pub registry: Arc<dyn InstanceRegistry>,
    pub active_progress: Option<Arc<RwLock<Option<OperationProgress>>>>,
    pub tick_notify: Option<Arc<Notify>>,
}

/// The `log` carries committed entries across calls; pass the same `log`
/// instance for all passes of the same operation to enable replay.
// r[impl history.action-log.replay]
// r[impl barrier.replay]
pub fn run_operation<W: WorldStateOracle + 'static>(
    op: OperationContext<'_, W>,
    scope: &mut Scope<'_>,
) -> OperationResult {
    let OperationContext {
        engine,
        script_ast,
        operation_id,
        app,
        action_name,
        log,
        world,
        registry,
        active_progress,
        tick_notify,
    } = op;

    // Build the replay context from committed log entries.
    let committed = log.load();
    let ctx = Arc::new(Mutex::new(ReplayContext::new(
        operation_id,
        committed,
        world as Arc<dyn WorldStateOracle>,
    )));

    // Clear the thread-local barrier-hit flag at the start of each pass.
    clear_barrier_hit();

    let app_name = app.def.lock().name.clone();
    let rt = RuntimeInstance::with_context(Arc::clone(&ctx), app_name.clone(), registry);

    // Re-run the BSL script with a fresh scope and App to recover the FnPtr
    // for this action. FnPtrs are not stored in AppDef (which must be Send);
    // the thread-local capture buffer collects them during the re-run and is
    // discarded immediately after. The fresh AppDef is compared against the
    // stored one as an idempotency check, then also discarded.
    let (closure, is_param_change) = {
        let (mut fresh_scope, fresh_app) = crate::defs::scope();
        fresh_app.def.lock().name = app_name;
        // i[param.store] — restore persisted param values so on_change closures
        // capture the correct values when the script is re-evaluated.
        {
            let stored_params = app.def.lock().params.clone();
            fresh_app.def.lock().params = stored_params;
        }
        begin_closure_capture();
        let run_result = engine.run_ast_with_scope(&mut fresh_scope, script_ast);
        let captured = end_closure_capture(); // always drain, even on error
        if let Err(e) = run_result {
            return OperationResult::Failed(e);
        }
        check_idempotent(&fresh_app.def.lock(), &app.def.lock());

        let closure = if let Some(c) = captured.actions.get(action_name) {
            c.clone()
        } else if let Some(c) = captured.param_changes.get(action_name) {
            c.clone()
        } else {
            return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                format!("Action '{}' not found", action_name).into(),
                rhai::Position::NONE,
            )));
        };
        let is_param_change = fresh_app.def.lock().param_changes.contains(action_name);
        (closure, is_param_change)
        // captured, fresh_scope, and fresh_app are all dropped here.
    };

    let old_app = App::default();

    scope.push("__bsl_rt", rt);
    scope.push("__bsl_closure", closure);
    scope.push("__bsl_old_app", old_app);

    let call_script = if is_param_change {
        "__bsl_closure.call(__bsl_rt, __bsl_old_app)"
    } else {
        "__bsl_closure.call(__bsl_rt)"
    };

    // Evaluate only the closure call — do NOT re-execute the script body.
    // The closure already captured its entire environment (app, resources,
    // params, etc.) at creation time during run_file.  Re-executing the script
    // here would run top-level statements such as param.on_change() inside the
    // ActionClosureGuard, which incorrectly triggers the in-closure guard check.
    let call_ast = engine
        .compile(call_script)
        .expect("static call script must compile");
    let result = {
        let _guard = ActionClosureGuard::new();
        engine.eval_ast_with_scope::<Dynamic>(scope, &call_ast)
    };

    let _ = scope.remove::<Dynamic>("__bsl_rt");
    let _ = scope.remove::<Dynamic>("__bsl_closure");
    let _ = scope.remove::<Dynamic>("__bsl_old_app");

    // Flush pending entries from the context to the log.
    let pending = ctx.lock().take_pending();
    log.commit(&pending);
    if let Some(ap) = &active_progress {
        *ap.write() = Some(OperationProgress::from_log(&log.load()));
    }
    if let Some(notify) = &tick_notify {
        notify.notify_one();
    }

    match result {
        Ok(_) => OperationResult::Completed,
        Err(ref e) => match extract_barrier_hit(e) {
            Some(condition) => OperationResult::Suspended(condition),
            None => OperationResult::Failed(result.unwrap_err()),
        },
    }
}

/// Compare a freshly-evaluated AppDef against the stored one and warn if they
/// differ. A mismatch means the BSL script produces different structure on
/// re-run, which indicates a non-idempotent script and may cause the closure's
/// captured variables to be inconsistent with the stored resource state.
fn check_idempotent(fresh: &AppDef, stored: &AppDef) {
    let mut diffs: Vec<&str> = Vec::new();

    if fresh.actions.keys().ne(stored.actions.keys()) {
        diffs.push("actions");
    }
    if fresh.shells.keys().ne(stored.shells.keys()) {
        diffs.push("shells");
    }
    if fresh.install.is_some() != stored.install.is_some() {
        diffs.push("install handler");
    }
    if fresh.param_changes != stored.param_changes {
        diffs.push("param_changes");
    }
    if fresh.resources.keys().ne(stored.resources.keys()) {
        diffs.push("resources");
    }
    if fresh.params.keys().ne(stored.params.keys()) {
        diffs.push("params");
    }

    if !diffs.is_empty() {
        tracing::warn!(
            fields = diffs.join(", "),
            "BSL script produced a different AppDef on re-run; \
             script may not be idempotent"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
    // Same as `barrier_suspends_then_resumes` in tests/barrier.rs but
    // backed by a real SQLite in-memory DB.
    #[test]
    fn db_action_log_barrier_suspends_then_resumes() {
        let (engine, mut scope, app, ast) = {
            let (engine, mut scope, app) = crate::setup_language();
            let ast = crate::tests::run_script(
                &engine,
                &mut scope,
                r#"
                app.on_start(|rt| {
                    rt.start(app.deployment("web").image("nginx")).ready();
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
                Db::open_in_memory().expect("in-memory DB"),
                op.clone(),
                "test-app",
                "start",
            )
        };

        // Pass 1: web is Pending → suspend
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
            },
            &mut scope,
        );
        assert!(matches!(result, OperationResult::Suspended(_)));

        // Verify the entry was persisted
        let entries = log.load();
        assert_eq!(entries.len(), 1, "one entry after first pass");
        let barrier = entries[0]
            .barrier
            .as_ref()
            .expect("barrier should be recorded");
        assert!(!barrier.satisfied, "barrier not yet satisfied");

        // Satisfy the condition
        oracle.set(dep("web"), LifecycleState::Ready);

        // Pass 2: same DB log, barrier satisfied → complete
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
            },
            &mut scope,
        );
        assert!(matches!(r, OperationResult::Completed));

        // No duplicate entries: the DB entry persists in its pre-satisfied state.
        // Replay correctly identifies satisfied barriers via the world oracle rather
        // than relying on the persisted `satisfied` flag — the flag is an optimisation
        // that `InMemoryActionLog` benefits from but is not required for correctness.
        let entries = log.load();
        assert_eq!(entries.len(), 1, "no duplicate entries after second pass");
    }

    // r[barrier.replay]
    // DB-backed sequential barriers: two barriers, three passes.
    #[test]
    fn db_action_log_sequential_barriers() {
        let (engine, mut scope, app, ast) = {
            let (engine, mut scope, app) = crate::setup_language();
            let ast = crate::tests::run_script(
                &engine,
                &mut scope,
                r#"
                app.on_start(|rt| {
                    rt.start(app.deployment("frontend").image("nginx")).scheduled();
                    rt.start(app.deployment("backend").image("api")).ready();
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
            Db::open_in_memory().expect("in-memory DB"),
            op.clone(),
            "test-app",
            "start",
        );

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
            },
            &mut scope,
        );
        assert!(matches!(r, OperationResult::Suspended(_)));

        oracle.set(dep("frontend"), LifecycleState::Scheduled);

        // Pass 2: frontend ok, backend not Ready → suspend
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
                operation_id: op.clone(),
                app: &app,
                action_name: "start",
                log: &log,
                world: Arc::clone(&oracle),
                registry: Arc::clone(&reg),
                active_progress: None,
                tick_notify: None,
            },
            &mut scope,
        );
        assert!(matches!(r, OperationResult::Completed));

        // Exactly two entries, no duplicates
        let entries = log.load();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].call_index, 0);
        assert_eq!(entries[1].call_index, 1);
    }
}
