use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

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
}

impl crate::runtime::barrier::VolumeWriter for OperationVolumeWriter {
    fn write(
        &self,
        app: &str,
        target: crate::runtime::barrier::VolumeWriteTarget,
        path: &str,
        contents: &[u8],
    ) -> Result<(), String> {
        use crate::runtime::barrier::VolumeWriteTarget;
        use crate::runtime::identity::VolumeName;
        use crate::system::actuator::{safe_volume_write, tmpfs_volume_path};
        use std::path::PathBuf;

        let driver = Arc::clone(&self.driver);
        let app = app.to_owned();
        let path = path.to_owned();
        let contents = contents.to_vec();

        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let mountpoint: PathBuf = match target {
                    VolumeWriteTarget::NamedVolume { name, tmpfs } => {
                        let vol_name = VolumeName::for_app(&app, &name);
                        if tmpfs {
                            // spec: ACTU#volume-storage
                            tmpfs_volume_path(vol_name.as_str())
                        } else {
                            driver.volume_store.path(&vol_name)
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

fn make_volume_writer(state: &OiState) -> Option<Arc<dyn crate::runtime::barrier::VolumeWriter>> {
    Some(Arc::new(OperationVolumeWriter {
        driver: Arc::clone(&state.driver),
    }))
}

// spec: RT#command-execution
/// Adapter from the system actuator's async `exec_command` to the
/// synchronous `Executor` trait expected by the BSL runtime.
struct OperationExecutor {
    container: Arc<dyn crate::system::ContainerRuntime>,
}

impl crate::runtime::barrier::Executor for OperationExecutor {
    fn exec(
        &self,
        name: &str,
        argv: &[String],
        extra_env: &[(String, String)],
    ) -> Result<i32, String> {
        let container = Arc::clone(&self.container);
        let name = name.to_owned();
        let argv: Vec<String> = argv.to_vec();
        let extra_env: Vec<(String, String)> = extra_env.to_vec();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { container.exec_command(&name, &argv, &extra_env).await })
        });
        result.map_err(|e| e.to_string())
    }
}

fn make_executor(state: &OiState) -> Option<Arc<dyn crate::runtime::barrier::Executor>> {
    Some(Arc::new(OperationExecutor {
        container: Arc::clone(&state.driver.container),
    }))
}

// spec: ACN#volume-params
/// For every `kind: "volume"` param in the action's schema, look up the
/// operator-supplied site volume name, build an
/// [`OperationVolumeBinding`] keyed by a generated per-operation name, and
/// rewrite the params map so the action closure sees `<key>_volume = <generated-name>`
/// — consumed by `app.external_volume(param[...])`.
///
/// Volumes that have already been validated to exist by `invoke_action`'s
/// pre-flight; here we only resolve to the host path and warn (instead of
/// failing) if a volume disappeared between validation and dispatch — the
/// closure will then see a missing `<key>_volume` and can fail gracefully.
fn resolve_action_volume_params(
    state: &OiState,
    app: &App,
    action_name: &ActionName,
    mut params: serde_json::Map<String, serde_json::Value>,
    operation_id: &str,
) -> (
    serde_json::Map<String, serde_json::Value>,
    HashMap<String, OperationVolumeBinding>,
) {
    use crate::defs::install::ParamKind;
    use crate::defs::volume::{VolumeParamSpec, build_operation_volume_params};
    use crate::runtime::site_volumes;
    use seedling_protocol::names::SiteVolumeName;

    let def = app.def.load();
    let schema = match def.actions.get(action_name.as_str()) {
        Some(a) => a.params.clone(),
        None => return (params, HashMap::new()),
    };
    drop(def);

    let mut specs: Vec<(String, VolumeParamSpec)> = Vec::new();
    let volume_keys: Vec<String> = schema
        .iter()
        .filter(|(_, p)| p.kind == ParamKind::Volume)
        .map(|(name, _)| name.as_str().to_owned())
        .collect();
    for key in &volume_keys {
        let val = match params.remove(key.as_str()) {
            Some(serde_json::Value::String(s)) => s,
            other => {
                if other.is_some() {
                    tracing::warn!(
                        action = %action_name,
                        key = %key,
                        "volume param value is not a string; skipping binding",
                    );
                }
                continue;
            }
        };
        let site_name = match SiteVolumeName::new(&val) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(
                    action = %action_name,
                    key = %key,
                    value = %val,
                    "invalid site volume name; skipping binding: {e}",
                );
                continue;
            }
        };
        let lookup_name = site_name.clone();
        let resolved = state
            .db
            .call(move |db| site_volumes::get(db, &lookup_name).ok().flatten());
        let Some(site_vol) = resolved else {
            tracing::warn!(
                action = %action_name,
                key = %key,
                value = %val,
                "site volume not found at dispatch; skipping binding",
            );
            continue;
        };
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
        return (params, HashMap::new());
    }
    let (bindings, injected) = build_operation_volume_params(operation_id, specs);
    for (k, v) in injected {
        params.insert(k, v);
    }
    (params, bindings)
}

