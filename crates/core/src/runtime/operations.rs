//! Running a lifecycle operation: an action's closure or a gate's body, from
//! its dispatch through every replay to its end.

use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use parking_lot::{Mutex, RwLock};
use seedling_protocol::{
    events::{EventSender, OperationEventCtx, OperationTarget},
    names::AppName,
};
use tokio::sync::Notify;

use crate::{
    defs::{app::App, volume::OperationVolumeBinding},
    runtime::{
        InstanceRegistry, Scheduler,
        apps::AppRegistry,
        barrier::{
            CancelToken, OperationId,
            oracle::DbWorldOracle,
            replay::{
                DbActionLog, GateBody, OperationContext, OperationResult, Subject, run_operation,
            },
        },
        db::DbHandle,
        desired::OperationProgress,
        dynamic_cleanup::{cleanup_dynamic_resources, dynamic_poll_interval},
        faults,
        gates::GateRuntime,
        history::{CurrentOperation, clear_current_operation, save_current_operation},
        registry::DbInstanceRegistry,
        secrets::Cipher,
    },
    system::{ContainerRuntime, System},
};

pub(crate) mod adapters;
pub mod admission;

/// What the operation engine and the gate runtime run with: the daemon's
/// shared handles to apps, the scheduler, the database and the system.
pub struct Engine {
    pub registry: Arc<RwLock<AppRegistry>>,
    pub db: DbHandle,
    /// Where the database is, for an operation's blocking task to open its
    /// own connection.
    pub db_path: PathBuf,
    pub scheduler: Arc<Mutex<Scheduler>>,
    pub tick_notify: Arc<Notify>,
    pub container_runtime: Arc<dyn ContainerRuntime>,
    pub driver: Arc<System>,
    pub event_tx: EventSender,
    pub script_limits: crate::ScriptLimits,
    // spec: SEC#secret-key
    pub cipher: Arc<Cipher>,
    /// What the runtime holds in memory about its gates' checks, shared with
    /// the reconciler.
    // spec: GRUN#checks
    pub gate_runtime: Arc<GateRuntime>,
}

fn open_operation_dbs(db_path: &Path, app_name: &AppName) -> Option<DbHandle> {
    match DbHandle::open(db_path) {
        Ok(db) => Some(db),
        Err(e) => {
            tracing::error!(app = %app_name, "open operation db: {e}");
            None
        }
    }
}

/// The params an operation starts with.
pub enum OperationParams {
    /// As requested: volume params name site volumes and are bound at dispatch.
    Requested(crate::runtime::param_map::ParamMap),
    /// As persisted by an interrupted operation, with the bindings its
    /// `<key>_volume` params name, which are established again as they were.
    // spec: OPS#operation-scoped-volumes
    Replayed {
        params: crate::runtime::param_map::ParamMap,
        bindings: crate::runtime::history::OperationVolumeBindings,
    },
}

/// A dispatch's params with its volume params bound, and the bindings made.
pub(crate) struct BoundParams {
    pub params: crate::runtime::param_map::ParamMap,
    pub bindings: HashMap<String, OperationVolumeBinding>,
}

