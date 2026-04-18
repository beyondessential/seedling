use serde::Deserialize;
use serde_json::json;

use crate::{
    oi::{
        error::{ErrorCode, HandlerResult, OiError},
        state::OiState,
    },
    runtime::backup_apps,
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
    // Validate the app is registered and declares the required actions.
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