/// Run the operation loop synchronously until completion or failure.
#[expect(
    clippy::too_many_arguments,
    reason = "operation state is inherently multi-dimensional"
)]
fn run_operation_loop(
    app: &App,
    db: DbHandle,
    params: serde_json::Map<String, serde_json::Value>,
    active_progress: Arc<RwLock<Option<OperationProgress>>>,
    tick_notify: Arc<Notify>,
    op_ctx: &OperationEventCtx,
    script_limits: &crate::ScriptLimits,
    cipher: Arc<crate::runtime::secrets::Cipher>,
    operation_volume_bindings: HashMap<String, OperationVolumeBinding>,
    cancel_token: Arc<CancelToken>,
    container_signaler: Option<Arc<dyn crate::runtime::barrier::ContainerSignaler>>,
    volume_writer: Option<Arc<dyn crate::runtime::barrier::VolumeWriter>>,
    executor: Option<Arc<dyn crate::runtime::barrier::Executor>>,
) -> bool {
    let app_name = &op_ctx.app;
    let action_name = &op_ctx.action_name;
    let operation_id = OperationId(op_ctx.operation_id.clone());

    let (engine, mut scope, _) = crate::setup_language(script_limits, Arc::clone(&app.bundle));
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
    let cipher_for_persist = Arc::clone(&cipher);
    if let Err(e) = db.call(move |db| {
        save_current_operation(db, &cipher_for_persist, &record, &params_for_persist)
    }) {
        tracing::warn!(
            app = %app_name,
            action = %action_name,
            "failed to persist current_operation: {e}"
        );
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
                });
                op_ctx.failed(&desc);
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

/// Piecewise backoff: 2s while waited < 2 min, then ramp to 30s at 1 hour,
/// then ramp to 300s at 6 hours, then cap at 300s.
// spec: SUSP#re-evaluation-cadence
pub(crate) fn dynamic_poll_interval(waited_secs: u64) -> Duration {
    const LOW: u64 = 2;
    const MID: u64 = 30;
    const HIGH: u64 = 300;
    const T_LOW_END: u64 = 120; // 2 min
    const T_MID_END: u64 = 3600; // 1 hour
    const T_HIGH_END: u64 = 21600; // 6 hours

    let poll = if waited_secs <= T_LOW_END {
        LOW
    } else if waited_secs <= T_MID_END {
        let ratio = (waited_secs - T_LOW_END) as f64 / (T_MID_END - T_LOW_END) as f64;
        LOW + ((MID - LOW) as f64 * ratio) as u64
    } else if waited_secs <= T_HIGH_END {
        let ratio = (waited_secs - T_MID_END) as f64 / (T_HIGH_END - T_MID_END) as f64;
        MID + ((HIGH - MID) as f64 * ratio) as u64
    } else {
        HIGH
    };
    Duration::from_secs(poll)
}

