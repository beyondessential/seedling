use std::{borrow::Cow, collections::HashMap, path::Path, sync::Arc};

use parking_lot::RwLock;
use seedling_protocol::events::{OperationEventCtx, OperationTarget};
use seedling_protocol::names::AppName;
use tokio::sync::Notify;

use crate::{
    defs::{app::App, volume::OperationVolumeBinding},
    oi::state::OiState,
    runtime::{
        AppPhase, InstanceRegistry,
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
        history::{CurrentOperation, clear_current_operation, save_current_operation},
        registry::DbInstanceRegistry,
    },
};

fn open_operation_dbs(db_path: &Path, app_name: &AppName) -> Option<DbHandle> {
    match DbHandle::open(db_path) {
        Ok(db) => Some(db),
        Err(e) => {
            tracing::error!(app = %app_name, "open operation db: {e}");
            None
        }
    }
}

// spec: RT#signal-delivery
/// Adapter from the system actuator's async `signal_container` to the
/// synchronous `ContainerSignaler` trait expected by the BSL runtime. The
/// adapter is constructed in the OI layer so that language-only tests don't
/// drag in a tokio runtime.
struct OperationSignaler {
    container: Arc<dyn crate::system::ContainerRuntime>,
}

impl crate::runtime::barrier::ContainerSignaler for OperationSignaler {
    fn signal(&self, container_name: &str, signal: &str) -> Result<bool, String> {
        let container = Arc::clone(&self.container);
        let name = container_name.to_owned();
        let sig = signal.to_owned();
        // The operation loop runs inside `spawn_blocking`, so we can step
        // back into the async runtime via `block_in_place` + the current
        // handle to deliver the signal synchronously.
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { container.signal_container(&name, &sig).await })
        });
        result.map_err(|e| e.to_string())
    }
}

pub(crate) fn make_container_signaler(
    state: &OiState,
) -> Option<Arc<dyn crate::runtime::barrier::ContainerSignaler>> {
    Some(Arc::new(OperationSignaler {
        container: Arc::clone(&state.driver.container),
    }))
}

// spec: RT#volume-writes
/// Adapter from the system actuator's async `safe_volume_write` to the
/// synchronous `VolumeWriter` trait expected by the BSL runtime, mirroring
/// `OperationSignaler`. Resolves the BSL-side volume identity to a host path
/// the same way the actuator does at container start.
struct OperationVolumeWriter {
    driver: Arc<crate::system::System>,
    db: crate::runtime::db::DbHandle,
}

impl crate::runtime::barrier::VolumeWriter for OperationVolumeWriter {
    // spec: VOL#external-volumes
    fn resolve_external(
        &self,
        app: &str,
        name: &str,
    ) -> Result<Option<crate::runtime::barrier::ResolvedExternalWrite>, String> {
        use crate::system::types::MountSource;

        let app = app
            .parse::<AppName>()
            .map_err(|e| format!("{app} is not an app name: {e}"))?;
        let mappings = crate::system::actuator::try_resolve_external_volumes(
            &self.db,
            &self.driver.volume_store,
            &app,
        )
        .map_err(|e| format!("the external volume mappings cannot be read: {e}"))?;
        match mappings.get(name) {
            None => Ok(None),
            Some(mount) => match &mount.source {
                MountSource::Bind(host_path) => {
                    Ok(Some(crate::runtime::barrier::ResolvedExternalWrite {
                        host_path: host_path.clone(),
                        read_only: mount.read_only,
                    }))
                }
                MountSource::Volume(_) | MountSource::Tmpfs => Err(format!(
                    "the mapping of external volume {name} is not a directory on the host"
                )),
            },
        }
    }

