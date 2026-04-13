use std::collections::BTreeMap;
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
    db: Arc<Mutex<Db>>,
    operation_id: super::OperationId,
    app: String,
    action_name: String,
}

impl DbActionLog {
    pub fn new(
        db: Arc<Mutex<Db>>,
        operation_id: super::OperationId,
        app: impl Into<String>,
        action_name: impl Into<String>,
    ) -> Self {
        Self {
            db,
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
#[derive(Debug)]
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
    /// Requirements for the install action; `None` for all other actions.
    pub install_requirements: Option<BTreeMap<String, String>>,
    /// `true` when this operation executes a shell action closure.
    /// Affects the call script used to invoke the closure.
    pub is_shell: bool,
    /// DB handle for persisting dynamic resources created during the operation.
    pub db: Option<Arc<parking_lot::Mutex<Db>>>,
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
        install_requirements,
        is_shell: _,
        db,
    } = op;

    // Save the operation ID string before it is moved into the replay context.
    let op_id_str = operation_id.0.clone();

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
    let rt = RuntimeInstance::with_context(Arc::clone(&ctx), app_name.clone(), registry, db);

    // Re-run the BSL script with a fresh scope and App to recover the FnPtr
    // for this action. FnPtrs are not stored in AppDef (which must be Send);
    // the thread-local capture buffer collects them during the re-run and is
    // discarded immediately after. The fresh AppDef is compared against the
    // stored one as an idempotency check, then also discarded.
    let (closure, is_install, is_param_change, is_shell) = {
        let (mut fresh_scope, fresh_app) = crate::defs::scope();
        fresh_app.def.lock().name = app_name;
        // i[param.store] — restore persisted param values so is_set()/value()
        // return correct results when the script is re-evaluated for closure recovery.
        *fresh_app.stored.lock() = app.stored.lock().clone();
        begin_closure_capture();
        let run_result = engine.run_ast_with_scope(&mut fresh_scope, script_ast);
        let captured = end_closure_capture(); // always drain, even on error
        if let Err(e) = run_result {
            return OperationResult::Failed(e);
        }
        check_idempotent(&fresh_app.def.lock(), &app.def.lock());

        let (closure, is_install, is_shell) = if action_name == "install" {
            if let Some(c) = captured.install {
                (c, true, false)
            } else {
                return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                    "install action not defined in script".into(),
                    rhai::Position::NONE,
                )));
            }
        } else if let Some(c) = captured.actions.get(action_name) {
            (c.clone(), false, false)
        } else if let Some(c) = captured.param_changes.get(action_name) {
            (c.clone(), false, false)
        } else if let Some(c) = captured.shells.get(action_name) {
            (c.clone(), false, true)
        } else {
            return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                format!("Action '{}' not found", action_name).into(),
                rhai::Position::NONE,
            )));
        };
        let is_param_change =
            !is_install && fresh_app.def.lock().param_changes.contains(action_name);
        (closure, is_install, is_param_change, is_shell)
        // captured, fresh_scope, and fresh_app are all dropped here.
    };

    let old_app = App::default();

    scope.push("__bsl_rt", rt);
    scope.push("__bsl_closure", closure);
    scope.push("__bsl_old_app", old_app);
    let reqs_map: rhai::Map = if action_name == "install" {
        install_requirements
            .as_ref()
            .map(|reqs| {
                reqs.iter()
                    .map(|(k, v)| (k.as_str().into(), rhai::Dynamic::from(v.clone())))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        rhai::Map::new()
    };
    scope.push("__bsl_reqs", reqs_map);
    if is_shell {
        scope.push("__bsl_attach", super::shell::shell_attach_fn_ptr());
    }

    let call_script = if is_install {
        "__bsl_closure.call(__bsl_rt, __bsl_reqs)"
    } else if is_param_change {
        "__bsl_closure.call(__bsl_rt, __bsl_old_app)"
    } else if is_shell {
        "try { __bsl_closure.call(__bsl_rt, __bsl_attach) } catch { let _r = __bsl_closure.call(__bsl_rt); __bsl_shell_attach_impl(_r) }"
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
    let action_def = Arc::new(Mutex::new(app.def.lock().clone()));
    let result = {
        let _guard = ActionClosureGuard::new(action_def, op_id_str.clone());
        engine.eval_ast_with_scope::<Dynamic>(scope, &call_ast)
    };

    let _ = scope.remove::<Dynamic>("__bsl_rt");
    let _ = scope.remove::<Dynamic>("__bsl_closure");
    let _ = scope.remove::<Dynamic>("__bsl_old_app");
    let _ = scope.remove::<rhai::Map>("__bsl_reqs");
    if is_shell {
        let _ = scope.remove::<rhai::FnPtr>("__bsl_attach");
    }

    // Flush pending entries from the context to the log.
    let pending = ctx.lock().take_pending();
    log.commit(&pending);
    if let Some(ap) = &active_progress {
        let mut progress = OperationProgress::from_log(&log.load());
        progress.dynamic_defs = ctx.lock().dynamic_defs.clone();
        *ap.write() = Some(progress);
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
    if fresh.params != stored.params {
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

#[cfg(test)]
mod tests;
