use std::{collections::HashMap, sync::Arc};

use parking_lot::{Mutex, RwLock};
use rhai::{AST, Dynamic, Engine, EvalAltResult, Scope};
use seedling_protocol::names::{ActionName, AppName};
use tokio::sync::Notify;

use crate::defs::app::{App, AppDef, begin_closure_capture, end_closure_capture};
use crate::defs::volume::OperationVolumeBinding;
use crate::runtime::barrier::oracle::WorldStateOracle;
use crate::runtime::barrier::runtime::{
    ActionClosureGuard, RuntimeInstance, clear_barrier_hit, extract_barrier_hit, extract_cancel_hit,
};
use crate::runtime::barrier::{
    ActionLogEntry, BarrierCondition, CancelToken, OperationId, ReplayContext,
};
use crate::runtime::desired::OperationProgress;
use crate::runtime::generations;
use crate::runtime::history;
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
    fn load(&self) -> Result<Vec<ActionLogEntry>, Box<dyn std::error::Error + Send + Sync>>;

    /// Commit new or updated entries. Entries are identified by `call_index`;
    /// committing an entry that already exists updates its barrier satisfaction
    /// status rather than creating a duplicate.
    fn commit(&self, entries: &[ActionLogEntry]);

    /// A handle that commits entries at once, whose failure is reported, for
    /// a call that must have its entry on record before it acts on the host.
    ///
    /// `None` for a log that cannot commit mid-pass, whose calls then record
    /// themselves only when the pass ends.
    // spec: SUSP#side-effecting-calls
    fn early_committer(&self) -> Option<EarlyCommit> {
        None
    }
}

/// Commits entries to the log at once, and says whether they are on record.
pub type EarlyCommit = Arc<dyn Fn(&[ActionLogEntry]) -> Result<(), String> + Send + Sync>;

// ---------------------------------------------------------------------------
// InMemoryActionLog
// ---------------------------------------------------------------------------

// spec: HIST#action-execution-log
#[derive(Debug, Default)]
pub struct InMemoryActionLog {
    entries: Arc<Mutex<Vec<ActionLogEntry>>>,
}

/// Replace the entry at each of `new_entries`' positions, or add it.
fn merge_entries(entries: &mut Vec<ActionLogEntry>, new_entries: &[ActionLogEntry]) {
    for entry in new_entries {
        match entries
            .iter_mut()
            .find(|e| e.call_index == entry.call_index)
        {
            Some(existing) => *existing = entry.clone(),
            None => entries.push(entry.clone()),
        }
    }
}

impl InMemoryActionLog {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ActionLog for InMemoryActionLog {
    fn load(&self) -> Result<Vec<ActionLogEntry>, Box<dyn std::error::Error + Send + Sync>> {
        // In the order of the calls, as the database log returns them: the
        // entries are read back by position.
        let mut entries = self.entries.lock().clone();
        entries.sort_by_key(|e| e.call_index);
        Ok(entries)
    }

    fn early_committer(&self) -> Option<EarlyCommit> {
        let entries = Arc::clone(&self.entries);
        Some(Arc::new(move |new_entries| {
            merge_entries(&mut entries.lock(), new_entries);
            Ok(())
        }))
    }

    fn commit(&self, new_entries: &[ActionLogEntry]) {
        let mut entries = self.entries.lock();
        // spec: RCN#reconciliation-loop
        // spec: HIST#action-calls
        // A re-emitted entry replaces the one at its position outright, as
        // the database log's INSERT OR REPLACE does. This used to merge
        // only a satisfied flag that had turned true, so a correction that
        // recorded a barrier as *waiting* — its start and its deadline —
        // was dropped here while landing in the database. An operation then
        // behaved differently depending on which log backed it, and the
        // paths that correction feeds could not be tested against the
        // in-memory log the tests use.
        //
        // A re-emitted entry is always a whole entry: every caller that
        // claims a position already held clones the committed entry and
        // changes the one field it means to.
        merge_entries(&mut entries, new_entries);
    }
}

// ---------------------------------------------------------------------------
// DbActionLog
// ---------------------------------------------------------------------------

// spec: HIST#action-execution-log
pub struct DbActionLog {
    db: crate::runtime::db::DbHandle,
    operation_id: super::OperationId,
    app: AppName,
    action_name: ActionName,
}

impl DbActionLog {
    pub fn new(
        db: crate::runtime::db::DbHandle,
        operation_id: super::OperationId,
        app: AppName,
        action_name: ActionName,
    ) -> Self {
        Self {
            db,
            operation_id,
            app,
            action_name,
        }
    }
}

impl DbActionLog {
    fn commit_checked(&self, entries: &[super::ActionLogEntry]) -> Result<(), String> {
        let op_id = self.operation_id.clone();
        let app = self.app.clone();
        let action_name = self.action_name.clone();
        let entries = entries.to_vec();
        self.db
            .call(move |db| {
                for entry in &entries {
                    history::insert_action_log_entry(db, &op_id, &app, &action_name, entry)?;
                }
                Ok::<_, rusqlite::Error>(())
            })
            .map_err(|e| e.to_string())
    }
}

impl ActionLog for DbActionLog {
    fn early_committer(&self) -> Option<EarlyCommit> {
        let log = DbActionLog::new(
            self.db.clone(),
            self.operation_id.clone(),
            self.app.clone(),
            self.action_name.clone(),
        );
        Some(Arc::new(move |entries| log.commit_checked(entries)))
    }