async fn cleanup_dynamic_resources(
    state: &OiState,
    operation_id_str: &str,
    active_progress: &RwLock<Option<OperationProgress>>,
    tick_notify: &Notify,
) {
    use crate::defs::deployment::Deployment;
    use crate::defs::job::Job;
    use crate::defs::resource::{Resource, ResourceKind};
    use crate::runtime::LifecycleState;
    use crate::runtime::barrier::oracle::derive_lifecycle_state;
    use crate::runtime::desired::{delete_dynamic_resources_for_operation, list_dynamic_resources};
    use crate::runtime::history::query_observations;
    use crate::runtime::identity::{InstanceId, InstanceVariant, ResourceInstance};

    let op_id_for_filter = operation_id_str.to_owned();
    let dynamic_records: Vec<_> = state
        .db
        .call(move |db| list_dynamic_resources(db).unwrap_or_default())
        .into_iter()
        .filter(|r| r.operation_id == op_id_for_filter)
        .collect();

    if !dynamic_records.is_empty() {
        let mut cleanup = OperationProgress::new();

        for record in &dynamic_records {
            let uuid = match uuid::Uuid::parse_str(&record.instance_id) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(
                        instance_id = %record.instance_id,
                        "dynamic cleanup: bad instance_id: {e}"
                    );
                    continue;
                }
            };

            let kind = match record.kind.as_str() {
                "Deployment" => ResourceKind::Deployment,
                "Job" => ResourceKind::Job,
                _ => continue,
            };

            let instance = ResourceInstance {
                id: InstanceId(uuid),
                app: record.app.clone(),
                kind,
                name: None,
                variant: InstanceVariant::Singleton,
                display_name: record.display_name.clone(),
            };

            let minimal = match kind {
                ResourceKind::Deployment => Resource::Deployment(Deployment {
                    name: std::sync::Arc::new(String::new()),
                    def: Default::default(),
                    frozen: false,
                }),
                ResourceKind::Job => Resource::Job(Job {
                    name: std::sync::Arc::new(String::new()),
                    def: Default::default(),
                    frozen: false,
                }),
                _ => unreachable!(),
            };

            cleanup.stopped(instance.clone());
            cleanup.dynamic_defs.insert(instance, minimal);
        }

        if !cleanup.is_empty() {
            *active_progress.write() = Some(cleanup);
            tick_notify.notify_one();

            let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    tracing::warn!(
                        operation_id = %operation_id_str,
                        "dynamic resource cleanup timed out"
                    );
                    break;
                }

                // Collect the instances the reconciler is tearing down. Both
                // checks below run against this snapshot so we don't hold
                // the active_progress lock across .await.
                let cleanup_instances: Vec<ResourceInstance> = {
                    let guard = active_progress.read();
                    guard
                        .as_ref()
                        .map(|p| p.dynamic_defs.keys().cloned().collect())
                        .unwrap_or_default()
                };

                let lifecycles_terminal = cleanup_instances.iter().all(|inst| {
                    let inst = inst.clone();
                    state.db.call(move |db| {
                        let obs = query_observations(db, &inst).unwrap_or_default();
                        derive_lifecycle_state(&inst, &obs).has_reached(LifecycleState::Terminated)
                    })
                });

                // A container with `podman --rm` can reach the Unscheduled
                // lifecycle state via `container_removed` before the
                // reconciler has had a chance to remove the pod network.
                // Breaking out of cleanup at that point leaks the network
                // and the next Job on the same app trips over "subnet
                // already used". Require every pod network to also be gone.
                let mut networks_all_gone = true;
                for inst in &cleanup_instances {
                    let net_name = format!("seedling-{}", inst.display_name);
                    match state.driver.container.network_exists(&net_name).await {
                        Ok(false) => {}
                        Ok(true) => {
                            networks_all_gone = false;
                            break;
                        }
                        Err(e) => {
                            tracing::warn!(
                                instance = %inst.display_name,
                                "network_exists check failed during cleanup; assuming present: {e}"
                            );
                            networks_all_gone = false;
                            break;
                        }
                    }
                }

                if lifecycles_terminal && networks_all_gone {
                    break;
                }

                tokio::time::sleep(Duration::from_secs(2)).await;
                tick_notify.notify_one();
            }
        }
    }

    // spec: FLT#workload-faults
    // Clear per-instance faults for every dynamic resource that we just
    // tore down. Without this, faults filed against short-lived Job
    // instances (image_pull_failed, container_start_failed, …) persist
    // forever because the instance id they reference has gone away.
    let instance_ids_to_clear: Vec<(AppName, String)> = dynamic_records
        .iter()
        .map(|r| (r.app.clone(), r.instance_id.clone()))
        .collect();
    let op_id_owned = operation_id_str.to_owned();
    state.db.call(move |db| {
        for (app, instance_id) in &instance_ids_to_clear {
            if let Err(e) = faults::clear_faults_for_instance(db, app, instance_id) {
                tracing::warn!(
                    app = %app,
                    instance_id = %instance_id,
                    "failed to clear per-instance faults during dynamic cleanup: {e}"
                );
            }
        }
        if let Err(e) = delete_dynamic_resources_for_operation(db, &op_id_owned) {
            tracing::error!(
                operation_id = %op_id_owned,
                "failed to delete dynamic resource records: {e}"
            );
        }
    });
}

