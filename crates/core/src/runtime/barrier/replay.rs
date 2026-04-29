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
    fn load(&self) -> Result<Vec<ActionLogEntry>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.entries.lock().clone())
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

impl ActionLog for DbActionLog {
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
        // r[impl reconciliation.idempotency]
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

// r[impl operation.lifecycle]
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
    // r[impl operation.cancel]
    Cancelled,
}

// ---------------------------------------------------------------------------
// run_operation
// ---------------------------------------------------------------------------

fn json_value_to_rhai_dynamic(v: &serde_json::Value) -> rhai::Dynamic {
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
    // r[impl operation.params]
    pub params: serde_json::Map<String, serde_json::Value>,
    /// `true` when this operation executes a shell action closure.
    /// Affects the call script used to invoke the closure.
    pub is_shell: bool,
    /// DB handle for persisting dynamic resources created during the operation.
    pub db: Option<crate::runtime::db::DbHandle>,
    /// Generation that was current immediately before the change that
    /// triggered this operation. Used to materialise `old` for `on_change`
    /// handlers.
    // r[impl operation.lifecycle.generations]
    pub source_generation: u64,
    /// Generation produced by the change that triggered this operation, or
    /// the current generation at dispatch for operator-invoked actions.
    // r[impl operation.lifecycle.generations]
    pub target_generation: u64,
    /// Script limits to use when reconstructing the prior `App` for
    /// `on_change` handlers. Required only when source_generation > 0 and
    /// the action is a parameter-change handler.
    pub script_limits: Option<crate::ScriptLimits>,
    /// Cipher for decrypting secret param history when reconstructing the
    /// prior `App` for `on_change` handlers.
    pub cipher: Option<std::sync::Arc<crate::runtime::secrets::Cipher>>,
    /// Operation-scoped external volume bindings injected by the runtime.
    /// Empty for normal operations; populated for internal operations such as
    /// backup actions that need to expose snapshot paths to the closure.
    // l[impl volume.external.dynamic]
    pub operation_volume_bindings: HashMap<String, OperationVolumeBinding>,
    /// Cooperative cancellation signal. Barriers consult it at entry so an
    /// in-flight cancel unwinds quickly instead of waiting for the next
    /// deadline.
    // r[impl operation.cancel]
    pub cancel_token: Arc<CancelToken>,
    /// Hook for `rt.signal()`. `None` in language-only test contexts where
    /// no real container runtime exists.
    // l[impl rt.signal]
    pub container_signaler: Option<std::sync::Arc<dyn crate::runtime::barrier::ContainerSignaler>>,
    /// Hook for `rt.write()`. `None` in language-only test contexts where no
    /// real filesystem is involved.
    // l[impl rt.write]
    pub volume_writer: Option<std::sync::Arc<dyn crate::runtime::barrier::VolumeWriter>>,
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
        params,
        is_shell: _,
        db,
        source_generation,
        target_generation: _,
        script_limits,
        cipher,
        operation_volume_bindings,
        cancel_token,
        container_signaler,
        volume_writer,
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
    let ctx = Arc::new(Mutex::new(ReplayContext::new(
        operation_id,
        committed,
        world as Arc<dyn WorldStateOracle>,
        Arc::clone(&cancel_token),
    )));
    // l[impl rt.signal]
    ctx.lock().container_signaler = container_signaler;
    // l[impl rt.write]
    ctx.lock().volume_writer = volume_writer;

    // Clear the thread-local barrier-hit flag at the start of each pass.
    clear_barrier_hit();

    let app_name = app.def.load().name.clone();
    let rt =
        RuntimeInstance::with_context(Arc::clone(&ctx), app_name.clone(), registry, db.clone());

    // Re-run the BSL script with a fresh scope and App to recover the FnPtr
    // for this action. FnPtrs are not stored in AppDef (which must be Send);
    // the thread-local capture buffer collects them during the re-run and is
    // discarded immediately after. The fresh AppDef is compared against the
    // stored one as an idempotency check, then also discarded.
    let (closure, is_param_change, is_shell) = {
        let (mut fresh_scope, fresh_app) = crate::defs::scope();
        fresh_app.def.rcu(|d| {
            let mut d = (**d).clone();
            d.name = app_name.clone();
            d
        });
        // i[param.store] — restore persisted param values so is_set()/value()
        // return correct results when the script is re-evaluated for closure recovery.
        *fresh_app.stored.lock() = app.stored.lock().clone();
        begin_closure_capture();
        let run_result = engine.run_ast_with_scope(&mut fresh_scope, script_ast);
        let captured = end_closure_capture(); // always drain, even on error
        if let Err(e) = run_result {
            return OperationResult::Failed(e);
        }
        check_idempotent(&fresh_app.def.load(), &app.def.load());

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
            !is_install && fresh_app.def.load().param_changes.contains(action_name);
        (closure, is_param_change, is_shell)
        // captured, fresh_scope, and fresh_app are all dropped here.
    };

    // l[impl param.on-change.old]
    // For param-change handlers, materialise `old` from the previous
    // generation (= source_generation). For other operations, leave it
    // empty: the spec only defines `old` for on_change handlers, but the
    // closure-call script still references the variable.
    let old_app = if is_param_change && source_generation > 0 {
        match (&db, &script_limits, &cipher) {
            (Some(db_handle), Some(limits), Some(cipher)) => {
                let app_name_owned = app.def.load().name.clone();
                let limits = limits.clone();
                let cipher = std::sync::Arc::clone(cipher);
                db_handle.call(move |db| {
                    match generations::reconstruct_app_def(
                        db,
                        &app_name_owned,
                        source_generation,
                        &limits,
                        &cipher,
                    ) {
                        Ok(a) => a,
                        Err(e) => {
                            tracing::warn!(
                                app = %app_name_owned,
                                source_generation,
                                "failed to reconstruct old App for on_change; using empty: {e}"
                            );
                            App::default()
                        }
                    }
                })
            }
            _ => {
                debug_assert!(
                    false,
                    "param_change replay missing db, script_limits, or cipher in OperationContext"
                );
                App::default()
            }
        }
    } else {
        App::default()
    };

    scope.push("__bsl_rt", rt);
    scope.push("__bsl_closure", closure);
    scope.push("__bsl_old_app", old_app);
    let param_map: rhai::Map = params
        .iter()
        .map(|(k, v)| (k.as_str().into(), json_value_to_rhai_dynamic(v)))
        .collect();
    scope.push("__bsl_param", param_map);
    if is_shell {
        scope.push("__bsl_shell", super::shell::ShellControl::new());
    }

    let call_script = if is_param_change {
        "__bsl_closure.call(__bsl_rt, __bsl_old_app)"
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
    let result = {
        let _guard =
            ActionClosureGuard::new(action_def, op_id_str.clone(), operation_volume_bindings);
        engine.eval_ast_with_scope::<Dynamic>(scope, &call_ast)
    };

    let _ = scope.remove::<Dynamic>("__bsl_rt");
    let _ = scope.remove::<Dynamic>("__bsl_closure");
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
            // r[impl operation.cancel]
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
// r[impl barrier.replay.determinism]
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
