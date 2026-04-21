use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use seedling_protocol::{
    backup_actions,
    error::{ErrorCode, HandlerResult, OiError},
};

use crate::{
    defs::volume::{VolumeParamSpec, build_operation_volume_params},
    oi::{handler::actions::lifecycle::run_operation_for_backup, state::OiState},
    runtime::{backup_apps, backup_execution, backup_strategies, barrier::OperationId, faults},
};

#[derive(Deserialize)]
pub(crate) struct RegisterBackupAppParams {
    pub app: String,
}

// i[impl backup.app.register]
pub(crate) fn register_backup_app(
    state: &OiState,
    params: RegisterBackupAppParams,
) -> HandlerResult {
    {
        let reg = state.registry.read();
        let entry = reg
            .get(&params.app)
            .ok_or_else(|| OiError::not_found(format!("app not found: {}", params.app)))?;
        let def = entry.app.def.load();
        let missing: Vec<&str> = backup_actions::REQUIRED_ACTIONS
            .iter()
            .copied()
            .filter(|a| !def.actions.contains_key(*a))
            .collect();
        if !missing.is_empty() {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "app {:?} is missing required backup actions: {}",
                    params.app,
                    missing.join(", ")
                ),
            ));
        }
    }

    let app_owned = params.app.clone();
    state
        .db
        .call(move |db| backup_apps::register(db, &app_owned))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to register backup app: {e}"),
            )
        })?;

    Ok(json!({ "registered": true }))
}

#[derive(Deserialize)]
pub(crate) struct DeregisterBackupAppParams {
    pub app: String,
}

// i[impl backup.app.deregister]
pub(crate) fn deregister_backup_app(
    state: &OiState,
    params: DeregisterBackupAppParams,
) -> HandlerResult {
    // i[impl backup.app.deregister] — reject if any strategy references this backup app.
    let app_owned = params.app.clone();
    let (in_use, deleted) = state.db.call(move |db| -> Result<_, OiError> {
        let in_use = backup_strategies::references_backup_app(db, &app_owned).map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to check strategy references: {e}"),
            )
        })?;
        if in_use {
            return Ok((true, false));
        }
        let deleted = backup_apps::deregister(db, &app_owned).map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to deregister backup app: {e}"),
            )
        })?;
        Ok((false, deleted))
    })?;

    if in_use {
        return Err(OiError::new(
            ErrorCode::BackupAppInUse,
            format!(
                "backup app {:?} is referenced by one or more strategies",
                params.app
            ),
        ));
    }

    if !deleted {
        return Err(OiError::not_found(format!(
            "app {:?} is not registered as a backup app",
            params.app
        )));
    }

    Ok(json!({ "deregistered": true }))
}

// i[impl backup.app.list]
pub(crate) fn list_backup_apps(state: &OiState) -> HandlerResult {
    let apps = state.db.call(backup_apps::list_all).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to list backup apps: {e}"),
        )
    })?;

    let items: Vec<_> = apps.iter().map(|a| json!({ "app": a })).collect();

    Ok(json!(items))
}

#[derive(Deserialize)]
pub(crate) struct CreateStrategyParams {
    pub name: String,
    pub via: String,
    pub schedule: String,
    pub volumes: Vec<String>,
}

// i[impl backup.strategy.create]
pub(crate) fn create_strategy(state: &OiState, params: CreateStrategyParams) -> HandlerResult {
    validate_schedule(&params.schedule)?;

    let strategy = backup_strategies::BackupStrategy {
        name: params.name,
        via: params.via,
        schedule: params.schedule,
        volumes: params.volumes,
        last_fired_at: None,
    };
    state.db.call(move |db| -> Result<_, OiError> {
        if !backup_apps::is_registered(db, &strategy.via)
            .map_err(|e| OiError::new(ErrorCode::Internal, format!("db backup apps: {e}")))?
        {
            return Err(OiError::not_found(format!(
                "app {:?} is not registered as a backup app",
                strategy.via
            )));
        }
        backup_strategies::create(db, &strategy).map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to create strategy: {e}"),
            )
        })?;
        Ok(())
    })?;

    Ok(json!({ "created": true }))
}

