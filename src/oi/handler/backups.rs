use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use crate::{
    defs::volume::OperationVolumeBinding,
    oi::{
        error::{ErrorCode, HandlerResult, OiError},
        handler::actions::lifecycle::run_operation_for_backup,
        state::OiState,
    },
    runtime::{backup_apps, backup_execution, backup_strategies, barrier::OperationId, faults},
};

#[derive(Deserialize)]
pub(crate) struct RegisterBackupAppParams {
    pub name: String,
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
        let def = entry.app.def.lock();
        let missing: Vec<&str> = backup_apps::REQUIRED_ACTIONS
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

    let db = state.db.lock();
    backup_apps::register(&db, &params.name, &params.app).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to register backup app: {e}"),
        )
    })?;

    Ok(json!({ "registered": true }))
}

#[derive(Deserialize)]
pub(crate) struct DeregisterBackupAppParams {
    pub name: String,
}

// i[impl backup.app.deregister]
pub(crate) fn deregister_backup_app(
    state: &OiState,
    params: DeregisterBackupAppParams,
) -> HandlerResult {
    let db = state.db.lock();

    // i[impl backup.app.deregister] — reject if any strategy references this backup app.
    let in_use = backup_strategies::references_backup_app(&db, &params.name).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to check strategy references: {e}"),
        )
    })?;
    if in_use {
        return Err(OiError::new(
            ErrorCode::BackupAppInUse,
            format!(
                "backup app {:?} is referenced by one or more strategies",
                params.name
            ),
        ));
    }

    let deleted = backup_apps::deregister(&db, &params.name).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to deregister backup app: {e}"),
        )
    })?;

    if !deleted {
        return Err(OiError::not_found(format!(
            "no backup app named {:?}",
            params.name
        )));
    }

    Ok(json!({ "deregistered": true }))
}

// i[impl backup.app.list]
pub(crate) fn list_backup_apps(state: &OiState) -> HandlerResult {
    let db = state.db.lock();
    let apps = backup_apps::list_all(&db).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to list backup apps: {e}"),
        )
    })?;

    let items: Vec<_> = apps
        .iter()
        .map(|a| json!({ "name": a.name, "app": a.app }))
        .collect();

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

    let db = state.db.lock();

    backup_apps::get_by_name(&db, &params.via)
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db backup apps: {e}")))?
        .ok_or_else(|| OiError::not_found(format!("no backup app named {:?}", params.via)))?;

    let strategy = backup_strategies::BackupStrategy {
        name: params.name,
        via: params.via,
        schedule: params.schedule,
        volumes: params.volumes,
        last_fired_at: None,
    };
    backup_strategies::create(&db, &strategy).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to create strategy: {e}"),
        )
    })?;

    Ok(json!({ "created": true }))
}

#[derive(Deserialize)]
pub(crate) struct StrategyNameParams {
    pub name: String,
}

// i[impl backup.strategy.show]
pub(crate) fn show_strategy(state: &OiState, params: StrategyNameParams) -> HandlerResult {
    let db = state.db.lock();
    let strategy = backup_strategies::get(&db, &params.name)
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db strategies: {e}")))?
        .ok_or_else(|| OiError::not_found(format!("no strategy named {:?}", params.name)))?;

    Ok(strategy_to_json(&strategy))
}