    fn load(&self) -> Result<Vec<super::ActionLogEntry>, Box<dyn std::error::Error + Send + Sync>> {
        let op_id = self.operation_id.clone();
        Ok(self
            .db
            .call(move |db| history::load_action_log(db, &op_id))?)
    }

    fn commit(&self, entries: &[super::ActionLogEntry]) {
        let op_id = self.operation_id.clone();
        let app = self.app.clone();
        let action_name = self.action_name.clone();
        // spec: RCN#reconciliation-loop
        let entries = entries.to_vec();
        self.db.call(move |db| {
            for entry in &entries {
                if let Err(e) =
                    history::insert_action_log_entry(db, &op_id, &app, &action_name, entry)
                {
                    // UNIQUE constraint violations are expected during replay —
                    // the same entry is committed again idempotently.
                    if matches!(
                        &e,
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error {
                                code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                                ..
                            },
                            _,
                        )
                    ) {
                        continue;
                    }
                    tracing::warn!(
                        app = %app,
                        action = %action_name,
                        call_index = entry.call_index,
                        "failed to commit action log entry: {e}",
                    );
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// OperationResult
// ---------------------------------------------------------------------------

// spec: OPS#operation
#[derive(Debug)]
pub enum OperationResult {
    /// The closure ran to completion.
    Completed,
    /// A barrier was hit; the caller must satisfy the condition and retry.
    Suspended(BarrierCondition),
    /// The closure threw a genuine BSL error.
    Failed(Box<EvalAltResult>),
    /// The operation was cancelled mid-run via the cancel token. Cleanup is
    /// run as for `Failed`, but the terminal state is distinct for audit
    /// purposes.
    // spec: OPS#cancellation
    Cancelled,
}

// ---------------------------------------------------------------------------
// run_operation
// ---------------------------------------------------------------------------

pub(super) fn json_value_to_rhai_dynamic(v: &serde_json::Value) -> rhai::Dynamic {
    match v {
        serde_json::Value::String(s) => Dynamic::from(s.clone()),
        serde_json::Value::Bool(b) => Dynamic::from(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Dynamic::from(i)
            } else if let Some(f) = n.as_f64() {
                Dynamic::from(f)
            } else {
                Dynamic::UNIT
            }
        }
        serde_json::Value::Null => Dynamic::UNIT,
        serde_json::Value::Object(map) => {
            let rhai_map: rhai::Map = map
                .iter()
                .map(|(k, v)| (k.as_str().into(), json_value_to_rhai_dynamic(v)))
                .collect();
            Dynamic::from(rhai_map)
        }
        serde_json::Value::Array(arr) => {
            let rhai_arr: rhai::Array = arr.iter().map(json_value_to_rhai_dynamic).collect();
            Dynamic::from(rhai_arr)
        }
    }
}

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
    /// Action params passed by the invoker. For install actions, contains the
    /// validated requirements. Empty map for actions with no params.
    // spec: OPS#persistence
    pub params: serde_json::Map<String, serde_json::Value>,
    /// `true` when this operation executes a shell action closure.
    /// Affects the call script used to invoke the closure.
    pub is_shell: bool,
    /// DB handle for persisting dynamic resources created during the operation.
    pub db: Option<crate::runtime::db::DbHandle>,
    /// Generation that was current immediately before the change that
    /// triggered this operation. Used to materialise `old` for `on_change`
    /// handlers.
    // spec: OPS#initiating-events
    pub source_generation: u64,
    /// Generation produced by the change that triggered this operation, or
    /// the current generation at dispatch for operator-invoked actions.
    // spec: OPS#initiating-events
    pub target_generation: u64,
    /// Script limits to use when reconstructing the prior `App` for
    /// `on_change` handlers. Required only when source_generation > 0 and
    /// the action is a parameter-change handler.
    pub script_limits: Option<crate::ScriptLimits>,
    /// Cipher for decrypting secret param history when reconstructing the
    /// prior `App` for `on_change` handlers.
    pub cipher: Option<std::sync::Arc<crate::runtime::secrets::Cipher>>,
    /// Operation-scoped external volume bindings injected by the runtime.
    /// Empty unless the operation hands a volume to the closure, as a
    /// `kind: "volume"` param does.
    // spec: VOL#external-volumes
    pub operation_volume_bindings: HashMap<String, OperationVolumeBinding>,
    /// Cooperative cancellation signal. Barriers consult it at entry so an
    /// in-flight cancel unwinds quickly instead of waiting for the next
    /// deadline.
    // spec: OPS#cancellation
    pub cancel_token: Arc<CancelToken>,
    /// Hook for `rt.signal()`. `None` in language-only test contexts where
    /// no real container runtime exists.
    // spec: RT#signal-delivery
    pub container_signaler: Option<std::sync::Arc<dyn crate::runtime::barrier::ContainerSignaler>>,
    /// Hook for `rt.write()`. `None` in language-only test contexts where no
    /// real filesystem is involved.
    // spec: RT#volume-writes
    pub volume_writer: Option<std::sync::Arc<dyn crate::runtime::barrier::VolumeWriter>>,
    /// Hook for `rt.exec()`. `None` in language-only test contexts where no
    /// real container runtime is present.
    // spec: RT#command-execution
    pub executor: Option<std::sync::Arc<dyn crate::runtime::barrier::Executor>>,
}

/// The `log` carries committed entries across calls; pass the same `log`
/// instance for all passes of the same operation to enable replay.
// spec: HIST#action-calls
// spec: SUSP#replay
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
        params,
        is_shell: requested_shell,
        db,
        source_generation,
        target_generation: _,
        script_limits,
        cipher,
        operation_volume_bindings,
        cancel_token,
        container_signaler,
        volume_writer,
        executor,
    } = op;

    // Save the operation ID string before it is moved into the replay context.
    let op_id_str = operation_id.0.clone();

    // Build the replay context from committed log entries.
    let committed = match log.load() {
        Ok(entries) => entries,
        Err(e) => {
            return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                format!("failed to load action log: {e}").into(),
                rhai::Position::NONE,
            )));
        }
    };
    // Emit a per-pass marker so an operator scrolling `apps logs` can
    // see where each replay begins. Suspended operations re-run from
    // call_index 0 each pass; without this marker the duplicated
    // breadcrumbs after a barrier wake up look like the script ran
    // twice.
    // spec: ACTU#breadcrumbs
    if !committed.is_empty() {
        crate::system::breadcrumb::Breadcrumb {
            app: Some(&app.def.load().name),
            kind: crate::system::breadcrumb::BreadcrumbKind::Replay {
                operation_id: &op_id_str,
                committed_len: committed.len(),
            },
            script_pos: None,
        }
        .emit();
    }
    let ctx = Arc::new(Mutex::new(ReplayContext::new(
        operation_id,
        committed,
        world as Arc<dyn WorldStateOracle>,
        Arc::clone(&cancel_token),
    )));
    // spec: RT#signal-delivery
    ctx.lock().container_signaler = container_signaler;
    // spec: RT#volume-writes
    ctx.lock().volume_writer = volume_writer;
    // spec: RT#command-execution
    ctx.lock().executor = executor;
    // spec: SEC#storage
    ctx.lock().cipher = cipher.clone();
    // spec: SUSP#side-effecting-calls
    ctx.lock().early_commit = log.early_committer();

    // Clear the thread-local barrier-hit flag at the start of each pass.
    clear_barrier_hit();

    let app_name = app.def.load().name.clone();
    let rt =
        RuntimeInstance::with_context(Arc::clone(&ctx), app_name.clone(), registry, db.clone());
    // spec: ACN#action-invocation
    // Cloned before pushing into the Rhai scope so the Action.call thread-
    // local stash can hold its own handle. The clone is cheap — every
    // RuntimeInstance field is Arc'd.
    let rt_clone_for_active = rt.clone();

    // Re-run the BSL script with a fresh scope and App to recover the FnPtr
    // for this action. FnPtrs are not stored in AppDef (which must be Send);
    // the thread-local capture buffer collects them during the re-run and is
    // discarded immediately after. The fresh AppDef is compared against the
    // stored one as an idempotency check, then also discarded.
    let (closure, is_param_change, is_shell, captured_actions) = {
        let (mut fresh_scope, fresh_app) = crate::defs::scope(Arc::clone(&app.bundle));
        fresh_app.def.rcu(|d| {
            let mut d = (**d).clone();
            d.name = app_name.clone();
            d
        });
        // spec: PRM#parameter-storage — restore persisted param values so is_set()/value()
        // return correct results when the script is re-evaluated for closure recovery.
        *fresh_app.stored.lock() = app.stored.lock().clone();
        begin_closure_capture();
        let run_result = engine.run_ast_with_scope(&mut fresh_scope, script_ast);
        let captured = end_closure_capture(); // always drain, even on error
        if let Err(e) = run_result {
            return OperationResult::Failed(e);
        }
        check_idempotent(&fresh_app.def.load(), &app.def.load());

        let captured_actions = captured.actions.clone();
        let (closure, is_install, is_shell) = if action_name == "install" {
            if let Some(c) = captured.install {
                (Some(c), true, false)
            } else {
                return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                    "install action not defined in script".into(),
                    rhai::Position::NONE,
                )));
            }
        } else if requested_shell {
            // spec: ACN#shells
            // A shell is looked up among shells, and an action among
            // actions: a shell and an action may share a name.
            match captured.shells.get(action_name) {
                Some(c) => (Some(c.clone()), false, true),
                None => {
                    return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                        format!("Shell '{}' not found", action_name).into(),
                        rhai::Position::NONE,
                    )));
                }
            }
        } else if let Some(c) = captured.actions.get(action_name) {
            (Some(c.clone()), false, false)
        } else if let Some(c) = captured.param_changes.get(action_name) {
            (Some(c.clone()), false, false)
        } else if action_name == "start" {
            // spec: ACN#start-action
            // An app that declares no start action is brought up as though
            // its closure called `rt.start(app)`.
            (None, false, false)
        } else {
            return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                format!("Action '{}' not found", action_name).into(),
                rhai::Position::NONE,
            )));
        };
        let is_param_change =
            !is_install && fresh_app.def.load().param_changes.contains(action_name);
        (closure, is_param_change, is_shell, captured_actions)
        // captured (minus the cloned actions map kept in captured_actions),
        // fresh_scope, and fresh_app are all dropped here.
    };

    // spec: PARAM#change-handlers
    // For param-change handlers, materialise `old` from the previous
    // generation (= source_generation). For other operations, leave it
    // empty: the spec only defines `old` for on_change handlers, but the
    // closure-call script still references the variable. A handler is never
    // invoked with an `old` that does not reflect the previous generation.
    let old_app = if is_param_change && source_generation > 0 {
        match (&db, &script_limits, &cipher) {
            (Some(db_handle), Some(limits), Some(cipher)) => {
                let app_name_owned = app.def.load().name.clone();
                let limits = limits.clone();
                let cipher = std::sync::Arc::clone(cipher);
                let rebuilt = db_handle.call(move |db| {
                    generations::reconstruct_app_def(
                        db,
                        &app_name_owned,
                        source_generation,
                        &limits,
                        &cipher,
                    )
                });
                match rebuilt {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::error!(
                            app = %app.def.load().name,
                            source_generation,
                            "cannot rebuild the previous generation for on_change: {e}"
                        );
                        return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                            format!(
                                "the previous generation ({source_generation}) cannot be rebuilt, \
                                 so the change handler was not run: {e}"
                            )
                            .into(),
                            rhai::Position::NONE,
                        )));
                    }
                }
            }
            _ => {
                return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                    "the previous generation cannot be rebuilt without the database, script \
                     limits and cipher, so the change handler was not run"
                        .into(),
                    rhai::Position::NONE,
                )));
            }
        }
    } else {
        App::default()
    };

    scope.push("__bsl_rt", rt);
    let default_start = closure.is_none();
    // spec: PARAM#change-handlers
    // A change handler is called with as many of its arguments as it takes.
    let change_handler_arity = closure
        .as_ref()
        .filter(|_| is_param_change)
        .and_then(|c| closure_arity(script_ast, c));
    if let Some(closure) = closure {
        scope.push("__bsl_closure", closure);
    } else {
        scope.push("__bsl_app", app.clone());
    }
    scope.push("__bsl_old_app", old_app);
    let param_map: rhai::Map = params
        .iter()
        .map(|(k, v)| (k.as_str().into(), json_value_to_rhai_dynamic(v)))
        .collect();
    scope.push("__bsl_param", param_map);
    if is_shell {
        scope.push("__bsl_shell", super::shell::ShellControl::new());
    }

    let call_script = if default_start {
        // spec: ACN#start-action
        "__bsl_rt.start(__bsl_app)"
    } else if is_param_change {
        match change_handler_arity {
            Some(0) => "__bsl_closure.call()",
            Some(1) => "__bsl_closure.call(__bsl_rt)",
            // Two, or more: a closure declaring more than two parameters
            // fails when called, as it must.
            _ => "__bsl_closure.call(__bsl_rt, __bsl_old_app)",
        }
    } else if is_shell {
        "__bsl_closure.call(__bsl_rt, __bsl_shell, __bsl_param)"
    } else {
        "__bsl_closure.call(__bsl_rt, __bsl_param)"
    };

    // Evaluate only the closure call — do NOT re-execute the script body.
    // The closure already captured its entire environment (app, resources,
    // params, etc.) at creation time during run_file.  Re-executing the script
    // here would run top-level statements such as param.on_change() inside the
    // ActionClosureGuard, which incorrectly triggers the in-closure guard check.
    let call_ast = engine
        .compile(call_script)
        .expect("static call script must compile");
    let action_def = Arc::new(arc_swap::ArcSwap::new(app.def.load_full()));
    // spec: ACN#action-invocation
    // The action call table makes captured FnPtrs available to
    // app.action(name) / Action.call(...) for the duration of the
    // outer closure body. The outer action's name seeds the call
    // stack so a self-call is rejected as recursion.
    let outer_name = ActionName::new_unchecked(action_name);
    let rt_for_table = rt_clone_for_active.clone();
    let result = crate::defs::app::with_action_call_table(captured_actions, outer_name, || {
        let _guard =
            ActionClosureGuard::new(action_def, op_id_str.clone(), operation_volume_bindings)
                .with_active_rt(rt_for_table);
        engine.eval_ast_with_scope::<Dynamic>(scope, &call_ast)
    });

    let _ = scope.remove::<Dynamic>("__bsl_rt");
    let _ = scope.remove::<Dynamic>("__bsl_closure");
    let _ = scope.remove::<crate::defs::app::App>("__bsl_app");
    let _ = scope.remove::<Dynamic>("__bsl_old_app");
    let _ = scope.remove::<rhai::Map>("__bsl_param");
    if is_shell {
        let _ = scope.remove::<super::shell::ShellControl>("__bsl_shell");
    }

    // Flush pending entries from the context to the log.
    let pending = ctx.lock().take_pending();
    log.commit(&pending);
    if let Some(ap) = &active_progress {
        let loaded = match log.load() {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!("failed to load action log for progress update: {e}");
                vec![]
            }
        };
        let mut progress = OperationProgress::from_log(&loaded);
        progress.dynamic_defs = ctx.lock().dynamic_defs.clone();
        *ap.write() = Some(progress);
    }
    if let Some(notify) = &tick_notify {
        notify.notify_one();
    }

    match result {
        Ok(_) => OperationResult::Completed,
        Err(ref e) => {
            // spec: OPS#cancellation
            if extract_cancel_hit(e) {
                return OperationResult::Cancelled;
            }
            match extract_barrier_hit(e) {
                Some(condition) => OperationResult::Suspended(condition),
                None => OperationResult::Failed(result.unwrap_err()),
            }
        }
    }
}

/// Compare a freshly-evaluated AppDef against the stored one and warn if they
/// differ. A mismatch means the BSL script produces different structure on
/// re-run, which indicates a non-idempotent script and may cause the closure's
/// captured variables to be inconsistent with the stored resource state.
// spec: SUSP#replay
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

/// How many parameters the closure `closure` declares, not counting the
/// variables it captured, or `None` if `ast` does not hold it.
fn closure_arity(ast: &rhai::AST, closure: &rhai::FnPtr) -> Option<usize> {
    ast.iter_functions()
        .find(|f| f.name == closure.fn_name())
        .map(|f| f.params.len().saturating_sub(closure.curry().len()))
}