#[derive(Deserialize)]
pub(crate) struct StrategyNameParams {
    pub name: String,
}

// i[impl backup.strategy.show]
pub(crate) fn show_strategy(state: &OiState, params: StrategyNameParams) -> HandlerResult {
    let name_owned = params.name.clone();
    let strategy = state
        .db
        .call(move |db| backup_strategies::get(db, &name_owned))
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db strategies: {e}")))?
        .ok_or_else(|| OiError::not_found(format!("no strategy named {:?}", params.name)))?;

    Ok(strategy_to_json(&strategy))
}

// i[impl backup.strategy.list]
pub(crate) fn list_strategies(state: &OiState) -> HandlerResult {
    let strategies = state.db.call(backup_strategies::list_all).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to list strategies: {e}"),
        )
    })?;

    Ok(json!(
        strategies.iter().map(strategy_to_json).collect::<Vec<_>>()
    ))
}

#[derive(Deserialize)]
pub(crate) struct UpdateStrategyParams {
    pub name: String,
    pub via: Option<String>,
    pub schedule: Option<String>,
    pub volumes: Option<Vec<String>>,
}

// i[impl backup.strategy.update]
pub(crate) fn update_strategy(state: &OiState, params: UpdateStrategyParams) -> HandlerResult {
    if let Some(sched) = &params.schedule {
        validate_schedule(sched)?;
    }

    let name_owned = params.name.clone();
    let via_owned = params.via.clone();
    let schedule_owned = params.schedule.clone();
    let volumes_owned = params.volumes.clone();
    let updated = state.db.call(move |db| -> Result<bool, OiError> {
        if let Some(ref via) = via_owned
            && !backup_apps::is_registered(db, via)
                .map_err(|e| OiError::new(ErrorCode::Internal, format!("db backup apps: {e}")))?
        {
            return Err(OiError::not_found(format!(
                "app {via:?} is not registered as a backup app"
            )));
        }
        backup_strategies::update(
            db,
            &name_owned,
            via_owned.as_deref(),
            schedule_owned.as_deref(),
            volumes_owned.as_deref(),
        )
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to update strategy: {e}"),
            )
        })
    })?;

    if !updated {
        return Err(OiError::not_found(format!(
            "no strategy named {:?}",
            params.name
        )));
    }

    Ok(json!({ "updated": true }))
}

// i[impl backup.strategy.delete]
pub(crate) fn delete_strategy(state: &OiState, params: StrategyNameParams) -> HandlerResult {
    let name_owned = params.name.clone();
    let deleted = state
        .db
        .call(move |db| backup_strategies::delete(db, &name_owned))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to delete strategy: {e}"),
            )
        })?;

    if !deleted {
        return Err(OiError::not_found(format!(
            "no strategy named {:?}",
            params.name
        )));
    }

    Ok(json!({ "deleted": true }))
}

#[derive(Deserialize)]
pub(crate) struct RunBackupParams {
    pub strategy: String,
}

// i[impl backup.run]
pub(crate) fn run_backup(state: &Arc<OiState>, params: RunBackupParams) -> HandlerResult {
    let strategy_name = params.strategy.clone();
    let strategy = state
        .db
        .call(move |db| backup_strategies::get(db, &strategy_name))
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db strategies: {e}")))?
        .ok_or_else(|| OiError::not_found(format!("no strategy named {:?}", params.strategy)))?;

    let ids: Vec<OperationId> = strategy
        .volumes
        .iter()
        .map(|_| OperationId::new())
        .collect();

    let operations: Vec<_> = strategy
        .volumes
        .iter()
        .zip(ids.iter())
        .map(|(vol, id)| json!({ "volume": vol, "operation_id": id.0 }))
        .collect();

    spawn_backup_run(Arc::clone(state), strategy, ids, true);
    Ok(json!(operations))
}

