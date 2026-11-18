use std::{collections::HashMap, path::Path, sync::Arc};

use parking_lot::RwLock;
use seedling_protocol::events::OperationEventCtx;
use seedling_protocol::names::{ActionName, AppName};
use tokio::sync::Notify;

use crate::{
    defs::{app::App, volume::OperationVolumeBinding},
    oi::state::OiState,
    runtime::{
        AppPhase, InstanceRegistry,
        barrier::{
            CancelToken, OperationId,
            oracle::DbWorldOracle,
            replay::{DbActionLog, OperationContext, OperationResult, run_operation},
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

fn make_container_signaler(
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

fn make_executor(state: &OiState) -> Option<Arc<dyn crate::runtime::barrier::Executor>> {
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

/// End an operation that was accepted but cannot run, as a failed one ends.
fn refuse_to_run(db: &DbHandle, op_ctx: &OperationEventCtx, description: String) {
    let app_name = op_ctx.app.clone();
    let operation_id = op_ctx.operation_id.clone();
    let for_db = description.clone();
    db.call(move |db| {
        let _ = faults::file_fault(db, &app_name, None, None, None, "operation_failed", &for_db);
        let _ = clear_current_operation(db);
        let _ = crate::runtime::generations::record_operation_outcome(
            db,
            &operation_id,
            crate::runtime::generations::Outcome::Failed,
            Some(&for_db),
        );
    });
    op_ctx.failed(&description);
}

/// Run the operation loop synchronously until completion or failure.
#[expect(
    clippy::too_many_arguments,
    reason = "operation state is inherently multi-dimensional"
)]
fn run_operation_loop(
    app: &App,
    db: DbHandle,
    bound: Result<BoundParams, String>,
    active_progress: Arc<RwLock<Option<OperationProgress>>>,
    tick_notify: Arc<Notify>,
    op_ctx: &OperationEventCtx,
    script_limits: &crate::ScriptLimits,
    cipher: Arc<crate::runtime::secrets::Cipher>,
    cancel_token: Arc<CancelToken>,
    container_signaler: Option<Arc<dyn crate::runtime::barrier::ContainerSignaler>>,
    volume_writer: Option<Arc<dyn crate::runtime::barrier::VolumeWriter>>,
    executor: Option<Arc<dyn crate::runtime::barrier::Executor>>,
) -> bool {
    let app_name = &op_ctx.app;
    let action_name = &op_ctx.action_name;
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
            refuse_to_run(
                &db,
                op_ctx,
                format!("{action_name} not run: a volume param could not be bound: {why}"),
            );
            return false;
        }
    };

    // spec: OPS#initiating-events
    // The operation runs against the definition and parameters of its target
    // generation, however many generations have been committed since it was
    // dispatched, and on every replay. If that generation cannot be rebuilt
    // the operation fails; it is never run against another's state.
    let target_app;
    let app = {
        let name = app_name.clone();
        let current = db.call(move |db| crate::runtime::generations::current(db, &name));
        match current {
            Ok(Some(current))
                if op_ctx.target_generation != 0 && current != op_ctx.target_generation =>
            {
                let name = app_name.clone();
                let (limits, cipher, target) = (
                    script_limits.clone(),
                    Arc::clone(&cipher),
                    op_ctx.target_generation,
                );
                match db.call(move |db| {
                    crate::runtime::generations::reconstruct_app_def(
                        db, &name, target, &limits, &cipher,
                    )
                }) {
                    Ok(rebuilt) => {
                        target_app = rebuilt;
                        &target_app
                    }
                    Err(e) => {
                        tracing::error!(app = %app_name, action = %action_name, "cannot rebuild generation {}: {e}", op_ctx.target_generation);
                        refuse_to_run(
                            &db,
                            op_ctx,
                            format!(
                                "{action_name} not run: generation {} cannot be rebuilt: {e}",
                                op_ctx.target_generation
                            ),
                        );
                        return false;
                    }
                }
            }
            _ => app,
        }
    };

    let (engine, mut scope, _) = match crate::setup_language(script_limits, Arc::clone(&app.bundle))
    {
        Ok(language) => language,
        // spec: CONST#host-constants
        Err(e) => {
            tracing::error!(app = %app_name, action = %action_name, "{action_name} not run: {e}");
            refuse_to_run(&db, op_ctx, format!("{action_name} not run: {e}"));
            return false;
        }
    };
    // spec: BND#script-formation
    let compiled = app
        .bundle
        .script()
        .map_err(|e| e.to_string())
        .and_then(|script| {
            engine
                .compile(script.text())
                .map_err(|e| script.remap_error(&e.to_string()))
        });
    let ast = match compiled {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(app = %app_name, action = %action_name, "script compile error: {e}");
            let app_name_owned = app_name.clone();
            let desc = format!("script compile error in {action_name}: {e}");
            db.call(move |db| {
                let _ = faults::file_fault(
                    db,
                    &app_name_owned,
                    None,
                    None,
                    None,
                    "operation_failed",
                    &desc,
                );
            });
            return false;
        }
    };

    let log = DbActionLog::new(
        db.clone(),
        operation_id.clone(),
        app_name.clone(),
        action_name.clone(),
    );
    let world = Arc::new(DbWorldOracle::new(db.clone()));
    let registry: Arc<dyn InstanceRegistry> = Arc::new(DbInstanceRegistry::new(db.clone()));

    // spec: OPS#initiating-events, SUSP#replay, OPS#persistence
    // Persist the current operation so a runtime restart can resume it.
    // Params are encrypted because they may carry secret values.
    let record = CurrentOperation {
        operation_id: operation_id.clone(),
        app: app_name.clone(),
        action_name: action_name.clone(),
        source_generation: op_ctx.source_generation,
        target_generation: op_ctx.target_generation,
    };
    let params_for_persist = params.clone();
    let bindings_for_persist = operation_volume_bindings.clone();
    let cipher_for_persist = Arc::clone(&cipher);
    // spec: OPS#persistence
    // The closure does not run unless the operation is on record: a restart
    // could not otherwise replay it, and its params could not be recovered.
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
        refuse_to_run(
            &db,
            op_ctx,
            format!("{action_name} not run: the operation could not be persisted: {e}"),
        );
        return false;
    }

    loop {
        let result = run_operation(
            OperationContext {
                engine: &engine,
                script_ast: &ast,
                operation_id: operation_id.clone(),
                app,
                action_name: action_name.as_str(),
                log: &log,
                world: Arc::clone(&world),
                registry: Arc::clone(&registry),
                active_progress: Some(Arc::clone(&active_progress)),
                tick_notify: Some(Arc::clone(&tick_notify)),
                params: params.clone(),
                is_shell: false,
                db: Some(db.clone()),
                source_generation: op_ctx.source_generation,
                target_generation: op_ctx.target_generation,
                script_limits: Some(script_limits.clone()),
                cipher: Some(Arc::clone(&cipher)),
                operation_volume_bindings: operation_volume_bindings.clone(),
                cancel_token: Arc::clone(&cancel_token),
                container_signaler: container_signaler.clone(),
                volume_writer: volume_writer.clone(),
                executor: executor.clone(),
            },
            &mut scope,
        );
        let operation_id_for_outcome = operation_id.0.clone();
        match result {
            OperationResult::Completed => {
                let app_name_owned = app_name.clone();
                db.call(move |db| {
                    // Clear stale "the previous attempt didn't run to
                    // completion" faults for this app: a fresh successful
                    // operation supersedes both prior `operation_failed`
                    // (a previous attempt errored out) and
                    // `operation_cancelled` (a previous attempt was
                    // operator-cancelled). Without this, a successful
                    // retry leaves the app stuck in `Faulted` even though
                    // its current state is fine.
                    for kind in ["operation_failed", "operation_cancelled"] {
                        faults::clear_faults_by_kind(db, &app_name_owned, kind).ok();
                    }
                    let _ = clear_current_operation(db);
                    // spec: GEN#generation-numbers
                    let _ = crate::runtime::generations::record_operation_outcome(
                        db,
                        &operation_id_for_outcome,
                        crate::runtime::generations::Outcome::Succeeded,
                        None,
                    );
                });
                op_ctx.completed();
                return true;
            }
            OperationResult::Failed(e) => {
                tracing::error!(app = %app_name, action = %action_name, "operation failed: {e}");
                let app_name_owned = app_name.clone();
                let desc = format!("{action_name} failed: {e}");
                db.call(move |db| {
                    let _ = faults::file_fault(
                        db,
                        &app_name_owned,
                        None,
                        None,
                        None,
                        "operation_failed",
                        &desc,
                    );
                    let _ = clear_current_operation(db);
                    let _ = crate::runtime::generations::record_operation_outcome(
                        db,
                        &operation_id_for_outcome,
                        crate::runtime::generations::Outcome::Failed,
                        Some(&desc),
                    );
                });
                op_ctx.failed(&e.to_string());
                return false;
            }
            // spec: OPS#cancellation
            OperationResult::Cancelled => {
                tracing::warn!(app = %app_name, action = %action_name, "operation cancelled");
                let app_name_owned = app_name.clone();
                let desc = format!("{action_name} cancelled by operator");
                let desc_for_fault = desc.clone();
                db.call(move |db| {
                    let _ = faults::file_fault(
                        db,
                        &app_name_owned,
                        None,
                        None,
                        None,
                        "operation_cancelled",
                        &desc_for_fault,
                    );
                    let _ = clear_current_operation(db);
                    let _ = crate::runtime::generations::record_operation_outcome(
                        db,
                        &operation_id_for_outcome,
                        crate::runtime::generations::Outcome::Cancelled,
                        None,
                    );
                });
                // spec: OPS#cancellation
                // A cancelled operation ends in an outcome of its own.
                op_ctx.cancelled();
                return false;
            }
            OperationResult::Suspended(cond) => {
                tick_notify.notify_one();
                let waited = earliest_unsatisfied_barrier_wait_secs(&log);
                // Surface long-running barriers so operators can tell a
                // legitimately-long wait from a stuck one.
                if waited > 0 && waited.is_multiple_of(600) {
                    tracing::info!(
                        app = %app_name,
                        action = %action_name,
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
    action_name: ActionName,
    operation_id: OperationId,
    params: OperationParams,
    source_generation: u64,
    target_generation: u64,
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
            action_name.clone(),
            &operation_id.0,
            source_generation,
            target_generation,
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
        let schema = app
            .def
            .load()
            .actions
            .get(action_name.as_str())
            .map(|a| a.params.clone())
            .unwrap_or_default();
        let bound = match params {
            OperationParams::Requested(params) => {
                bind_volume_params(&state, &schema, params, &operation_id.0)
            }
            // spec: OPS#operation-scoped-volumes
            OperationParams::Replayed { params, bindings } => Ok(BoundParams { params, bindings }),
        };

        let ran = {
            let app_name = app_name.clone();
            let active_progress = Arc::clone(&active_progress);
            let tick_notify = Arc::clone(&tick_notify);
            let op_ctx = op_ctx.clone();
            let cipher = Arc::clone(&cipher);
            let cancel_token = Arc::clone(&cancel_token);

            let signaler = make_container_signaler(&state);
            let writer = make_volume_writer(&state);
            let executor = make_executor(&state);
            tokio::task::spawn_blocking(move || {
                let db = match open_operation_dbs(&db_path, &app_name) {
                    Some(d) => d,
                    None => return false,
                };
                run_operation_loop(
                    &app,
                    db,
                    bound,
                    active_progress,
                    tick_notify,
                    &op_ctx,
                    &script_limits,
                    cipher,
                    cancel_token,
                    signaler,
                    writer,
                    executor,
                )
            })
            .await
        };
        if let Err(e) = ran {
            tracing::error!(app = %app_name, action = %action_name, "operation task failed: {e}");
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

        let next = state.scheduler.lock().complete_current();
        if let Some(queued) = next {
            spawn_accepted_operation(
                Arc::clone(&state),
                queued.app,
                queued.action,
                queued.operation_id,
                OperationParams::Requested(queued.params),
                queued.source_generation,
                queued.target_generation,
                queued.trigger,
                queued.actor,
            );
        }
    });
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
        let registered = state.registry.read().get(op.app.as_str()).is_some();
        if !registered {
            tracing::warn!(app = %op.app, action = %op.action, "queued operation names an unregistered app; discarding");
            let id = op.operation_id.clone();
            state.db.call(move |db| {
                let _ = crate::runtime::history::clear_queued_operation(db, &id);
            });
            continue;
        }
        let result = state.scheduler.lock().request_with_id(
            &op.app,
            &op.action,
            op.params.clone(),
            op.source_generation,
            op.target_generation,
            &op.trigger,
            op.operation_id.clone(),
            op.actor.clone(),
        );
        match result {
            ScheduleResult::Accepted => {
                spawn_accepted_operation(
                    Arc::clone(state),
                    op.app,
                    op.action,
                    op.operation_id,
                    OperationParams::Requested(op.params),
                    op.source_generation,
                    op.target_generation,
                    op.trigger,
                    op.actor,
                );
            }
            ScheduleResult::Queued => {}
            ScheduleResult::Rejected(reason) => {
                tracing::warn!(app = %op.app, action = %op.action, ?reason, "restored queued operation refused; discarding");
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