    fn write(
        &self,
        app: &str,
        target: crate::runtime::barrier::VolumeWriteTarget,
        path: &str,
        contents: &[u8],
    ) -> Result<(), String> {
        use std::path::PathBuf;

        use crate::{
            runtime::{barrier::VolumeWriteTarget, identity::VolumeName},
            system::actuator::{
                ensure_named_volume_storage, ensure_tmpfs_volume_dir, safe_volume_write,
                tmpfs_volume_path,
            },
        };

        let driver = Arc::clone(&self.driver);
        let app = app.to_owned();
        let path = path.to_owned();
        let contents = contents.to_vec();

        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let mountpoint: PathBuf = match target {
                    // spec: RT#volume-writes
                    // Storage that does not exist yet is created and seeded
                    // first, as starting the volume would.
                    VolumeWriteTarget::NamedVolume {
                        name,
                        tmpfs,
                        writes,
                    } => {
                        if tmpfs {
                            // spec: ACTU#volume-storage
                            let host_path =
                                tmpfs_volume_path(VolumeName::for_app(&app, &name).as_str());
                            ensure_tmpfs_volume_dir(&host_path, &writes)
                                .await
                                .map_err(|e| format!("rt.write: {e}"))?;
                            host_path
                        } else {
                            ensure_named_volume_storage(&driver.volume_store, &app, &name, &writes)
                                .await
                                .map_err(|e| format!("rt.write: {e}"))?
                        }
                    }
                    VolumeWriteTarget::AnonymousVolume { anon_id, tmpfs } => {
                        if tmpfs {
                            // spec: ACTU#volume-storage
                            tmpfs_volume_path(&anon_id)
                        } else {
                            driver
                                .container
                                .volume_mountpoint(&anon_id)
                                .await
                                .map_err(|e| format!("rt.write: {e}"))?
                        }
                    }
                    VolumeWriteTarget::ExternalBound { host_path } => host_path,
                };
                safe_volume_write(&mountpoint, &path, &contents)
                    .await
                    .map_err(|e| format!("rt.write: {e}"))
            })
        })
    }
}

pub(crate) fn make_volume_writer(
    state: &OiState,
) -> Option<Arc<dyn crate::runtime::barrier::VolumeWriter>> {
    Some(Arc::new(OperationVolumeWriter {
        driver: Arc::clone(&state.driver),
        db: state.db.clone(),
    }))
}

// spec: RT#command-execution
/// Adapter from the system actuator's async `exec_command` to the
/// synchronous `Executor` trait expected by the BSL runtime.
struct OperationExecutor {
    container: Arc<dyn crate::system::ContainerRuntime>,
}

impl crate::runtime::barrier::Executor for OperationExecutor {
    fn check_running(&self, target: &crate::runtime::ResourceInstance) -> Result<(), String> {
        let container = Arc::clone(&self.container);
        let name = target.display_name.clone();
        let state = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { container.inspect(&name).await })
        });
        match state {
            Ok(Some(s)) if matches!(s.status, crate::system::types::ContainerStatus::Running) => {
                Ok(())
            }
            Ok(Some(s)) => Err(format!(
                "{} is not running (it is {:?})",
                target.display_name, s.status
            )),
            Ok(None) => Err(format!(
                "{} is not running: no such container",
                target.display_name
            )),
            Err(e) => Err(format!(
                "cannot tell whether {} is running: {e}",
                target.display_name
            )),
        }
    }

    fn exec(
        &self,
        target: &crate::runtime::ResourceInstance,
        argv: &[String],
        extra_env: &[(String, String)],
    ) -> Result<i32, String> {
        let container = Arc::clone(&self.container);
        let name = target.display_name.clone();
        let argv: Vec<String> = argv.to_vec();
        let extra_env: Vec<(String, String)> = extra_env.to_vec();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { container.exec_command(&name, &argv, &extra_env).await })
        });
        let output = result.map_err(|e| e.to_string())?;
        // spec: RT#command-execution
        // What the command wrote goes with the container's logs, not to the
        // script and not to the daemon's own.
        crate::system::journal::record_exec_output(target, &output.stdout, &output.stderr);
        Ok(output.exit_code)
    }
}

pub(crate) fn make_executor(state: &OiState) -> Option<Arc<dyn crate::runtime::barrier::Executor>> {
    Some(Arc::new(OperationExecutor {
        container: Arc::clone(&state.driver.container),
    }))
}

/// The params an operation starts with.
pub enum OperationParams {
    /// As requested: volume params name site volumes and are bound at dispatch.
    Requested(serde_json::Map<String, serde_json::Value>),
    /// As persisted by an interrupted operation, with the bindings its
    /// `<key>_volume` params name, which are established again as they were.
    // spec: OPS#operation-scoped-volumes
    Replayed {
        params: serde_json::Map<String, serde_json::Value>,
        bindings: crate::runtime::history::OperationVolumeBindings,
    },
}