// spec: ACN#volume-params
/// For every `kind: "volume"` param in `schema`, look up the
/// operator-supplied site volume name, build an
/// [`OperationVolumeBinding`] keyed by a generated per-operation name, and
/// rewrite the params map so the closure sees `<key>_volume = <generated-name>`
/// — consumed by `app.external_volume(param[...])`.
///
/// A binding that cannot be made is an error and the closure does not run:
/// a volume that disappeared between validation and dispatch, a name that is
/// not one, or a record that cannot be read. Running it with the key missing
/// would have it act on a volume nobody chose.
pub(crate) fn bind_volume_params(
    state: &Engine,
    schema: &std::collections::BTreeMap<
        seedling_protocol::names::ParamName,
        crate::defs::param_def::ParamDef,
    >,
    mut params: crate::runtime::param_map::ParamMap,
    operation_id: &str,
) -> Result<BoundParams, String> {
    use crate::defs::param_def::ParamKind;
    use crate::defs::volume::{VolumeParamSpec, build_operation_volume_params};
    use crate::runtime::site_volumes;
    use seedling_protocol::names::SiteVolumeName;

    let mut specs: Vec<(String, VolumeParamSpec)> = Vec::new();
    let volume_keys: Vec<String> = schema
        .iter()
        .filter(|(_, p)| p.kind == ParamKind::Volume)
        .map(|(name, _)| name.as_str().to_owned())
        .collect();
    for key in &volume_keys {
        let val = match params.remove(key.as_str()) {
            Some(serde_json::Value::String(s)) if !s.is_empty() => s,
            // An optional volume param that was not supplied, or was left
            // empty, binds nothing.
            Some(serde_json::Value::String(_)) | None => continue,
            Some(_) => return Err(format!("volume param {key:?} is not a string")),
        };
        let site_name = SiteVolumeName::new(&val).map_err(|e| {
            format!("volume param {key:?} names {val:?}, which is not a volume name: {e}")
        })?;
        let lookup_name = site_name.clone();
        let site_vol = state
            .db
            .call(move |db| site_volumes::get(db, &lookup_name))
            .map_err(|e| format!("could not read site volume {val:?} for param {key:?}: {e}"))?
            .ok_or_else(|| {
                format!("site volume {val:?} named by param {key:?} no longer exists")
            })?;
        let host_path = match &site_vol.kind {
            site_volumes::SiteVolumeKind::Bind { host_path } => std::path::PathBuf::from(host_path),
            _ => state.driver.volume_store.site_path(site_name.as_str()),
        };
        let read_only = site_vol.is_read_only();
        specs.push((
            key.clone(),
            VolumeParamSpec {
                host_path,
                read_only,
                filename: None,
            },
        ));
    }

    if specs.is_empty() {
        return Ok(BoundParams {
            params,
            bindings: HashMap::new(),
        });
    }
    let (bindings, injected) = build_operation_volume_params(operation_id, specs);
    for (k, v) in injected {
        params.insert(k, v);
    }
    Ok(BoundParams { params, bindings })
}

/// The app as it was at `generation`: `current` where that is the app's
/// current generation or one that has not been recorded, and otherwise the
/// app rebuilt from the generation's record.
// spec: OPS#initiating-events
pub(crate) fn app_at_generation<'a>(
    db: &DbHandle,
    app_name: &AppName,
    current: &'a App,
    generation: u64,
    limits: &crate::ScriptLimits,
    cipher: &Arc<crate::runtime::secrets::Cipher>,
) -> Result<Cow<'a, App>, crate::runtime::generations::Error> {
    let name = app_name.clone();
    let now = db.call(move |db| crate::runtime::generations::current(db, &name))?;
    match now {
        Some(now) if generation != 0 && now != generation => {
            let (name, limits, cipher) = (app_name.clone(), limits.clone(), Arc::clone(cipher));
            db.call(move |db| {
                crate::runtime::generations::reconstruct_app_def(
                    db, &name, generation, &limits, &cipher,
                )
            })
            .map(Cow::Owned)
        }
        _ => Ok(Cow::Borrowed(current)),
    }
}

/// The script engine, a scope and the compiled script of `app`.
// spec: BND#script-formation
pub(crate) fn language_for(
    app: &App,
    limits: &crate::ScriptLimits,
) -> Result<(rhai::Engine, rhai::Scope<'static>, rhai::AST), String> {
    // spec: CONST#host-constants
    let (engine, scope, _) =
        crate::setup_language(limits, Arc::clone(&app.bundle)).map_err(|e| e.to_string())?;
    let script = app.bundle.script().map_err(|e| e.to_string())?;
    let ast = crate::defs::compile(&engine, script.text())
        .map_err(|e| script.remap_error(&e.to_string()))
        .map_err(|e| format!("script compile error: {e}"))?;
    Ok((engine, scope, ast))
}

/// How an operation's closure ended, before anything is reported of it.
pub(crate) enum OperationEnd {
    Completed,
    Failed(String),
    // spec: OPS#cancellation
    Cancelled,
    /// The operation was accepted and could not be started.
    NotRun(String),
}

/// Clear the faults a lifecycle operation that completes supersedes: an
/// earlier operation that failed or was cancelled no longer describes the
/// app.
// spec: FLT#app-faults
pub(crate) fn clear_superseded_operation_faults(db: &crate::runtime::db::Db, app: &AppName) {
    for kind in ["operation_failed", "operation_cancelled"] {
        if let Err(e) = faults::clear_faults_by_kind(db, app, kind) {
            tracing::error!(%app, "cannot clear {kind} after an operation completed: {e}");
        }
    }
}

