use serde::Deserialize;
use serde_json::json;

use crate::{
    oi::{
        error::{ErrorCode, OiError},
        state::OiState,
    },
    runtime::{apps, registries},
};

use super::HandlerResult;

#[derive(Deserialize)]
pub(crate) struct RegistryParams {
    pub registry: String,
}

// i[registry.list]
pub(crate) fn list_registries(state: &OiState) -> HandlerResult {
    let db = state.db.lock();
    let registries = registries::list_allowed_registries(&db)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    Ok(json!({ "registries": registries }))
}

// i[registry.add]
pub(crate) fn add_registry(state: &OiState, params: RegistryParams) -> HandlerResult {
    let db = state.db.lock();
    registries::add_allowed_registry(&db, &params.registry)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    Ok(json!({ "ok": true }))
}

// i[registry.remove]
pub(crate) fn remove_registry(state: &OiState, params: RegistryParams) -> HandlerResult {
    let db = state.db.lock();
    let removed = registries::remove_allowed_registry(&db, &params.registry)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    if !removed {
        return Err(OiError::not_found(format!(
            "registry not in allowlist: {}",
            params.registry
        )));
    }
    drop(db);

    re_evaluate_all_apps(state);

    Ok(json!({ "ok": true }))
}

fn re_evaluate_all_apps(state: &OiState) {
    let app_names: Vec<String> = {
        let reg = state.registry.read();
        reg.list().into_iter().map(|(name, _)| name).collect()
    };
    for name in &app_names {
        let script = {
            let reg = state.registry.read();
            match reg.get(name) {
                Some(entry) => entry.script.clone(),
                None => continue,
            }
        };
        let loaded_params = {
            let db = state.db.lock();
            apps::load_params_for_app(&db, name).unwrap_or_default()
        };
        state
            .registry
            .write()
            .reload(name, script, &loaded_params, &state.script_limits);
        {
            let reg = state.registry.read();
            if let Some(entry) = reg.get(name) {
                let db = state.db.lock();
                apps::sync_script_error_fault(&db, entry);
                apps::sync_registry_faults(&db, entry);
            }
        }
    }
}