/// A dispatch's params with its volume params bound, and the bindings made.
pub(crate) struct BoundParams {
    pub params: serde_json::Map<String, serde_json::Value>,
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
    state: &OiState,
    schema: &std::collections::BTreeMap<
        seedling_protocol::names::ParamName,
        crate::defs::param_def::ParamDef,
    >,
    mut params: serde_json::Map<String, serde_json::Value>,
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
    let ast = engine
        .compile(script.text())
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

/// End an action as `end` says: file what it calls for, clear the persisted
/// operation, and report it.
///
/// A gate run ends otherwise, as its outcome says.
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
            let running = target.gate().cloned();
            db.call(move |db| {
                crate::runtime::gates::Exclusions::for_closure(
                    db,
                    &name,
                    &def,
                    &cipher,
                    &remembered,
                    running.as_ref(),
                )
            })
        };
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
        match result {
            OperationResult::Completed => return OperationEnd::Completed,
            OperationResult::Failed(e) => {
                tracing::error!(app = %app_name, operation = %target, "operation failed: {e}");
                return OperationEnd::Failed(e.to_string());
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

/// Flip the app's phase in memory and persist the new row. Never hold the
/// registry write lock across the db call: the schedule ticker holds db then
/// acquires registry.read(), so registry.write() + db.lock() in either order
/// deadlocks with it.
// spec: EVT#event-types
pub fn set_phase_and_persist(
    state: &OiState,
    app_name: &AppName,
    new_phase: AppPhase,
    actor: Option<Arc<seedling_protocol::actor::Actor>>,
) {
    use crate::oi::handler::apps::{extract_persist_fields, persist_app_fields};
    let phase_str = phase_event_name(&new_phase);
    // spec: FWD#forward-lifetime
    // spec: SHL#session-end
    if !matches!(new_phase, AppPhase::Installed) {
        state.end_sessions_of_app(app_name.as_str());
    }
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(app_name.as_str()) {
            *entry.phase.lock() = new_phase;
        }
    }
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(app_name.as_str()) {
            let (app_n, generation_n, installed, uninstalling) = extract_persist_fields(entry);
            if let Err(e) = state.db.call(move |db| {
                persist_app_fields(db, &app_n, generation_n, installed, uninstalling)
            }) {
                tracing::error!(app = %app_name, "persist phase transition: {e}");
            }
        }
    }
    state.event_tx.app_phase_changed(app_name, phase_str, actor);
    state.tick_notify.notify_one();
}

fn phase_event_name(phase: &AppPhase) -> &'static str {
    match phase {
        AppPhase::NotInstalled => "not_installed",
        AppPhase::Installed => "installed",
        AppPhase::Uninstalling => "uninstalling",
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "internal helper grouping all operation state"
)]
pub fn spawn_accepted_operation(
    state: Arc<OiState>,
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

                let signaler = make_container_signaler(&state);
                let writer = make_volume_writer(&state);
                let executor = make_executor(&state);
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
pub(crate) fn complete_and_dequeue(
    state: &OiState,
) -> Option<crate::runtime::scheduler::QueuedOperation> {
    let registry = state.registry.read();
    state
        .scheduler
        .lock()
        .complete_current(|app| registry.get(app.as_str()).map(|e| e.current_generation))
}

/// What came of asking for a lifecycle operation.
pub(crate) enum Admission {
    /// It started at once.
    Started(OperationId),
    /// It is queued behind another app's operation, and persisted.
    Queued(OperationId),
    Rejected(crate::runtime::scheduler::RejectReason),
    /// It was queued and could not be persisted, so it was taken off the queue.
    CannotPersist(seedling_protocol::error::OiError),
}

/// Ask for an operation, and start it if it is accepted at once.
///
/// Every request for an operation goes through here: an operator's action, a
/// schedule's firing, and a gate that is due. The scheduler decides, under its
/// own lock, whether it starts, waits or is refused.
// spec: OPS#concurrency
// spec: TRD#uninstall-admission
#[expect(
    clippy::too_many_arguments,
    reason = "the request is what the scheduler needs to admit it"
)]
pub(crate) fn admit_and_spawn(
    state: &Arc<OiState>,
    app: &AppName,
    target: OperationTarget,
    params: serde_json::Map<String, serde_json::Value>,
    generation: u64,
    trigger: &str,
    phase: &Arc<parking_lot::Mutex<AppPhase>>,
    actor: Option<Arc<seedling_protocol::actor::Actor>>,
) -> Admission {
    use crate::runtime::scheduler::ScheduleResult;
    let (result, operation_id) = {
        let mut sched = state.scheduler.lock();
        let result = sched.admit(
            app,
            &target,
            params.clone(),
            generation,
            trigger,
            phase,
            actor.clone(),
        );
        let operation_id = match &result {
            ScheduleResult::Accepted => sched.active().map(|a| a.operation_id.clone()),
            ScheduleResult::Queued => sched
                .queue_iter()
                .find(|q| q.app == *app)
                .map(|q| q.operation_id.clone()),
            ScheduleResult::Rejected(_) => None,
        };
        (
            result,
            operation_id.unwrap_or_else(|| OperationId(String::new())),
        )
    };
    match result {
        ScheduleResult::Accepted => {
            spawn_accepted_operation(
                Arc::clone(state),
                app.clone(),
                target,
                operation_id.clone(),
                OperationParams::Requested(params),
                generation,
                trigger.to_owned(),
                actor,
            );
            Admission::Started(operation_id)
        }
        ScheduleResult::Queued => match persist_queued(state, &operation_id) {
            Ok(()) => Admission::Queued(operation_id),
            Err(e) => Admission::CannotPersist(e),
        },
        ScheduleResult::Rejected(reason) => Admission::Rejected(reason),
    }
}