pub fn spawn_backup_run(
    state: Arc<OiState>,
    strategy: backup_strategies::BackupStrategy,
    operation_ids: Vec<OperationId>,
    is_manual: bool,
) {
    tokio::spawn(async move {
        run_strategy_backup(&state, &strategy, &operation_ids, is_manual).await;
    });
}

// r[impl backup.execution]
async fn run_strategy_backup(
    state: &Arc<OiState>,
    strategy: &backup_strategies::BackupStrategy,
    operation_ids: &[OperationId],
    is_manual: bool,
) {
    // With the nickname gone, strategy.via IS the BSL app name.
    let backing_app_name = strategy.via.clone();
    let via_owned = backing_app_name.clone();
    let strategy_name_for_err = strategy.name.clone();
    let registered = tokio::task::block_in_place(|| {
        state
            .db
            .call(move |db| backup_apps::is_registered(db, &via_owned))
    });
    match registered {
        Ok(true) => {}
        Ok(false) => {
            tracing::error!(
                strategy = %strategy_name_for_err,
                via = %strategy.via,
                "backup app no longer registered"
            );
            return;
        }
        Err(e) => {
            tracing::error!(
                strategy = %strategy_name_for_err,
                "failed to look up backup app: {e}"
            );
            return;
        }
    }

    // r[impl backup.validation.fire-time]
    {
        let reg = state.registry.read();
        let valid = reg.get(&backing_app_name).is_some_and(|entry| {
            let def = entry.app.def.load();
            backup_actions::REQUIRED_ACTIONS
                .iter()
                .all(|a| def.actions.contains_key(*a))
        });
        if !valid {
            tracing::error!(
                strategy = %strategy.name,
                app = %backing_app_name,
                "backup app missing required actions at fire time"
            );
            let app_owned = backing_app_name.clone();
            tokio::task::block_in_place(|| {
                state.db.call(move |db| {
                    let _ = faults::file_fault(
                        db,
                        &app_owned,
                        None,
                        None,
                        None,
                        "backup_app_unavailable",
                        &format!("backup app {app_owned:?} is missing required backup actions"),
                    );
                })
            });
            return;
        }
    }

    if !is_manual {
        let delay = backup_execution::random_delay_secs(&strategy.schedule);
        if delay > 0 {
            tracing::debug!(strategy = %strategy.name, delay_secs = delay, "applying backup delay");
            tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
        }
    }

    for (vol_id, op_id) in strategy.volumes.iter().zip(operation_ids.iter()) {
        // r[impl backup.execution.per-volume-failure]
        run_volume_backup(state, &backing_app_name, strategy, vol_id, op_id, is_manual).await;
    }
}

