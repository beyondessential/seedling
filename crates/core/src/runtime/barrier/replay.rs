use std::{collections::HashMap, sync::Arc};

use parking_lot::{Mutex, RwLock};
use rhai::{AST, Dynamic, Engine, EvalAltResult, Scope};
use seedling_protocol::events::OperationTarget;
use seedling_protocol::names::{ActionName, AppName, GateName};
use tokio::sync::Notify;

use crate::defs::app::{App, capture_closures};
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
    target: OperationTarget,
}

impl DbActionLog {
    pub fn new(
        db: crate::runtime::db::DbHandle,
        operation_id: super::OperationId,
        app: AppName,
        target: OperationTarget,
    ) -> Self {
        Self {
            db,
            operation_id,
            app,
            target,
        }
    }
}

impl DbActionLog {
    fn commit_checked(&self, entries: &[super::ActionLogEntry]) -> Result<(), String> {
        let op_id = self.operation_id.clone();
        let app = self.app.clone();
        let target = self.target.clone();
        let entries = entries.to_vec();
        self.db
            .call(move |db| {
                for entry in &entries {
                    history::insert_action_log_entry(db, &op_id, &app, target.name(), entry)?;
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
            self.target.clone(),
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
        let target = self.target.clone();
        // spec: RCN#reconciliation-loop
        let entries = entries.to_vec();
        self.db.call(move |db| {
            for entry in &entries {
                if let Err(e) =
                    history::insert_action_log_entry(db, &op_id, &app, target.name(), entry)
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
                        operation = %target,
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

/// What one run of the runtime executes: an action's closure, a shell's, or
/// a gate's body.
// spec: OPS#operation
#[derive(Clone, Copy)]
pub enum Subject<'a> {
    Action(&'a str),
    /// A shell session's closure, which does not drive the app's declared
    /// resources.
    // spec: OPS#shell-sessions
    Shell(&'a str),
    Gate(GateBody<'a>),
}

impl Subject<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Action(name) | Self::Shell(name) => name,
            Self::Gate(gate) => gate.name.as_str(),
        }
    }
}

/// The gate a run executes the body of.
#[derive(Clone, Copy)]
pub struct GateBody<'a> {
    pub name: &'a GateName,
    /// The app as it was at the gate's last success, or `None` for a gate
    /// that has none, which the body receives as the unit value.
    // spec: GATE#body
    pub last: Option<&'a App>,
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
    pub subject: Subject<'a>,
    pub log: &'a dyn ActionLog,
    pub world: Arc<W>,
    pub registry: Arc<dyn InstanceRegistry>,
    pub active_progress: Option<Arc<RwLock<Option<OperationProgress>>>>,
    pub tick_notify: Option<Arc<Notify>>,
    /// Action params passed by the invoker. For install actions, contains the
    /// validated requirements. Empty map for actions with no params.
    // spec: OPS#persistence
    pub params: serde_json::Map<String, serde_json::Value>,
    /// DB handle for persisting dynamic resources created during the operation.
    pub db: Option<crate::runtime::db::DbHandle>,
    /// Cipher for sealing and opening the secret parameters of a recorded
    /// `Action.invoke` call.
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
    /// What `rt.start` and `rt.stop` leave out on account of gates.
    // spec: RT#resources-a-gate-holds
    pub exclusions: crate::runtime::gates::Exclusions,
    /// The seed the closure's derivation helpers derive from, the same for
    /// every pass of the operation or shell session.
    // spec: SEC#operation-seeds
    pub operation_seed: crate::runtime::seeds::Seed,
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
        operation_seed,
        operation_id,
        app,
        subject,
        log,
        world,
        registry,
        active_progress,
        tick_notify,
        params,
        db,
        cipher,
        operation_volume_bindings,
        cancel_token,
        container_signaler,
        volume_writer,
        executor,
        exclusions,
    } = op;
    let action_name = subject.name();
    let requested_shell = matches!(subject, Subject::Shell(_));
    let gate = match subject {
        Subject::Gate(gate) => Some(gate),
        _ => None,
    };

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
    ctx.lock().is_shell = requested_shell;
    // spec: RT#resources-a-gate-holds
    ctx.lock().gate_exclusions = exclusions;
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

    // FnPtrs are not stored in AppDef (which must be Send), so the script is
    // evaluated again to recover this action's.
    let (closure, is_shell, captured_actions) = {
        let captured = match capture_closures(engine, script_ast, app) {
            Ok(captured) => captured,
            Err(e) => return OperationResult::Failed(e),
        };

        let captured_actions = captured.actions.clone();
        let (closure, is_shell) = if let Some(gate) = &gate {
            // spec: GATE#body
            match captured.gates.get(gate.name) {
                Some(Some(body)) => (body.clone(), false),
                Some(None) => {
                    return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                        format!("Gate '{}' has no body", gate.name).into(),
                        rhai::Position::NONE,
                    )));
                }
                None => {
                    return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                        format!("Gate '{}' not found", gate.name).into(),
                        rhai::Position::NONE,
                    )));
                }
            }
        } else if requested_shell {
            // spec: ACN#shells
            // A shell is looked up among shells, and an action among
            // actions: a shell and an action may share a name.
            match captured.shells.get(action_name) {
                Some(c) => (c.clone(), true),
                None => {
                    return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                        format!("Shell '{}' not found", action_name).into(),
                        rhai::Position::NONE,
                    )));
                }
            }
        } else if let Some(c) = captured.actions.get(action_name) {
            (c.clone(), false)
        } else {
            return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                format!("Action '{}' not found", action_name).into(),
                rhai::Position::NONE,
            )));
        };
        (closure, is_shell, captured_actions)
    };

    // spec: GATE#body
    // A gate's body takes as many of its arguments as it declares parameters
    // for, so how many that is is read before the closure is called.
    let gate_call = match &gate {
        Some(_) => match crate::runtime::probe::closure_arity(script_ast, &closure) {
            Some(arity @ 0..=2) => Some(arity),
            Some(arity) => {
                return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                    format!(
                        "the body of gate '{action_name}' takes {arity} parameters, but it is \
                         called with at most two: a runtime handle and `last`"
                    )
                    .into(),
                    rhai::Position::NONE,
                )));
            }
            None => {
                return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                    format!("the body of gate '{action_name}' cannot be called").into(),
                    rhai::Position::NONE,
                )));
            }
        },
        None => None,
    };
    scope.push("__bsl_rt", rt);
    scope.push("__bsl_closure", closure);
    let param_map = crate::defs::json::to_map(&params);
    scope.push("__bsl_param", param_map);
    if is_shell {
        scope.push("__bsl_shell", super::shell::ShellControl::new());
    }
    if let Some(gate) = &gate {
        scope.push(
            "__bsl_last",
            gate.last
                .map_or(Dynamic::UNIT, |last| Dynamic::from(last.clone())),
        );
    }

    let call_script = match gate_call {
        Some(0) => "__bsl_closure.call()",
        Some(1) => "__bsl_closure.call(__bsl_rt)",
        Some(_) => "__bsl_closure.call(__bsl_rt, __bsl_last)",
        None if is_shell => "__bsl_closure.call(__bsl_rt, __bsl_shell, __bsl_param)",
        None => "__bsl_closure.call(__bsl_rt, __bsl_param)",
    };

    // Evaluate only the closure call — do NOT re-execute the script body.
    // The closure already captured its entire environment (app, resources,
    // params, etc.) at creation time during run_file.  Re-executing the script
    // here would run top-level statements such as param.validate() inside the
    // ActionClosureGuard, which incorrectly triggers the in-closure guard check.
    let call_ast = engine
        .compile(call_script)
        .expect("static call script must compile");
    let action_def = Arc::new(arc_swap::ArcSwap::new(app.def.load_full()));
    // spec: ACN#action-invocation
    // The action call table makes captured FnPtrs available to
    // app.action(name) / Action.call(...) for the duration of the
    // outer closure body. The outer action's name seeds the call
    // stack so a self-call is rejected as recursion; a gate's body is not
    // an action, so it seeds nothing.
    let outer_name = gate
        .is_none()
        .then(|| ActionName::new_unchecked(action_name.to_owned()));
    let rt_for_table = rt_clone_for_active.clone();
    let result = crate::defs::app::with_action_call_table(captured_actions, outer_name, || {
        let _guard =
            ActionClosureGuard::new(action_def, op_id_str.clone(), operation_volume_bindings)
                .with_active_rt(rt_for_table)
                .with_operation_seed(&operation_seed);
        engine.eval_ast_with_scope::<Dynamic>(scope, &call_ast)
    });

    let _ = scope.remove::<Dynamic>("__bsl_rt");
    let _ = scope.remove::<Dynamic>("__bsl_closure");
    let _ = scope.remove::<rhai::Map>("__bsl_param");
    if is_shell {
        let _ = scope.remove::<super::shell::ShellControl>("__bsl_shell");
    }
    if gate.is_some() {
        let _ = scope.remove::<Dynamic>("__bsl_last");
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
        // spec: OPS#initiating-events
        progress.definition = Some(app.def.load_full());
        // spec: RCN#desired-state
        progress.gate_run = gate.map(|gate| gate.name.clone());
        // spec: RT#starting-resources
        // A shell does not drive its app's declared resources: the steady
        // state or the operation in progress does.
        if is_shell {
            progress.retain_dynamic();
        }
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

#[cfg(test)]
mod tests;
