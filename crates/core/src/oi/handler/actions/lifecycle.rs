use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use parking_lot::RwLock;
use seedling_protocol::events::OperationEventCtx;
use tokio::sync::Notify;

use crate::{
    defs::{app::App, volume::OperationVolumeBinding},
    oi::state::OiState,
    runtime::{
        AppPhase, InstanceRegistry,
        barrier::{
            OperationId,
            oracle::DbWorldOracle,
            replay::{DbActionLog, OperationContext, OperationResult, run_operation},
        },
        db::DbHandle,
        desired::OperationProgress,
        faults,
        registry::DbInstanceRegistry,
    },
};

fn open_operation_dbs(db_path: &Path, app_name: &str) -> Option<DbHandle> {
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
    operation_volume_bindings: HashMap<String, OperationVolumeBinding>,
) -> bool {
    let app_name = op_ctx.app.as_str();
    let action_name = op_ctx.action_name.as_str();
    let operation_id = OperationId(op_ctx.operation_id.clone());

    let (engine, mut scope, _) = crate::setup_language(script_limits);
    let ast = match engine.compile(script) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(app = %app_name, action = %action_name, "script compile error: {e}");
            let app_name_owned = app_name.to_owned();
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

    let log = DbActionLog::new(db.clone(), operation_id.clone(), app_name, action_name);
    let world = Arc::new(DbWorldOracle::new(db.clone()));
    let registry: Arc<dyn InstanceRegistry> = Arc::new(DbInstanceRegistry::new(db.clone()));

    loop {
        let result = run_operation(
            OperationContext {
                engine: &engine,
                script_ast: &ast,
                operation_id: operation_id.clone(),
                app,
                action_name,
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
                operation_volume_bindings: operation_volume_bindings.clone(),
            },
            &mut scope,
        );
        match result {
            OperationResult::Completed => {
                let app_name_owned = app_name.to_owned();
                db.call(move |db| {
                    faults::clear_faults_by_kind(db, &app_name_owned, "operation_failed").ok();
                });
                op_ctx.completed();
                return true;
            }
            OperationResult::Failed(e) => {
                tracing::error!(app = %app_name, action = %action_name, "operation failed: {e}");
                let app_name_owned = app_name.to_owned();
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
                });
                op_ctx.failed(&e.to_string());
                return false;
            }
            OperationResult::Suspended(_) => {
                tick_notify.notify_one();
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
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

                let all_stopped = {
                    let guard = active_progress.read();
                    if let Some(p) = &*guard {
                        p.dynamic_defs.keys().all(|inst| {
                            let inst = inst.clone();
                            state.db.call(move |db| {
                                let obs = query_observations(db, &inst).unwrap_or_default();
                                derive_lifecycle_state(&inst, &obs)
                                    .has_reached(LifecycleState::Terminated)
                            })
                        })
                    } else {
                        true
                    }
                };

                if all_stopped {
                    break;
                }

                tokio::time::sleep(Duration::from_secs(2)).await;
                tick_notify.notify_one();
            }
        }
    }

    let op_id_owned = operation_id_str.to_owned();
    state.db.call(move |db| {
        if let Err(e) = delete_dynamic_resources_for_operation(db, &op_id_owned) {
            tracing::error!(
                operation_id = %op_id_owned,
                "failed to delete dynamic resource records: {e}"
            );
        }
    });
}

fn finalize_install(state: &OiState, app_name: &str) {
    use crate::oi::handler::apps::{extract_persist_fields, persist_app_fields};
    // i[action.invoke.install.completion]
    // Update the in-memory phase under the write lock, then persist under a
    // read lock. Never hold the write lock while acquiring db: the schedule
    // ticker holds db then acquires registry.read(), so registry.write() +
    // db.lock() in either order creates a deadlock with it.
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(app_name) {
            *entry.phase.lock() = AppPhase::Installed;
        }
    }
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(app_name) {
            let (app_n, generation_n, installed, uninstalling) = extract_persist_fields(entry);
            if let Err(e) = state.db.call(move |db| {
                persist_app_fields(db, &app_n, generation_n, installed, uninstalling)
            }) {
                tracing::error!(app = %app_name, "persist installed flag: {e}");
            }
        }
    }
    state.tick_notify.notify_one();
    tracing::info!(app = %app_name, "install completed; app is now installed");
}

#[expect(
    clippy::too_many_arguments,
    reason = "internal helper grouping all operation state"
)]
pub fn spawn_accepted_operation(
    state: Arc<OiState>,
    app_name: String,
    action_name: String,
    operation_id: OperationId,
    params: serde_json::Map<String, serde_json::Value>,
    source_generation: u64,
    target_generation: u64,
    trigger: String,
    actor: Option<std::sync::Arc<seedling_protocol::actor::Actor>>,
) {
    let (app, active_progress, tick_notify, script) = {
        let reg = state.registry.read();
        match reg.get(&app_name) {
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
    let is_install = action_name == "install";

    tokio::spawn(async move {
        // i[wire.actor]
        let op_ctx = event_tx.operation(
            &app_name,
            &action_name,
            &operation_id.0,
            source_generation,
            target_generation,
            actor,
        );
        op_ctx.started(&trigger);

        let operation_id_str = operation_id.0.clone();

        let success = {
            let app_name = app_name.clone();
            let active_progress = Arc::clone(&active_progress);
            let tick_notify = Arc::clone(&tick_notify);
            let params = params.clone();
            let op_ctx = op_ctx.clone();

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
                    HashMap::new(),
                )
            })
            .await
            .unwrap_or(false)
        };

        cleanup_dynamic_resources(&state, &operation_id_str, &active_progress, &tick_notify).await;

        *active_progress.write() = None;
        tick_notify.notify_one();

        if is_install && success {
            finalize_install(&state, &app_name);
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
    backup_app_name: &str,
    action_name: &str,
    operation_id: OperationId,
    params: serde_json::Map<String, serde_json::Value>,
    source_generation: u64,
    target_generation: u64,
    operation_volume_bindings: HashMap<String, OperationVolumeBinding>,
) -> bool {
    let (app, active_progress, tick_notify, script) = {
        let reg = state.registry.read();
        match reg.get(backup_app_name) {
            Some(e) => (
                e.app.clone(),
                Arc::clone(&e.active_progress),
                Arc::clone(&e.tick_notify),
                e.script.clone(),
            ),
            None => {
                tracing::error!(app = %backup_app_name, "run_operation_for_backup: app not found");
                return false;
            }
        }
    };

    let db_path = state.db_path.clone();
    let script_limits = state.script_limits.clone();
    let operation_id_str = operation_id.0.clone();

    let op_ctx = state.event_tx.operation(
        backup_app_name,
        action_name,
        &operation_id.0,
        source_generation,
        target_generation,
        None,
    );
    op_ctx.started("backup");

    let success = {
        let active_progress_clone = Arc::clone(&active_progress);
        let tick_notify_clone = Arc::clone(&tick_notify);
        let op_ctx = op_ctx.clone();
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
                operation_volume_bindings,
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