/// End an action as `end` says: file what it calls for, clear the persisted
/// operation, and report it.
///
/// A gate run ends otherwise, as its outcome says.
// spec: OPS#completion
// spec: OPS#cancellation
fn end_action(db: &DbHandle, op_ctx: &OperationEventCtx, end: OperationEnd) {
    let app_name = op_ctx.app.clone();
    let action_name = op_ctx.target.name().to_owned();
    match end {
        OperationEnd::Completed => {
            db.call(move |db| {
                clear_superseded_operation_faults(db, &app_name);
                let _ = clear_current_operation(db);
            });
            op_ctx.completed();
        }
        OperationEnd::Failed(why) => {
            let description = format!("{action_name} failed: {why}");
            db.call(move |db| {
                let _ = faults::file_fault(
                    db,
                    &app_name,
                    None,
                    None,
                    None,
                    "operation_failed",
                    &description,
                );
                let _ = clear_current_operation(db);
            });
            op_ctx.failed(&why);
        }
        OperationEnd::NotRun(description) => {
            let for_db = description.clone();
            db.call(move |db| {
                let _ = faults::file_fault(
                    db,
                    &app_name,
                    None,
                    None,
                    None,
                    "operation_failed",
                    &for_db,
                );
                let _ = clear_current_operation(db);
            });
            op_ctx.failed(&description);
        }
        OperationEnd::Cancelled => {
            let description = format!("{action_name} cancelled by operator");
            db.call(move |db| {
                let _ = faults::file_fault(
                    db,
                    &app_name,
                    None,
                    None,
                    None,
                    "operation_cancelled",
                    &description,
                );
                let _ = clear_current_operation(db);
            });
            // spec: OPS#cancellation
            // A cancelled operation ends in an outcome of its own.
            op_ctx.cancelled();
        }
    }
}

/// What a gate's run gives the closure it calls.
pub(crate) struct GateInput<'a> {
    /// The app as it was at the gate's last success, if it has one.
    // spec: GATE#body
    pub last: Option<&'a App>,
}

/// Where a pass of the operation loop stands in what an operation persists.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Persisted {
    /// The loop persists the operation before the closure first runs.
    No,
    /// The operation is already on record.
    Yes,
}

