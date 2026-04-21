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

/// Run the operation loop synchronously until completion or failure.
#[expect(
    clippy::too_many_arguments,
    reason = "operation state is inherently multi-dimensional"
)]
fn run_operation_loop(
    app: &App,
    script: &str,
    db: DbHandle,
    params: serde_json::Map<String, serde_json::Value>,
    active_progress: Arc<RwLock<Option<OperationProgress>>>,
    tick_notify: Arc<Notify>,
    op_ctx: &OperationEventCtx,
    script_limits: &crate::ScriptLimits,
    cipher: Arc<crate::runtime::secrets::Cipher>,
    operation_volume_bindings: HashMap<String, OperationVolumeBinding>,
    persist_for_replay: bool,
    cancel_token: Arc<CancelToken>,
) -> bool {
    let app_name = &op_ctx.app;
    let action_name = &op_ctx.action_name;
    let operation_id = OperationId(op_ctx.operation_id.clone());

    let (engine, mut scope, _) = crate::setup_language(script_limits);
    let ast = match engine.compile(script) {
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

    // r[impl operation.lifecycle.events] r[impl barrier.replay] r[impl operation.params]
    // Persist the current operation so a runtime restart can resume it.
    // Params are encrypted because they may carry secret values.
    // Callers that are inherently not replayable (backup actions, which hold
    // a per-process snapshot binding in operation_volume_bindings) pass
    // persist_for_replay=false and skip this step.
    if persist_for_replay {
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
            },
            &mut scope,
        );
        match result {
            OperationResult::Completed => {
                let app_name_owned = app_name.clone();
                db.call(move |db| {
                    faults::clear_faults_by_kind(db, &app_name_owned, "operation_failed").ok();
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
            // r[impl operation.cancel]
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
/// responsive) and long for protracted waits (so an hours-long backup doesn't
/// replay tens of thousands of times).
// r[impl barrier.suspension.poll-backoff]
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
        .filter_map(|e| e.barrier.as_ref())
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
// r[impl barrier.suspension.poll-backoff]
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

    // r[fault.image-pull] r[fault.container-start]
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
// i[impl event.types]
fn set_phase_and_persist(state: &OiState, app_name: &AppName, new_phase: AppPhase) {
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

// i[impl action.invoke.install]
fn enter_installing_phase(state: &OiState, app_name: &AppName) {
    set_phase_and_persist(state, app_name, AppPhase::Installing);
    tracing::info!(app = %app_name, "install started; entering Installing phase");
}

// i[impl action.invoke.install.completion]
fn finalize_install(state: &OiState, app_name: &AppName) {
    set_phase_and_persist(state, app_name, AppPhase::Installed);
    tracing::info!(app = %app_name, "install completed; app is now installed");
}

// i[impl action.invoke.install.completion]
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
    let (app, active_progress, tick_notify, script) = {
        let reg = state.registry.read();
        match reg.get(app_name.as_str()) {
            Some(e) => (
                e.app.clone(),
                Arc::clone(&e.active_progress),
                Arc::clone(&e.tick_notify),
                e.script.clone(),
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

    tokio::spawn(async move {
        // i[wire.actor]
        let op_ctx = event_tx.operation(
            app_name.clone(),
            action_name.clone(),
            &operation_id.0,
            source_generation,
            target_generation,
            actor,
        );
        op_ctx.started(&trigger);

        // i[impl action.invoke.install]
        // Flip to Installing as soon as the install operation actually starts
        // (covers both directly-accepted and later-dequeued installs because
        // both paths funnel through spawn_accepted_operation).
        if is_install {
            enter_installing_phase(&state, &app_name);
        }

        let operation_id_str = operation_id.0.clone();

        // r[impl operation.cancel]
        // Take the cancel token from the active scheduler record so a
        // subsequent cancel endpoint call wakes this operation.
        let cancel_token = state
            .scheduler
            .lock()
            .active()
            .filter(|a| a.operation_id == operation_id)
            .map(|a| Arc::clone(&a.cancel_token))
            .unwrap_or_else(|| Arc::new(CancelToken::new()));

        let success = {
            let app_name = app_name.clone();
            let active_progress = Arc::clone(&active_progress);
            let tick_notify = Arc::clone(&tick_notify);
            let params = params.clone();
            let op_ctx = op_ctx.clone();
            let cipher = Arc::clone(&cipher);
            let cancel_token = Arc::clone(&cancel_token);

            tokio::task::spawn_blocking(move || {
                let db = match open_operation_dbs(&db_path, &app_name) {
                    Some(d) => d,
                    None => return false,
                };
                run_operation_loop(
                    &app,
                    &script,
                    db,
                    params,
                    active_progress,
                    tick_notify,
                    &op_ctx,
                    &script_limits,
                    cipher,
                    HashMap::new(),
                    true,
                    cancel_token,
                )
            })
            .await
            .unwrap_or(false)
        };

        cleanup_dynamic_resources(&state, &operation_id_str, &active_progress, &tick_notify).await;

        *active_progress.write() = None;
        tick_notify.notify_one();

        // i[impl action.invoke.install.completion]
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
                None,
            );
        }
    });
}

#[expect(
    clippy::too_many_arguments,
    reason = "internal helper grouping all operation state"
)]
pub(crate) async fn run_operation_for_backup(
    state: &Arc<OiState>,
    backup_app_name: &AppName,
    action_name: &ActionName,
    operation_id: OperationId,
    params: serde_json::Map<String, serde_json::Value>,
    source_generation: u64,
    target_generation: u64,
    operation_volume_bindings: HashMap<String, OperationVolumeBinding>,
) -> bool {
    let (app, active_progress, tick_notify, script) = {
        let reg = state.registry.read();
        match reg.get(backup_app_name.as_str()) {
            Some(e) => (
                e.app.clone(),
                Arc::clone(&e.active_progress),
                Arc::clone(&e.tick_notify),
                e.script.clone(),
            ),
            None => {
                tracing::error!(app = %backup_app_name, "run_operation_for_backup: app not found");
                // The caller (backups.rs) has already taken a scheduler slot
                // for this operation via acquire_scheduler_slot. Release it
                // before bailing so a stale "active" entry doesn't block
                // every subsequent backup request forever — previously this
                // leak caused /backups/snapshots/list to spin in the
                // acquire loop indefinitely.
                state.scheduler.lock().complete_current();
                return false;
            }
        }
    };

    let db_path = state.db_path.clone();
    let script_limits = state.script_limits.clone();
    let cipher = Arc::clone(&state.cipher);
    let operation_id_str = operation_id.0.clone();

    let op_ctx = state.event_tx.operation(
        backup_app_name.clone(),
        action_name.clone(),
        &operation_id.0,
        source_generation,
        target_generation,
        None,
    );
    op_ctx.started("backup");

    // r[impl operation.cancel]
    let cancel_token = state
        .scheduler
        .lock()
        .active()
        .filter(|a| a.operation_id == operation_id)
        .map(|a| Arc::clone(&a.cancel_token))
        .unwrap_or_else(|| Arc::new(CancelToken::new()));

    let success = {
        let active_progress_clone = Arc::clone(&active_progress);
        let tick_notify_clone = Arc::clone(&tick_notify);
        let op_ctx = op_ctx.clone();
        let cancel_token = Arc::clone(&cancel_token);
        tokio::task::spawn_blocking(move || {
            let db = match open_operation_dbs(&db_path, &op_ctx.app) {
                Some(d) => d,
                None => return false,
            };
            run_operation_loop(
                &app,
                &script,
                db,
                params,
                active_progress_clone,
                tick_notify_clone,
                &op_ctx,
                &script_limits,
                cipher,
                operation_volume_bindings,
                // Backup ops carry a per-process snapshot binding in
                // operation_volume_bindings that cannot survive a restart;
                // the spec exempts them from replay.
                false,
                cancel_token,
            )
        })
        .await
        .unwrap_or(false)
    };

    cleanup_dynamic_resources(state, &operation_id_str, &active_progress, &tick_notify).await;
    *active_progress.write() = None;
    tick_notify.notify_one();

    let next = state.scheduler.lock().complete_current();
    if let Some(queued) = next {
        spawn_accepted_operation(
            Arc::clone(state),
            queued.app,
            queued.action,
            queued.operation_id,
            queued.params,
            queued.source_generation,
            queued.target_generation,
            queued.trigger,
            None,
        );
    }

    success
}
