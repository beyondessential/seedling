use serde::Deserialize;
use serde_json::json;

use crate::{
    oi::{
        error::{ErrorCode, HandlerResult, OiError},
        state::OiState,
    },
    runtime::{backup_apps, backup_strategies},
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
#[expect(dead_code, reason = "fields read in Phase 6 execution stub")]
pub(crate) struct RunBackupParams {
    pub strategy: String,
    pub volume: Option<String>,
}

// i[impl backup.run]
pub(crate) fn run_backup(_state: &OiState, _params: RunBackupParams) -> HandlerResult {
    todo!("backup execution not yet implemented (Phase 6)")
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
