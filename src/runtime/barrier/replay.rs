use std::sync::Arc;

use parking_lot::Mutex;
use rhai::{AST, Dynamic, Engine, EvalAltResult, Scope};

use crate::defs::app::App;
use crate::runtime::barrier::oracle::WorldStateOracle;
use crate::runtime::barrier::runtime::{RuntimeInstance, clear_barrier_hit, extract_barrier_hit};
use crate::runtime::barrier::{ActionLogEntry, BarrierCondition, OperationId, ReplayContext};

// ---------------------------------------------------------------------------
// In-memory action log (used for tests; production will use SQLite)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct InMemoryActionLog {
    entries: Mutex<Vec<ActionLogEntry>>,
}

impl InMemoryActionLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(&self) -> Vec<ActionLogEntry> {
        self.entries.lock().clone()
    }

    pub fn commit(&self, new_entries: &[ActionLogEntry]) {
        let mut entries = self.entries.lock();
        for entry in new_entries {
            if let Some(existing) = entries
                .iter_mut()
                .find(|e| e.call_index == entry.call_index)
            {
                // Update satisfaction status if it changed.
                if let (Some(nb), Some(eb)) = (&entry.barrier, &mut existing.barrier) {
                    if nb.satisfied {
                        eb.satisfied = true;
                        eb.started_at_secs = nb.started_at_secs.or(eb.started_at_secs);
                    }
                }
            } else {
                entries.push(entry.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// OperationResult
// ---------------------------------------------------------------------------

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
/// The `log` carries committed entries across calls; pass the same `log`
/// instance for all passes of the same operation to enable replay.
pub fn run_operation<W>(
    engine: &Engine,
    scope: &mut Scope,
    script_ast: &AST,
    operation_id: OperationId,
    app: &App,
    action_name: &str,
    log: &InMemoryActionLog,
    world: Arc<W>,
) -> OperationResult
where
    W: WorldStateOracle + 'static,
{
    // Build the replay context from committed log entries.
    let committed = log.load();
    let ctx = Arc::new(Mutex::new(ReplayContext::new(
        operation_id,
        committed,
        world as Arc<dyn WorldStateOracle>,
    )));

    // Clear the thread-local barrier-hit flag at the start of each pass.
    clear_barrier_hit();

    let rt = RuntimeInstance::with_context(Arc::clone(&ctx));

    // Look up the action closure.
    let (closure, is_param_change) = {
        let def = app.0.lock();
        let closure = match def.actions.get(action_name) {
            Some(a) => a.closure.clone(),
            None => {
                return OperationResult::Failed(Box::new(EvalAltResult::ErrorRuntime(
                    format!("Action '{}' not found", action_name).into(),
                    rhai::Position::NONE,
                )));
            }
        };
        // param_changes closures receive (rt, old_app) like the old upgrade action
        let is_param_change = def.param_changes.contains_key(action_name);
        (closure, is_param_change)
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

    let result = eval_merged(engine, scope, script_ast, call_script);

    let _ = scope.remove::<Dynamic>("__bsl_rt");
    let _ = scope.remove::<Dynamic>("__bsl_closure");
    let _ = scope.remove::<Dynamic>("__bsl_old_app");

    // Flush pending entries from the context to the log.
    let pending = ctx.lock().take_pending();
    log.commit(&pending);

    match result {
        Ok(_) => OperationResult::Completed,
        Err(ref e) => match extract_barrier_hit(e) {
            Some(condition) => OperationResult::Suspended(condition),
            None => OperationResult::Failed(result.unwrap_err()),
        },
    }
}

fn eval_merged(
    engine: &Engine,
    scope: &mut Scope,
    script_ast: &AST,
    call_source: &str,
) -> Result<Dynamic, Box<EvalAltResult>> {
    let call_ast = engine.compile(call_source)?;
    let merged = script_ast.merge(&call_ast);
    engine.eval_ast_with_scope(scope, &merged)
}