/// Persist the request the scheduler has just queued as `operation_id`, so
/// the queue survives a restart, and refuse the request if that fails.
///
/// The request is on the scheduler's queue until this returns, and is taken
/// off it again when it cannot be persisted, so a request that is refused is
/// one that never ran. A request that has already started by the time it is
/// persisted leaves no record behind.
// spec: OPS#concurrency
pub fn persist_queued(
    state: &OiState,
    operation_id: &OperationId,
) -> Result<(), seedling_protocol::error::OiError> {
    let queued = state.scheduler.lock().queued(operation_id).cloned();
    let Some(queued) = queued else {
        return Ok(());
    };
    let cipher = Arc::clone(&state.cipher);
    let saved = state
        .db
        .call(move |db| crate::runtime::history::save_queued_operation(db, &cipher, &queued));
    if let Err(e) = saved {
        state.scheduler.lock().unqueue(operation_id);
        return Err(seedling_protocol::error::OiError::internal(format!(
            "cannot persist the queued operation, so it is refused: {e}"
        )));
    }
    if state.scheduler.lock().queued(operation_id).is_none() {
        let id = operation_id.clone();
        state.db.call(move |db| {
            let _ = crate::runtime::history::clear_queued_operation(db, &id);
        });
    }
    Ok(())
}

/// Restore the persisted queue after the decision to replay an interrupted
/// operation, in the order the requests were made.
///
/// A request whose app is no longer registered is discarded. With nothing in
/// progress the first restored request starts, and the rest queue behind it.
// spec: OPS#concurrency
pub fn restore_queue(
    state: &Arc<OiState>,
    queued: Vec<crate::runtime::scheduler::QueuedOperation>,
) {
    use crate::runtime::scheduler::ScheduleResult;
    for op in queued {
        // spec: OPS#initiating-events
        // A request that starts at once is dispatched now, so it runs against
        // the generation current now; one that queues is resolved again as it
        // leaves the queue.
        let current = state
            .registry
            .read()
            .get(op.app.as_str())
            .map(|entry| entry.current_generation);
        let Some(generation) = current else {
            tracing::warn!(app = %op.app, operation = %op.target, "queued operation names an unregistered app; discarding");
            let id = op.operation_id.clone();
            state.db.call(move |db| {
                let _ = crate::runtime::history::clear_queued_operation(db, &id);
            });
            continue;
        };
        let result = state.scheduler.lock().request_with_id(
            &op.app,
            &op.target,
            op.params.clone(),
            generation,
            &op.trigger,
            op.operation_id.clone(),
            op.actor.clone(),
        );
        match result {
            ScheduleResult::Accepted => {
                spawn_accepted_operation(
                    Arc::clone(state),
                    op.app,
                    op.target,
                    op.operation_id,
                    OperationParams::Requested(op.params),
                    generation,
                    op.trigger,
                    op.actor,
                );
            }
            ScheduleResult::Queued => {}
            ScheduleResult::Rejected(reason) => {
                tracing::warn!(app = %op.app, operation = %op.target, ?reason, "restored queued operation refused; discarding");
                let id = op.operation_id.clone();
                state.db.call(move |db| {
                    let _ = crate::runtime::history::clear_queued_operation(db, &id);
                });
            }
        }
    }
}

/// Persist a scheduled fire the scheduler queued, and stamp its fire time once
/// it is on record. A fire that cannot be persisted is taken off the queue and
/// left unstamped, so the next check fires it again.
// spec: SCHED#fire-times
// spec: OPS#concurrency
pub fn persist_queued_fire(
    state: &OiState,
    fire: &crate::runtime::schedules::FiredSchedule,
    now: jiff::Timestamp,
) {
    let Some(operation_id) = &fire.operation_id else {
        return;
    };
    match persist_queued(state, operation_id) {
        Ok(()) => {
            let (app, action, cron) =
                (fire.app.clone(), fire.action.clone(), fire.cronexpr.clone());
            state.db.call(move |db| {
                if let Err(e) =
                    crate::runtime::db::upsert_schedule_fired(db, &app, &action, &cron, &now.to_string())
                {
                    tracing::error!(app = %app, action = %action, "failed to stamp a queued fire: {e}");
                }
            });
        }
        Err(e) => {
            tracing::error!(app = %fire.app, action = %fire.action, "scheduled fire refused: {}", e.message);
        }
    }
}

#[cfg(test)]
mod tests;