// r[impl backup.execution]
// r[impl backup.execution.retry]
async fn run_volume_backup(
    state: &Arc<OiState>,
    backing_app_name: &str,
    strategy: &backup_strategies::BackupStrategy,
    vol_id: &str,
    operation_id: &OperationId,
    is_manual: bool,
) {
    let vol_store = &state.driver.volume_store;

    let source_path = match parse_vol_id_to_path(vol_id, vol_store) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(strategy = %strategy.name, vol = %vol_id, "invalid volume id: {e}");
            let app_owned = backing_app_name.to_owned();
            let desc = format!("strategy {:?}: {e}", strategy.name);
            tokio::task::block_in_place(|| {
                state.db.call(move |db| {
                    let _ = faults::file_fault(
                        db,
                        &app_owned,
                        None,
                        None,
                        None,
                        "backup_source_unavailable",
                        &desc,
                    );
                })
            });
            return;
        }
    };

    if !source_path.exists() {
        tracing::error!(strategy = %strategy.name, vol = %vol_id, "source volume path does not exist");
        let app_owned = backing_app_name.to_owned();
        let desc = format!(
            "strategy {:?}: volume {vol_id:?} path does not exist",
            strategy.name
        );
        tokio::task::block_in_place(|| {
            state.db.call(move |db| {
                let _ = faults::file_fault(
                    db,
                    &app_owned,
                    None,
                    None,
                    None,
                    "backup_source_unavailable",
                    &desc,
                );
            })
        });
        return;
    }

    let snapshot_name = format!("backup-snap-{}-{}", strategy.name, uuid::Uuid::new_v4());

    let mut attempt = 0u8;
    loop {
        attempt += 1;

        if let Err(e) = vol_store.snapshot_site(&snapshot_name, &source_path).await {
            tracing::error!(
                strategy = %strategy.name,
                vol = %vol_id,
                attempt,
                "failed to create snapshot: {e}"
            );
            if attempt >= 2 {
                let app_owned = backing_app_name.to_owned();
                let desc = format!(
                    "strategy {:?}: failed to snapshot volume {vol_id:?}: {e}",
                    strategy.name
                );
                tokio::task::block_in_place(|| {
                    state.db.call(move |db| {
                        let _ = faults::file_fault(
                            db,
                            &app_owned,
                            None,
                            None,
                            None,
                            "backup_failed",
                            &desc,
                        );
                    })
                });
            }
            if attempt < 2 {
                let delay = backup_execution::random_delay_secs(&strategy.schedule);
                tokio::time::sleep(tokio::time::Duration::from_secs(delay.max(1))).await;
                continue;
            }
            return;
        }

        let snapshot_path = vol_store.site_path(&snapshot_name);

        // r[impl backup.execution]
        // The scheduler and registry are both keyed on the BSL app name
        // (backing_app_name, e.g. "backup-kopia-s3"), not on the backup-app
        // nickname (backup_app_name, e.g. "kopia"). Pass backing_app_name to
        // both so the scheduler correctly serialises with other operations on
        // the same BSL app and run_operation_for_backup actually finds the
        // app in the registry.
        acquire_scheduler_slot(state, backing_app_name, operation_id).await;

        // r[impl operation.volume-param]
        let (bindings, mut action_params) = build_operation_volume_params(
            &operation_id.0,
            [(
                "source",
                VolumeParamSpec {
                    host_path: snapshot_path,
                    read_only: true,
                    filename: None,
                },
            )],
        );

        // Pass the structured backup object through so the action can stamp
        // (strategy, app, volume) onto the snapshot. list-snapshots and
        // restore-snapshot receive the same object and are expected to
        // filter on it — otherwise an operator restoring "myapp/data" from
        // a strategy that shares a backend repository with "otherapp/logs"
        // could see both, pick wrong, and overwrite the wrong target.
        action_params.insert(
            "backup".to_owned(),
            build_backup_param(&strategy.name, vol_id),
        );

        let success = run_operation_for_backup(
            state,
            backing_app_name,
            "save-snapshot",
            operation_id.clone(),
            action_params,
            0,
            0,
            bindings,
        )
        .await;

        // r[impl backup.execution]
        let _ = vol_store.remove_site(&snapshot_name).await;

        if success {
            let app_owned = backing_app_name.to_owned();
            tokio::task::block_in_place(|| {
                state.db.call(move |db| {
                    faults::clear_faults_by_kind(db, &app_owned, "backup_failed").ok();
                    faults::clear_faults_by_kind(db, &app_owned, "backup_source_unavailable").ok();
                })
            });
            return;
        }

        // r[impl backup.execution.retry]
        if attempt < 2 {
            tracing::warn!(
                strategy = %strategy.name,
                vol = %vol_id,
                "backup save-snapshot failed, will retry after delay"
            );
            if !is_manual {
                let delay = backup_execution::random_delay_secs(&strategy.schedule);
                tokio::time::sleep(tokio::time::Duration::from_secs(delay.max(1))).await;
            }
            continue;
        }

        tracing::error!(
            strategy = %strategy.name,
            vol = %vol_id,
            "backup save-snapshot failed after retry"
        );
        let app_owned = backing_app_name.to_owned();
        let desc = format!(
            "strategy {:?}: save-snapshot failed for volume {vol_id:?}",
            strategy.name
        );
        tokio::task::block_in_place(|| {
            state.db.call(move |db| {
                let _ =
                    faults::file_fault(db, &app_owned, None, None, None, "backup_failed", &desc);
            })
        });
        return;
    }
}