/// Run the operation loop synchronously until completion or failure.
#[expect(
    clippy::too_many_arguments,
    reason = "operation state is inherently multi-dimensional"
)]
pub(crate) fn run_operation_loop(
    app: &App,
    db: DbHandle,
    bound: Result<BoundParams, String>,
    active_progress: Arc<RwLock<Option<OperationProgress>>>,
    tick_notify: Arc<Notify>,
    op_ctx: &OperationEventCtx,
    script_limits: &crate::ScriptLimits,
    cipher: Arc<crate::runtime::secrets::Cipher>,
    gate_runtime: &crate::runtime::gates::GateRuntime,
    apps: &RwLock<crate::runtime::apps::AppRegistry>,
    cancel_token: Arc<CancelToken>,
    container_signaler: Option<Arc<dyn crate::runtime::barrier::ContainerSignaler>>,
    volume_writer: Option<Arc<dyn crate::runtime::barrier::VolumeWriter>>,
    executor: Option<Arc<dyn crate::runtime::barrier::Executor>>,
    gate: Option<GateInput<'_>>,
    persisted: Persisted,
) -> OperationEnd {
    let app_name = &op_ctx.app;
    let target = &op_ctx.target;
    let action_name = target.name();
    let operation_id = OperationId(op_ctx.operation_id.clone());

    // spec: ACN#volume-params
    // The closure does not run if a volume param could not be bound.
    let BoundParams {
        params,
        bindings: operation_volume_bindings,
    } = match bound {
        Ok(bound) => bound,
        Err(why) => {
            tracing::error!(app = %app_name, action = %action_name, "volume binding failed: {why}");
            return OperationEnd::NotRun(format!(
                "{action_name} not run: a volume param could not be bound: {why}"
            ));
        }
    };

    // spec: OPS#initiating-events
    // The operation runs against the definition and parameters of its
    // generation, however many generations have been committed since it was
    // dispatched, and on every replay. If that generation cannot be rebuilt
    // the operation fails; it is never run against another's state.
    let at_generation = match app_at_generation(
        &db,
        app_name,
        app,
        op_ctx.operation_generation,
        script_limits,
        &cipher,
    ) {
        Ok(app) => app,
        Err(e) => {
            tracing::error!(app = %app_name, action = %action_name, "cannot rebuild generation {}: {e}", op_ctx.operation_generation);
            return OperationEnd::NotRun(format!(
                "{action_name} not run: generation {} cannot be rebuilt: {e}",
                op_ctx.operation_generation
            ));
        }
    };
    let app = &*at_generation;

    let (engine, mut scope, ast) = match language_for(app, script_limits) {
        Ok(language) => language,
        Err(why) => {
            tracing::error!(app = %app_name, action = %action_name, "{action_name} not run: {why}");
            return OperationEnd::NotRun(format!("{action_name} not run: {why}"));
        }
    };

    let log = DbActionLog::new(
        db.clone(),
        operation_id.clone(),
        app_name.clone(),
        target.clone(),
    );
    let world = Arc::new(DbWorldOracle::new(db.clone()));
    let registry: Arc<dyn InstanceRegistry> = Arc::new(DbInstanceRegistry::new(db.clone()));

    // spec: OPS#initiating-events, SUSP#replay, OPS#persistence
    // Persist the current operation so a runtime restart can resume it.
    // Params are encrypted because they may carry secret values.
    // spec: OPS#persistence
    // The closure does not run unless the operation is on record: a restart
    // could not otherwise replay it, and its params could not be recovered.
    if persisted == Persisted::No {
        let record = CurrentOperation {
            operation_id: operation_id.clone(),
            app: app_name.clone(),
            target: target.clone(),
            generation: op_ctx.operation_generation,
        };
        let params_for_persist = params.clone();
        let bindings_for_persist = operation_volume_bindings.clone();
        let cipher_for_persist = Arc::clone(&cipher);
        if let Err(e) = db.call(move |db| {
            save_current_operation(
                db,
                &cipher_for_persist,
                &record,
                &params_for_persist,
                &bindings_for_persist,
            )
        }) {
            tracing::error!(
                app = %app_name,
                action = %action_name,
                "failed to persist current_operation; not running the operation: {e}"
            );
            return OperationEnd::NotRun(format!(
                "{action_name} not run: the operation could not be persisted: {e}"
            ));
        }
    }

    // spec: SEC#operation-seeds
    // Every pass, and every replay after a restart, derives from the seed on
    // record for the operation, recorded here before the first pass.
    let operation_seed = {
        let (id, cipher) = (operation_id.clone(), Arc::clone(&cipher));
        match db.call(move |db| crate::runtime::history::load_operation_seed(db, &cipher, &id)) {
            Ok(seed) => seed,
            Err(e) => {
                tracing::error!(app = %app_name, action = %action_name, "{action_name} not run: {e}");
                return OperationEnd::NotRun(format!("{action_name} not run: {e}"));
            }
        }
    };

    // spec: SEC#matching
    let sensitive = {
        let (name, cipher, def, stored) = (
            app_name.clone(),
            Arc::clone(&cipher),
            app.def.load_full(),
            app.stored.lock().clone(),
        );
        match db.call(move |db| {
            crate::runtime::apps::sensitive_values(db, &cipher, &name, &def, &stored)
        }) {
            Ok(values) => crate::runtime::redact::Scope::new(values),
            Err(e) => {
                tracing::error!(app = %app_name, action = %action_name, "{action_name} not run: cannot read the app's sensitive values: {e}");
                return OperationEnd::NotRun(format!(
                    "{action_name} not run: the app's sensitive values cannot be read: {e}"
                ));
            }
        }
    };
    if let Some(action) = target.action() {
        let def = app.def.load();
        let schema = def.actions.get(action.as_str()).map(|a| &a.params);
        sensitive.insert_secret_params(params.values(), |name| {
            schema
                .and_then(|schema| schema.get(name))
                .is_some_and(|p| p.is_secret())
        });
    }

    // spec: LIM#breakers
    // Only a pass at the app's current generation is tracked; one at an
    // earlier generation is stopped at the limit all the same.
    let thing = match (&gate, target.gate(), target.action()) {
        (Some(_), Some(name), _) => crate::runtime::breakers::Thing::Gate(name.clone()),
        (_, _, Some(name)) => crate::runtime::breakers::Thing::Action(name.clone()),
        _ => unreachable!("an operation runs an action or a gate's body"),
    };
    // What every evaluation of this app runs with, before the params or the
    // last success of the one thing: the base an `invoke` is consulted with
    // as well, so it is built once.
    let base = crate::runtime::breakers::Run::of_app(app, script_limits);
    let run = {
        let run = base.clone();
        match &gate {
            Some(input) => run.with_last_success(input.last),
            None => {
                let def = app.def.load();
                let schema = target
                    .action()
                    .and_then(|action| def.actions.get(action))
                    .map(|a| a.params.clone())
                    .unwrap_or_default();
                run.with_params(&params, &schema)
            }
        }
    };
    let tracked = || {
        apps.read()
            .get(app_name.as_str())
            .is_some_and(|e| e.current_generation == op_ctx.operation_generation)
    };
    // spec: ACN#action-invocation
    let _invokes = crate::runtime::breakers::InvokesGuard::enter(
        base,
        crate::runtime::breakers::consulting(app_name, db.clone(), Arc::clone(&cipher)),
    );
    let mut standing = None;
    if tracked() {
        let (name, cipher, checked, run) = (
            app_name.clone(),
            Arc::clone(&cipher),
            thing.clone(),
            run.clone(),
        );
        match db
            .call(move |db| crate::runtime::breakers::consult(db, &cipher, &name, &checked, &run))
        {
            Ok(s) => standing = Some(s),
            Err(refusal) => return OperationEnd::Failed(refusal.message().to_owned()),
        }
    }

    loop {
        // spec: RT#resources-a-gate-holds
        // What gates keep from the closure is read for each pass, so a pass
        // that follows a wait sees the gates as they stand then. In a gate's
        // body, the gate's own holds and dependants are not kept from it.
        let exclusions = {
            let (name, cipher, remembered) = (
                app_name.clone(),
                Arc::clone(&cipher),
                gate_runtime.remembered(app_name),
            );
            let def = app.def.load_full();
            let current = crate::runtime::gates::CurrentDefinition::of(apps, app_name);
            let running = target.gate().cloned();
            db.call(move |db| {
                crate::runtime::gates::Exclusions::for_closure(
                    db,
                    &name,
                    &def,
                    current.as_ref(),
                    &cipher,
                    &remembered,
                    running.as_ref(),
                )
            })
        };
        // spec: LIM#measuring-evaluation-time
        // A pass is one evaluation, the definition it begins with included.
        let evaluation = crate::evaluation::Evaluation::start(script_limits);
        let result = run_operation(
            OperationContext {
                engine: &engine,
                script_ast: &ast,
                operation_id: operation_id.clone(),
                app,
                subject: match (&gate, target.gate()) {
                    (Some(input), Some(name)) => Subject::Gate(GateBody {
                        name,
                        last: input.last,
                    }),
                    _ => Subject::Action(action_name),
                },
                operation_seed: operation_seed.clone(),
                sensitive: sensitive.clone(),
                log: &log,
                world: Arc::clone(&world),
                registry: Arc::clone(&registry),
                active_progress: Some(Arc::clone(&active_progress)),
                tick_notify: Some(Arc::clone(&tick_notify)),
                params: params.clone(),
                db: Some(db.clone()),
                cipher: Some(Arc::clone(&cipher)),
                operation_volume_bindings: operation_volume_bindings.clone(),
                cancel_token: Arc::clone(&cancel_token),
                container_signaler: container_signaler.clone(),
                volume_writer: volume_writer.clone(),
                executor: executor.clone(),
                exclusions,
            },
            &mut scope,
        );
        // spec: LIM#measuring-evaluation-time
        // spec: LIM#breakers
        let ran_out = evaluation.exceeded();
        drop(evaluation);
        if ran_out {
            let why = crate::evaluation::out_of_time_message(script_limits);
            tracing::error!(app = %app_name, operation = %target, "operation failed: {why}");
            if tracked() {
                crate::runtime::breakers::record_outcome(
                    &db,
                    &cipher,
                    app_name,
                    &thing,
                    &run,
                    crate::runtime::breakers::Outcome::RanOut {
                        generation: op_ctx.operation_generation,
                        limits: script_limits.clone(),
                    },
                );
            }
            return OperationEnd::Failed(why);
        }
        if let Some(standing) = standing.take() {
            crate::runtime::breakers::record_outcome(
                &db,
                &cipher,
                app_name,
                &thing,
                &run,
                crate::runtime::breakers::Outcome::InTime(standing),
            );
        }
        match result {
            OperationResult::Completed => return OperationEnd::Completed,
            OperationResult::Failed(e) => {
                // spec: SEC#withheld-values
                let why = sensitive.values().mask(&e.to_string()).into_owned();
                tracing::error!(app = %app_name, operation = %target, "operation failed: {why}");
                return OperationEnd::Failed(why);
            }
            // spec: OPS#cancellation
            OperationResult::Cancelled => {
                tracing::warn!(app = %app_name, operation = %target, "operation cancelled");
                return OperationEnd::Cancelled;
            }
            OperationResult::Suspended(cond) => {
                tick_notify.notify_one();
                let waited = earliest_unsatisfied_barrier_wait_secs(&log);
                // Surface long-running barriers so operators can tell a
                // legitimately-long wait from a stuck one.
                if waited > 0 && waited.is_multiple_of(600) {
                    tracing::info!(
                        app = %app_name,
                        operation = %target,
                        state = ?cond.required_state,
                        elapsed_secs = waited,
                        deadline_secs = ?cond.deadline_secs,
                        "barrier still waiting",
                    );
                }
                wait_next_tick(&cancel_token, waited);
            }
        }
    }
}

