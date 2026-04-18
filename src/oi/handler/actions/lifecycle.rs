use std::{path::Path, sync::Arc, time::Duration};

use parking_lot::{Mutex, RwLock};
use tokio::sync::Notify;

use crate::{
    defs::app::App,
    oi::{events, state::OiState},
    runtime::{
        AppPhase, AppRegistry, InstanceRegistry,
        barrier::{
            OperationId,
            oracle::DbWorldOracle,
            replay::{DbActionLog, OperationContext, OperationResult, run_operation},
        },
        db::Db,
        desired::OperationProgress,
        faults,
        registry::DbInstanceRegistry,
    },
};

/// Collected DB handle used by a single lifecycle operation pass.
struct OperationDbs {
    db: Arc<Mutex<Db>>,
}

fn open_operation_dbs(db_path: &Path, app_name: &str) -> Option<OperationDbs> {
    let db = match Db::open(db_path) {
        Ok(db) => Arc::new(Mutex::new(db)),
        Err(e) => {
            tracing::error!(app = %app_name, "open operation db: {e}");
            return None;
        }
    };
    Some(OperationDbs { db })
}

/// Run the operation loop synchronously until completion or failure.
///
/// Returns `true` on success, `false` on compile error, DB failure, or
/// operation failure.
#[expect(
    clippy::too_many_arguments,
    reason = "internal helper grouping all operation state"
)]
fn run_operation_loop(
    app: &App,
    app_name: &str,
    action_name: &str,
    operation_id: &OperationId,
    script: &str,
    dbs: OperationDbs,
    params: serde_json::Map<String, serde_json::Value>,
    active_progress: Arc<RwLock<Option<OperationProgress>>>,
    tick_notify: Arc<Notify>,
    event_tx: &events::EventSender,
    script_limits: &crate::ScriptLimits,
    source_generation: u64,
    target_generation: u64,
) -> bool {
    let (engine, mut scope, _) = crate::setup_language(script_limits);
    let ast = match engine.compile(script) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(app = %app_name, action = %action_name, "script compile error: {e}");
            let db = dbs.db.lock();
            let _ = faults::file_fault(
                &db,
                app_name,
                None,
                None,
                None,
                "operation_failed",
                &format!("script compile error in {action_name}: {e}"),
            );
            return false;
        }
    };

    let log = DbActionLog::new(
        Arc::clone(&dbs.db),
        operation_id.clone(),
        app_name,
        action_name,
    );
    let world = Arc::new(DbWorldOracle::new(Arc::clone(&dbs.db)));
    let registry: Arc<dyn InstanceRegistry> =
        Arc::new(DbInstanceRegistry::new(Arc::clone(&dbs.db)));

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
                db: Some(Arc::clone(&dbs.db)),
                source_generation,
                target_generation,
                script_limits: Some(script_limits.clone()),
                operation_volume_bindings: std::collections::HashMap::new(),
            },
            &mut scope,
        );
        match result {
            OperationResult::Completed => {
                let db = dbs.db.lock();
                faults::clear_faults_by_kind(&db, app_name, "operation_failed").ok();
                events::operation_completed(
                    event_tx,
                    app_name,
                    action_name,
                    &operation_id.0,
                    source_generation,
                    target_generation,
                );
                return true;
            }
            OperationResult::Failed(e) => {
                tracing::error!(app = %app_name, action = %action_name, "operation failed: {e}");
                let db = dbs.db.lock();
                let _ = faults::file_fault(
                    &db,
                    app_name,
                    None,
                    None,
                    None,
                    "operation_failed",
                    &format!("{action_name} failed: {e}"),
                );
                events::operation_failed(
                    event_tx,
                    app_name,
                    action_name,
                    &operation_id.0,
                    source_generation,
                    target_generation,
                    &e.to_string(),
                );
                return false;
            }
            OperationResult::Suspended(_) => {
                tick_notify.notify_one();
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

/// Tear down dynamic resources created during the operation.
///
/// Builds a cleanup `OperationProgress` with all dynamic instances marked as
/// stopped, then polls until they reach `Terminated` or a timeout is hit.
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

    let dynamic_records: Vec<_> = {
        let db = state.db.lock();
        list_dynamic_resources(&db)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.operation_id == operation_id_str)
            .collect()
    };

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
                            let db = state.db.lock();
                            let obs = query_observations(&db, inst).unwrap_or_default();
                            derive_lifecycle_state(inst, &obs)
                                .has_reached(LifecycleState::Terminated)
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

    let db = state.db.lock();
    if let Err(e) = delete_dynamic_resources_for_operation(&db, operation_id_str) {
        tracing::error!(
            operation_id = %operation_id_str,
            "failed to delete dynamic resource records: {e}"
        );
    }
}

/// Mark install as complete and persist the phase transition.
fn finalize_install(state: &OiState, app_name: &str) {
    // i[action.invoke.install.completion]
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(app_name) {
            *entry.phase.lock() = AppPhase::Installed;
            let db = state.db.lock();
            if let Err(e) = AppRegistry::persist_app(&db, entry) {
                tracing::error!(app = %app_name, "persist installed flag: {e}");
            }
        }
    }
    state.tick_notify.notify_one();
    tracing::info!(app = %app_name, "install completed; app is now installed");
}

/// Spawn an async task that runs a lifecycle operation to completion, then
/// handles queued follow-on operations and install completion bookkeeping.
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
        events::operation_started(
            &event_tx,
            &app_name,
            &action_name,
            &operation_id.0,
            source_generation,
            target_generation,
            &trigger,
        );
        let operation_id_str = operation_id.0.clone();

        // --- Run the operation on a blocking thread ---
        let success = {
            let event_tx = event_tx.clone();
            let app_name = app_name.clone();
            let action_name = action_name.clone();
            let active_progress = Arc::clone(&active_progress);
            let tick_notify = Arc::clone(&tick_notify);
            let params = params.clone();

            tokio::task::spawn_blocking(move || {
                let dbs = match open_operation_dbs(&db_path, &app_name) {
                    Some(d) => d,
                    None => return false,
                };
                run_operation_loop(
                    &app,
                    &app_name,
                    &action_name,
                    &operation_id,
                    &script,
                    dbs,
                    params,
                    active_progress,
                    tick_notify,
                    &event_tx,
                    &script_limits,
                    source_generation,
                    target_generation,
                )
            })
            .await
            .unwrap_or(false)
        };

        // --- Clean up dynamic resources ---
        cleanup_dynamic_resources(&state, &operation_id_str, &active_progress, &tick_notify).await;

        // --- Clear active progress and wake reconciler ---
        *active_progress.write() = None;
        tick_notify.notify_one();

        // --- Install completion ---
        if is_install && success {
            finalize_install(&state, &app_name);
        }

        // --- Drain the queue ---
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
            );
        }
    });
}