// r[impl backup.execution]
async fn acquire_scheduler_slot(state: &Arc<OiState>, app_name: &str, operation_id: &OperationId) {
    loop {
        {
            let mut sched = state.scheduler.lock();
            if sched.active().is_none() {
                sched.request_with_id(
                    app_name,
                    "save-snapshot",
                    serde_json::Map::new(),
                    0,
                    0,
                    "backup",
                    operation_id.clone(),
                );
                return;
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

#[derive(Deserialize)]
pub(crate) struct ListSnapshotsParams {
    pub strategy: String,
    pub volume: String,
}

// i[impl backup.snapshots.list]
pub(crate) fn list_snapshots(state: &Arc<OiState>, params: ListSnapshotsParams) -> HandlerResult {
    tokio::runtime::Handle::current().block_on(list_snapshots_async(state, params))
}

// r[impl backup.list]
async fn list_snapshots_async(state: &Arc<OiState>, params: ListSnapshotsParams) -> HandlerResult {
    let backing_app_name = resolve_backup_app(state, &params.strategy, &params.volume)?;
    validate_backup_app_actions(state, &backing_app_name)?;

    let tempdir = tempfile::tempdir().map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to create temp dir: {e}"),
        )
    })?;

    let operation_id = OperationId::new();
    // Scheduler + registry are keyed on the BSL app (backing_app_name),
    // not the backup-app nickname (backup_app_name).
    acquire_scheduler_slot(state, &backing_app_name, &operation_id).await;

    // r[impl operation.volume-param] r[impl operation.volume-param.filename]
    // The output filename is an implementation detail of the runtime — the
    // action closure receives it via `param["output_filename"]` and must
    // write exactly that name.
    let output_filename = "snapshots.json";
    let (bindings, mut action_params) = build_operation_volume_params(
        &operation_id.0,
        [(
            "output",
            VolumeParamSpec {
                host_path: tempdir.path().to_owned(),
                read_only: false,
                filename: Some(output_filename.to_owned()),
            },
        )],
    );

    action_params.insert(
        "backup".to_owned(),
        build_backup_param(&params.strategy, &params.volume),
    );

    let success = run_operation_for_backup(
        state,
        &backing_app_name,
        "list-snapshots",
        operation_id,
        action_params,
        0,
        0,
        bindings,
    )
    .await;

    if !success {
        return Err(OiError::new(
            ErrorCode::Internal,
            "list-snapshots action failed".to_owned(),
        ));
    }

    let snapshots_path = tempdir.path().join(output_filename);
    let raw: Vec<u8> = tokio::fs::read(&snapshots_path).await.map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("list-snapshots did not write {output_filename:?}: {e}"),
        )
    })?;

    let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("{output_filename:?} is not valid JSON: {e}"),
        )
    })?;

    Ok(value)
}

#[derive(Deserialize)]
pub(crate) struct RestoreBackupParams {
    pub strategy: String,
    pub volume: String,
    pub snapshot: String,
}

// i[impl backup.restore]
pub(crate) fn restore_backup(state: &Arc<OiState>, params: RestoreBackupParams) -> HandlerResult {
    tokio::runtime::Handle::current().block_on(restore_backup_async(state, params))
}