// i[impl backup.strategy.list]
pub(crate) fn list_strategies(state: &OiState) -> HandlerResult {
    let db = state.db.lock();
    let strategies = backup_strategies::list_all(&db).map_err(|e| {
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

    let db = state.db.lock();

    if let Some(via) = &params.via {
        backup_apps::get_by_name(&db, via)
            .map_err(|e| OiError::new(ErrorCode::Internal, format!("db backup apps: {e}")))?
            .ok_or_else(|| OiError::not_found(format!("no backup app named {via:?}")))?;
    }

    let updated = backup_strategies::update(
        &db,
        &params.name,
        params.via.as_deref(),
        params.schedule.as_deref(),
        params.volumes.as_deref(),
    )
    .map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to update strategy: {e}"),
        )
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
    let db = state.db.lock();
    let deleted = backup_strategies::delete(&db, &params.name).map_err(|e| {
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
    let db = state.db.lock();
    let strategy = backup_strategies::get(&db, &params.strategy)
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db strategies: {e}")))?
        .ok_or_else(|| OiError::not_found(format!("no strategy named {:?}", params.strategy)))?;
    drop(db);

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

// r[impl backup.execution]
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

// r[impl backup.validation.fire-time]
// r[impl backup.execution]
// r[impl backup.execution.per-volume-failure]
async fn run_strategy_backup(
    state: &Arc<OiState>,
    strategy: &backup_strategies::BackupStrategy,
    operation_ids: &[OperationId],
    is_manual: bool,
) {
    let (backup_app_name, backing_app_name) = {
        let db = state.db.lock();
        match backup_apps::get_by_name(&db, &strategy.via) {
            Ok(Some(ba)) => (ba.name, ba.app),
            Ok(None) => {
                tracing::error!(
                    strategy = %strategy.name,
                    via = %strategy.via,
                    "backup app no longer registered"
                );
                return;
            }
            Err(e) => {
                tracing::error!(
                    strategy = %strategy.name,
                    "failed to look up backup app: {e}"
                );
                return;
            }
        }
    };

    // r[impl backup.validation.fire-time]
    {
        let reg = state.registry.read();
        let valid = reg.get(&backing_app_name).is_some_and(|entry| {
            let def = entry.app.def.lock();
            backup_apps::REQUIRED_ACTIONS
                .iter()
                .all(|a| def.actions.contains_key(*a))
        });
        if !valid {
            tracing::error!(
                strategy = %strategy.name,
                app = %backing_app_name,
                "backup app missing required actions at fire time"
            );
            let db = state.db.lock();
            let _ = faults::file_fault(
                &db,
                &backing_app_name,
                None,
                None,
                None,
                "backup_app_unavailable",
                &format!("backup app {backing_app_name:?} is missing required backup actions"),
            );
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
        run_volume_backup(
            state,
            &backup_app_name,
            &backing_app_name,
            strategy,
            vol_id,
            op_id,
            is_manual,
        )
        .await;
    }
}

// r[impl backup.execution]
// r[impl backup.execution.retry]
async fn run_volume_backup(
    state: &Arc<OiState>,
    backup_app_name: &str,
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
            let db = state.db.lock();
            let _ = faults::file_fault(
                &db,
                backing_app_name,
                None,
                None,
                None,
                "backup_source_unavailable",
                &format!("strategy {:?}: {e}", strategy.name),
            );
            return;
        }
    };

    // r[impl backup.execution]
    if !source_path.exists() {
        tracing::error!(strategy = %strategy.name, vol = %vol_id, "source volume path does not exist");
        let db = state.db.lock();
        let _ = faults::file_fault(
            &db,
            backing_app_name,
            None,
            None,
            None,
            "backup_source_unavailable",
            &format!(
                "strategy {:?}: volume {vol_id:?} path does not exist",
                strategy.name
            ),
        );
        return;
    }

    let snapshot_name = format!("backup-snap-{}-{}", strategy.name, uuid::Uuid::new_v4());

    let mut attempt = 0u8;
    loop {
        attempt += 1;

        // r[impl backup.execution]
        if let Err(e) = vol_store.snapshot_site(&snapshot_name, &source_path).await {
            tracing::error!(
                strategy = %strategy.name,
                vol = %vol_id,
                attempt,
                "failed to create snapshot: {e}"
            );
            if attempt >= 2 {
                let db = state.db.lock();
                let _ = faults::file_fault(
                    &db,
                    backing_app_name,
                    None,
                    None,
                    None,
                    "backup_failed",
                    &format!(
                        "strategy {:?}: failed to snapshot volume {vol_id:?}: {e}",
                        strategy.name
                    ),
                );
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
        acquire_scheduler_slot(state, backup_app_name, operation_id).await;

        let mut bindings = std::collections::HashMap::new();
        bindings.insert(
            "source".to_owned(),
            OperationVolumeBinding {
                host_path: snapshot_path,
                read_only: true,
            },
        );

        let success = run_operation_for_backup(
            state,
            backup_app_name,
            "save-snapshot",
            operation_id.clone(),
            serde_json::Map::new(),
            0,
            0,
            bindings,
        )
        .await;

        // r[impl backup.execution]
        let _ = vol_store.remove_site(&snapshot_name).await;

        if success {
            let db = state.db.lock();
            faults::clear_faults_by_kind(&db, backing_app_name, "backup_failed").ok();
            faults::clear_faults_by_kind(&db, backing_app_name, "backup_source_unavailable").ok();
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
        let db = state.db.lock();
        let _ = faults::file_fault(
            &db,
            backing_app_name,
            None,
            None,
            None,
            "backup_failed",
            &format!(
                "strategy {:?}: save-snapshot failed for volume {vol_id:?}",
                strategy.name
            ),
        );
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
        Ok(vol_store.path(&format!("{prefix}-{vol}")))
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
    })
}