/// Sleep between replay cycles. Returns early if the operation was cancelled.
///
/// Cadence is dynamic: short while freshly suspended (so short barriers feel
/// responsive) and long for protracted waits (so an hours-long wait doesn't
/// replay tens of thousands of times).
// spec: SUSP#re-evaluation-cadence
fn wait_next_tick(cancel_token: &CancelToken, waited_on_barrier_secs: u64) {
    let interval = dynamic_poll_interval(waited_on_barrier_secs);
    // The cancel token's condvar wakes the sleep so an operator-initiated
    // cancel takes effect within one observation, not up to `interval` later.
    cancel_token.wait_for(interval);
}

/// How long has the operation been blocked on its earliest unsatisfied
/// barrier? Returns 0 if none.
fn earliest_unsatisfied_barrier_wait_secs(log: &DbActionLog) -> u64 {
    use crate::runtime::barrier::replay::ActionLog;
    let Ok(entries) = log.load() else { return 0 };
    let now = now_secs();
    entries
        .iter()
        .flat_map(|e| e.barriers.iter())
        .filter(|b| !b.satisfied)
        .filter_map(|b| b.started_at_secs)
        .map(|started| now.saturating_sub(started))
        .max()
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[expect(
    clippy::too_many_arguments,
    reason = "internal helper grouping all operation state"
)]
pub fn spawn_accepted_operation(
    state: Arc<Engine>,
    app_name: AppName,
    target: OperationTarget,
    operation_id: OperationId,
    params: OperationParams,
    generation: u64,
    trigger: String,
    actor: Option<std::sync::Arc<seedling_protocol::actor::Actor>>,
) {
    let (app, active_progress, tick_notify) = {
        let reg = state.registry.read();
        match reg.get(app_name.as_str()) {
            Some(e) => (
                e.app.clone(),
                Arc::clone(&e.active_progress),
                Arc::clone(&e.tick_notify),
            ),
            None => {
                tracing::error!(app = %app_name, "spawn_accepted_operation: app not found");
                return;
            }
        }
    };
    let db_path = state.db_path.clone();
    let event_tx = state.event_tx.clone();
    let script_limits = state.script_limits.clone();
    let cipher = Arc::clone(&state.cipher);

    tokio::spawn(async move {
        // spec: WIRE#actor-attribution
        let op_ctx = event_tx.operation(
            app_name.clone(),
            target.clone(),
            &operation_id.0,
            generation,
            actor,
        );
        op_ctx.started(&trigger);

        // spec: OPS#concurrency
        // The request is no longer waiting, so its record goes.
        {
            let id = operation_id.clone();
            state.db.call(move |db| {
                let _ = crate::runtime::history::clear_queued_operation(db, &id);
            });
        }

        let operation_id_str = operation_id.0.clone();

        // spec: OPS#cancellation
        // Take the cancel token from the active scheduler record so a
        // subsequent cancel endpoint call wakes this operation.
        let cancel_token = state
            .scheduler
            .lock()
            .active()
            .filter(|a| a.operation_id == operation_id)
            .map(|a| Arc::clone(&a.cancel_token))
            .unwrap_or_else(|| Arc::new(CancelToken::new()));

        // spec: ACN#volume-params
        // Resolve volume params: replace each `kind: "volume"` param with the
        // operator-supplied volume reference, looked up to a host path and
        // exposed to the action closure as a per-operation external volume
        // binding via the standard `<key>_volume` convention.
        let schema = target
            .action()
            .and_then(|action| app.def.load().actions.get(action.as_str()).cloned())
            .map(|a| a.params)
            .unwrap_or_default();
        let bound = match params {
            OperationParams::Requested(params) => {
                bind_volume_params(&state, &schema, params, &operation_id.0)
            }
            // spec: OPS#operation-scoped-volumes
            OperationParams::Replayed { params, bindings } => Ok(BoundParams { params, bindings }),
        };

        match target.gate().cloned() {
            None => {
                let (app_name_for_task, op_ctx_for_task) = (app_name.clone(), op_ctx.clone());
                let active_progress = Arc::clone(&active_progress);
                let tick_notify = Arc::clone(&tick_notify);
                let cipher = Arc::clone(&cipher);
                let cancel_token = Arc::clone(&cancel_token);
                let gate_runtime = Arc::clone(&state.gate_runtime);
                let apps = Arc::clone(&state.registry);

                let signaler = adapters::make_container_signaler(&state);
                let writer = adapters::make_volume_writer(&state);
                let executor = adapters::make_executor(&state);
                let db_path = db_path.clone();
                let script_limits = script_limits.clone();
                let app = app.clone();
                let end = tokio::task::spawn_blocking(move || {
                    let db = match open_operation_dbs(&db_path, &app_name_for_task) {
                        Some(d) => d,
                        None => {
                            return OperationEnd::NotRun(format!(
                                "{} not run: the database could not be opened",
                                op_ctx_for_task.target
                            ));
                        }
                    };
                    run_operation_loop(
                        &app,
                        db,
                        bound,
                        active_progress,
                        tick_notify,
                        &op_ctx_for_task,
                        &script_limits,
                        cipher,
                        &gate_runtime,
                        &apps,
                        cancel_token,
                        signaler,
                        writer,
                        executor,
                        None,
                        Persisted::No,
                    )
                })
                .await;
                match end {
                    Ok(end) => end_action(&state.db, &op_ctx, end),
                    Err(e) => {
                        tracing::error!(app = %app_name, operation = %target, "operation task failed: {e}");
                        end_action(
                            &state.db,
                            &op_ctx,
                            OperationEnd::Failed(format!("the operation's task failed: {e}")),
                        );
                    }
                }
            }
            Some(gate) => {
                crate::runtime::gates::run::run_gate(
                    &state,
                    crate::runtime::gates::run::GateRunInput {
                        app: app.clone(),
                        gate,
                        op_ctx: op_ctx.clone(),
                        active_progress: Arc::clone(&active_progress),
                        tick_notify: Arc::clone(&tick_notify),
                        cancel_token: Arc::clone(&cancel_token),
                        db_path: db_path.clone(),
                    },
                )
                .await;
            }
        }

        cleanup_dynamic_resources(
            &state.db,
            &state.driver,
            &app_name,
            &operation_id_str,
            &active_progress,
            &tick_notify,
        )
        .await;

        *active_progress.write() = None;
        tick_notify.notify_one();

        let next = complete_and_dequeue(&state);
        if let Some(queued) = next {
            spawn_accepted_operation(
                Arc::clone(&state),
                queued.app,
                queued.target,
                queued.operation_id,
                OperationParams::Requested(queued.params),
                queued.generation,
                queued.trigger,
                queued.actor,
            );
        }
    });
}

/// End the active operation and start the next queued one, if any, at the
/// generation current as it leaves the queue.
///
/// The registry is read before the scheduler is locked: the scheduler lock is
/// never held while waiting on the registry.
// spec: OPS#initiating-events
fn complete_and_dequeue(state: &Engine) -> Option<crate::runtime::scheduler::QueuedOperation> {
    let registry = state.registry.read();
    state
        .scheduler
        .lock()
        .complete_current(|app| registry.get(app.as_str()).map(|e| e.current_generation))
}

#[cfg(test)]
mod tests;