/// Flip the app's phase in memory and persist the new row. Never hold the
/// registry write lock across the db call: the schedule ticker holds db then
/// acquires registry.read(), so registry.write() + db.lock() in either order
/// deadlocks with it.
// spec: EVT#event-types
pub fn set_phase_and_persist(state: &OiState, app_name: &AppName, new_phase: AppPhase) {
    use crate::oi::handler::apps::{extract_persist_fields, persist_app_fields};
    let phase_str = phase_event_name(&new_phase);
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(app_name.as_str()) {
            *entry.phase.lock() = new_phase;
        }
    }
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(app_name.as_str()) {
            let (app_n, generation_n, installed, uninstalling, installing) =
                extract_persist_fields(entry);
            if let Err(e) = state.db.call(move |db| {
                persist_app_fields(
                    db,
                    &app_n,
                    generation_n,
                    installed,
                    uninstalling,
                    installing,
                )
            }) {
                tracing::error!(app = %app_name, "persist phase transition: {e}");
            }
        }
    }
    state.event_tx.app_phase_changed(app_name, phase_str, None);
    state.tick_notify.notify_one();
}

fn phase_event_name(phase: &AppPhase) -> &'static str {
    match phase {
        AppPhase::NotInstalled => "not_installed",
        AppPhase::Installing => "installing",
        AppPhase::Installed => "installed",
        AppPhase::Uninstalling => "uninstalling",
    }
}

// spec: ACI#install
fn enter_installing_phase(state: &OiState, app_name: &AppName) {
    set_phase_and_persist(state, app_name, AppPhase::Installing);
    tracing::info!(app = %app_name, "install started; entering Installing phase");
}

/// Place the app's just-completed install after every install completed
/// before it, durably and then in memory.
// spec: ING#claims-across-apps
pub(crate) fn record_install_completed(state: &OiState, app_name: &AppName) {
    let name = app_name.clone();
    match state
        .db
        .call(move |db| crate::runtime::apps::AppRegistry::record_install_completed(db, &name))
    {
        Ok(seq) => {
            if let Some(entry) = state.registry.write().get_mut(app_name.as_str()) {
                entry.install_seq = Some(seq);
            }
        }
        Err(e) => tracing::error!(app = %app_name, "record install completion order: {e}"),
    }
}

// spec: ACI#install-completion
fn finalize_install(state: &OiState, app_name: &AppName) {
    record_install_completed(state, app_name);
    set_phase_and_persist(state, app_name, AppPhase::Installed);
    tracing::info!(app = %app_name, "install completed; app is now installed");
}