// r[impl backup.restore]
async fn restore_backup_async(state: &Arc<OiState>, params: RestoreBackupParams) -> HandlerResult {
    let backing_app_name = resolve_backup_app(state, &params.strategy, &params.volume)?;
    validate_backup_app_actions(state, &backing_app_name)?;

    let vol_store = &state.driver.volume_store;
    let site_vol_name = format!(
        "restore-{}-{}",
        params.strategy,
        uuid::Uuid::new_v4().simple()
    );

    vol_store.create_site(&site_vol_name).await.map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to create restore site volume: {e}"),
        )
    })?;

    let operation_id = OperationId::new();
    // Scheduler + registry are keyed on the BSL app (backing_app_name),
    // not the backup-app nickname (backup_app_name).
    acquire_scheduler_slot(state, &backing_app_name, &operation_id).await;

    let dest_path = vol_store.site_path(&site_vol_name);
    // r[impl operation.volume-param]
    let (bindings, mut action_params) = build_operation_volume_params(
        &operation_id.0,
        [(
            "destination",
            VolumeParamSpec {
                host_path: dest_path,
                read_only: false,
                filename: None,
            },
        )],
    );

    action_params.insert(
        "backup".to_owned(),
        build_backup_param(&params.strategy, &params.volume),
    );
    action_params.insert("snapshot".to_owned(), json!(params.snapshot));

    let success = run_operation_for_backup(
        state,
        &backing_app_name,
        "restore-snapshot",
        operation_id,
        action_params,
        0,
        0,
        bindings,
    )
    .await;

    if !success {
        let _ = vol_store.remove_site(&site_vol_name).await;
        return Err(OiError::new(
            ErrorCode::Internal,
            "restore-snapshot action failed".to_owned(),
        ));
    }

    // r[impl backup.restore]
    // Register the newly populated site volume in the site_volumes table so
    // it's visible via /volumes/site/list and the web UI. vol_store.create_site
    // only creates the on-disk directory (or btrfs subvolume); without this
    // insertion the restored data would sit on disk but be unreachable.
    //
    // Kind=Managed, not Snapshot: Snapshot implies btrfs-level read-only
    // semantics, but the operator's intended use for a restored volume is
    // usually to map it into an app and write to it (swap it in for the
    // original). If they want a read-only view they can export it that way.
    let created_at = jiff::Timestamp::now().to_string();
    let def = crate::runtime::site_volumes::SiteVolumeDef {
        name: site_vol_name.clone(),
        kind: crate::runtime::site_volumes::SiteVolumeKind::Managed,
        created_at,
    };
    if let Err(e) = state
        .db
        .call(move |db| crate::runtime::site_volumes::create(db, &def))
    {
        // The on-disk data is fine; only the registry insert failed. Tear
        // down the directory so we don't leak orphan data whose existence
        // the operator can't see. Returning the error is honest: the
        // restore didn't achieve its stated effect.
        let _ = vol_store.remove_site(&site_vol_name).await;
        return Err(OiError::new(
            ErrorCode::Internal,
            format!("restore succeeded but failed to register site volume: {e}"),
        ));
    }

    Ok(json!({ "site_volume": site_vol_name }))
}

/// Resolve the BSL app name backing a strategy. Errors if the strategy is
/// missing or if its `via` app is no longer registered as a backup app.
fn resolve_backup_app(
    state: &Arc<OiState>,
    strategy_name: &str,
    _volume: &str,
) -> Result<String, OiError> {
    let strategy_name_owned = strategy_name.to_owned();
    state.db.call(move |db| -> Result<String, OiError> {
        let strategy = backup_strategies::get(db, &strategy_name_owned)
            .map_err(|e| OiError::new(ErrorCode::Internal, format!("db strategies: {e}")))?
            .ok_or_else(|| {
                OiError::not_found(format!("no strategy named {strategy_name_owned:?}"))
            })?;
        let registered = backup_apps::is_registered(db, &strategy.via)
            .map_err(|e| OiError::new(ErrorCode::Internal, format!("db backup apps: {e}")))?;
        if !registered {
            return Err(OiError::not_found(format!(
                "app {:?} is no longer registered as a backup app",
                strategy.via
            )));
        }
        Ok(strategy.via)
    })
}