// spec: ACI#install-completion
fn revert_install_phase(state: &OiState, app_name: &AppName) {
    set_phase_and_persist(state, app_name, AppPhase::NotInstalled);
    tracing::info!(app = %app_name, "install failed; reverted to NotInstalled");
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
    params: serde_json::Map<String, serde_json::Value>,
    source_generation: u64,
    target_generation: u64,
    trigger: String,
    actor: Option<std::sync::Arc<seedling_protocol::actor::Actor>>,
) {
    let (app, active_progress, tick_notify, start_pending) = {
        let reg = state.registry.read();
        match reg.get(app_name.as_str()) {
            Some(e) => (
                e.app.clone(),
                Arc::clone(&e.active_progress),
                Arc::clone(&e.tick_notify),
                Arc::clone(&e.start_pending),
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
    let is_install = action_name == "install";
    let is_start = action_name == "start";

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

        // spec: ACI#install
        // Flip to Installing as soon as the install operation actually starts
        // (covers both directly-accepted and later-dequeued installs because
        // both paths funnel through spawn_accepted_operation).
        if is_install {
            enter_installing_phase(&state, &app_name);
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
        let (params, operation_volume_bindings) =
            resolve_action_volume_params(&state, &app, &action_name, params, &operation_id.0);

        let success = {
            let app_name = app_name.clone();
            let active_progress = Arc::clone(&active_progress);
            let tick_notify = Arc::clone(&tick_notify);
            let params = params.clone();
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
                    params,
                    active_progress,
                    tick_notify,
                    &op_ctx,
                    &script_limits,
                    cipher,
                    operation_volume_bindings,
                    cancel_token,
                    signaler,
                    writer,
                    executor,
                )
            })
            .await
            .unwrap_or(false)
        };

        cleanup_dynamic_resources(&state, &operation_id_str, &active_progress, &tick_notify).await;

        // spec: RCN#desired-state
        // The start action has ended, however it ended. Released while the
        // operation's progress still stands, so the desired state passes from
        // what the closure placed there to the whole resource graph without a
        // moment of holding nothing.
        if is_start {
            start_pending.store(false, std::sync::atomic::Ordering::Release);
        }
        *active_progress.write() = None;
        tick_notify.notify_one();

        // spec: ACI#install-completion
        if is_install {
            if success {
                finalize_install(&state, &app_name);
            } else {
                revert_install_phase(&state, &app_name);
            }
        }

        let next = state.scheduler.lock().complete_current();
        if let Some(queued) = next {
            spawn_accepted_operation(
                Arc::clone(&state),
                queued.app,
                queued.action,
                queued.operation_id,
                queued.params,
                queued.source_generation,
                queued.target_generation,
                queued.trigger,
                queued.actor,
            );
        }

        // spec: OPS#initiating-events
        // The install has finished and its slot is free, so the start action
        // can be admitted: before this, the install itself held the app's.
        if is_install && success {
            initiate_start(&state, &app_name, "install");
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
    actor: Option<Arc<seedling_protocol::actor::Actor>>,
) -> Result<(), seedling_protocol::error::OiError> {
    let queued = {
        let mut sched = state.scheduler.lock();
        sched.set_queued_actor(operation_id, actor);
        sched.queued(operation_id).cloned()
    };
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
        let app = {
            let reg = state.registry.read();
            reg.get(op.app.as_str())
                .map(|e| Arc::clone(&e.start_pending))
        };
        let Some(start_pending) = app else {
            tracing::warn!(app = %op.app, action = %op.action, "queued operation names an unregistered app; discarding");
            let id = op.operation_id.clone();
            state.db.call(move |db| {
                let _ = crate::runtime::history::clear_queued_operation(db, &id);
            });
            continue;
        };
        let result = state.scheduler.lock().request_with_id(
            &op.app,
            &op.action,
            op.params.clone(),
            op.source_generation,
            op.target_generation,
            &op.trigger,
            op.operation_id.clone(),
        );
        match result {
            ScheduleResult::Accepted => {
                if op.action == "start" {
                    start_pending.store(true, std::sync::atomic::Ordering::Release);
                }
                spawn_accepted_operation(
                    Arc::clone(state),
                    op.app,
                    op.action,
                    op.operation_id,
                    op.params,
                    op.source_generation,
                    op.target_generation,
                    op.trigger,
                    op.actor,
                );
            }
            ScheduleResult::Queued => {
                if op.action == "start" {
                    start_pending.store(true, std::sync::atomic::Ordering::Release);
                }
                state
                    .scheduler
                    .lock()
                    .set_queued_actor(&op.operation_id, op.actor);
            }
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

/// Initiate `app_name`'s start action with `trigger`, holding the app's
/// desired state from now until the start action ends.
///
/// The start action runs as any lifecycle operation does: at once if nothing
/// else is in progress, or queued behind another app's.
// spec: OPS#initiating-events
// spec: ACN#start-action
// spec: RCN#desired-state
pub fn initiate_start(state: &Arc<OiState>, app_name: &AppName, trigger: &str) {
    let (generation, phase_handle, start_pending) = {
        let reg = state.registry.read();
        let Some(entry) = reg.get(app_name.as_str()) else {
            return;
        };
        (
            entry.current_generation,
            Arc::clone(&entry.phase),
            Arc::clone(&entry.start_pending),
        )
    };
    start_pending.store(true, std::sync::atomic::Ordering::Release);
    let start = ActionName::new_unchecked("start");
    let (result, op_id) = {
        let mut sched = state.scheduler.lock();
        let result = sched.admit(
            app_name,
            &start,
            serde_json::Map::new(),
            generation,
            generation,
            trigger,
            &phase_handle,
        );
        let op_id = match &result {
            crate::runtime::scheduler::ScheduleResult::Accepted => {
                sched.active().map(|a| a.operation_id.clone())
            }
            crate::runtime::scheduler::ScheduleResult::Queued => sched
                .queue_iter()
                .find(|q| q.app == *app_name)
                .map(|q| q.operation_id.clone()),
            crate::runtime::scheduler::ScheduleResult::Rejected(_) => None,
        };
        (result, op_id)
    };
    match (result, op_id) {
        (crate::runtime::scheduler::ScheduleResult::Accepted, Some(operation_id)) => {
            spawn_accepted_operation(
                Arc::clone(state),
                app_name.clone(),
                start,
                operation_id,
                serde_json::Map::new(),
                generation,
                generation,
                trigger.to_owned(),
                None,
            )
        }
        (crate::runtime::scheduler::ScheduleResult::Queued, Some(operation_id)) => {
            match persist_queued(state, &operation_id, None) {
                Ok(()) => tracing::info!(app = %app_name, trigger, "start action queued"),
                Err(e) => {
                    tracing::error!(app = %app_name, trigger, "start action refused: {}", e.message);
                    start_pending.store(false, std::sync::atomic::Ordering::Release);
                }
            }
        }
        (other, _) => {
            tracing::warn!(app = %app_name, trigger, "start action not admitted: {other:?}");
            start_pending.store(false, std::sync::atomic::Ordering::Release);
        }
    }
}