fn validate_backup_app_actions(
    state: &Arc<OiState>,
    backing_app_name: &str,
) -> Result<(), OiError> {
    let reg = state.registry.read();
    let valid = reg.get(backing_app_name).is_some_and(|entry| {
        let def = entry.app.def.load();
        backup_actions::REQUIRED_ACTIONS
            .iter()
            .all(|a| def.actions.contains_key(*a))
    });
    if !valid {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("backup app {backing_app_name:?} is missing required backup actions"),
        ));
    }
    Ok(())
}

/// Build the `backup` object passed to every backup-app action closure.
///
/// The object carries the structured identity of the backup operation —
/// which strategy triggered it, which app owns the source volume, and which
/// named volume within that app is being backed up. Backup apps use it for
/// two things:
///
///  1. **save-snapshot** tags the remote snapshot with `app` + `volume` so
///     the snapshot can be attributed later.
///  2. **list-snapshots** filters its JSON output to only the snapshots
///     matching the `app` + `volume` combination.
///
/// `restore-snapshot` also receives this object in addition to the opaque
/// `snapshot` identifier; the action is expected to verify the snapshot
/// belongs to the requested `app`/`volume` pair before writing to the
/// destination volume.
///
/// The volume id has the shape `"<app>/<volume>"` or `"_site/<volume>"` — we
/// split it once on `/` to produce the structured fields.
// i[impl backup.action.backup-param]
fn build_backup_param(strategy_name: &str, vol_id: &str) -> serde_json::Value {
    let (app, volume) = vol_id.split_once('/').unwrap_or((vol_id, ""));
    json!({
        "strategy": strategy_name,
        "app": app,
        "volume": volume,
    })
}

fn parse_vol_id_to_path(
    vol_id: &str,
    vol_store: &crate::system::volume_store::VolumeStore,
) -> Result<std::path::PathBuf, String> {
    let (prefix, vol) = vol_id.split_once('/').ok_or_else(|| {
        format!("invalid volume id {vol_id:?}: expected _site/<name> or <app>/<volume>")
    })?;
    if prefix.is_empty() || vol.is_empty() {
        return Err(format!(
            "invalid volume id {vol_id:?}: neither part may be empty"
        ));
    }
    if prefix == "_site" {
        Ok(vol_store.site_path(vol))
    } else {
        Ok(vol_store.path(&crate::runtime::identity::VolumeName::for_app(prefix, vol)))
    }
}

fn validate_schedule(schedule: &str) -> Result<(), OiError> {
    if backup_strategies::VALID_SCHEDULES.contains(&schedule) {
        Ok(())
    } else {
        Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "invalid schedule {:?}; must be one of: {}",
                schedule,
                backup_strategies::VALID_SCHEDULES.join(", ")
            ),
        ))
    }
}

fn strategy_to_json(s: &backup_strategies::BackupStrategy) -> serde_json::Value {
    json!({
        "name": s.name,
        "via": s.via,
        "schedule": s.schedule,
        "volumes": s.volumes,
        "last_fired_at": s.last_fired_at,
        "next_fire_at": compute_next_fire_at(s),
    })
}

/// The cron boundary at which this strategy is expected to fire next,
/// mirroring the logic in `backup_execution::check_due_strategies`. Returns
/// `None` when the schedule is unknown or parsing fails.
fn compute_next_fire_at(s: &backup_strategies::BackupStrategy) -> Option<String> {
    use jiff::{SignedDuration, Timestamp};

    let cronexpr_str = backup_execution::schedule_to_cronexpr(&s.schedule)?;
    let crontab = crate::defs::action::parse_cron_expr(cronexpr_str, "backup", &s.name).ok()?;

    let now = Timestamp::now();
    let base_time = match &s.last_fired_at {
        Some(ts) => ts.parse::<Timestamp>().unwrap_or_else(|_| {
            now.checked_sub(SignedDuration::from_secs(300))
                .unwrap_or(now)
        }),
        None => now
            .checked_sub(SignedDuration::from_secs(300))
            .unwrap_or(now),
    };

    let next = crontab.find_next(base_time).ok()?;
    Some(Timestamp::from(next).to_string())
}
