use seedling_protocol::error::{ErrorCode, OiError};
use seedling_protocol::names::AppName;
use serde::Deserialize;
use serde_json::json;

use super::HandlerResult;
use crate::{
    oi::state::OiState,
    runtime::{apps, registries},
};

#[derive(Deserialize)]
pub(crate) struct RegistryParams {
    pub registry: String,
}

#[derive(Deserialize)]
pub(crate) struct TagsParams {
    pub repository: String,
}

// spec: DEFN#repository-tags
pub(crate) fn list_tags(state: &OiState, params: TagsParams) -> HandlerResult {
    let repository =
        crate::runtime::definition::fetch::DefinitionRef::parse_repository(&params.repository)?;
    let allowed = state
        .db
        .call(registries::list_allowed_registries)
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?;
    crate::runtime::definition::fetch::check_allowed(&allowed, &repository)?;
    let tags = super::definition::block_on(crate::runtime::definition::fetch::list_tags(
        state.definition_registry.as_ref(),
        &repository,
    ))?;
    Ok(json!({ "tags": tags }))
}

// spec: IMGM#registry-allowlist
pub(crate) fn list_registries(state: &OiState) -> HandlerResult {
    let registries = state
        .db
        .call(registries::list_allowed_registries)
        .map_err(|e| OiError::internal(format!("db error: {e}")))?;
    Ok(json!({ "registries": registries }))
}

// spec: IMGM#registry-allowlist
pub(crate) fn add_registry(state: &OiState, params: RegistryParams) -> HandlerResult {
    let registry = params.registry.clone();
    state
        .db
        .call(move |db| registries::add_allowed_registry(db, &registry))
        .map_err(|e| OiError::internal(format!("db error: {e}")))?;

    // spec: FLT#filing-and-clearing — `disallowed_registry` is a condition fault:
    // true exactly while an app's images reference a registry outside the
    // allowlist. Adding the registry is the natural remediation, so it has to
    // re-evaluate for the same reason removing one does — otherwise the fault
    // the operator just fixed stands until something else happens to reload
    // the app.
    re_evaluate_all_apps(state);

    Ok(json!({ "ok": true }))
}

// spec: IMGM#registry-allowlist
pub(crate) fn remove_registry(state: &OiState, params: RegistryParams) -> HandlerResult {
    let registry = params.registry.clone();
    let removed = state
        .db
        .call(move |db| registries::remove_allowed_registry(db, &registry))
        .map_err(|e| OiError::internal(format!("db error: {e}")))?;
    if !removed {
        return Err(OiError::not_found(format!(
            "registry not in allowlist: {}",
            params.registry
        )));
    }

    re_evaluate_all_apps(state);

    Ok(json!({ "ok": true }))
}

/// Evaluate every installed app's definition again, for a change that moves
/// what its scripts read (a registry's settings, a host constant).
pub fn re_evaluate_all_apps(state: &OiState) {
    let app_names: Vec<AppName> = {
        let reg = state.registry.read();
        reg.names()
    };
    for name in &app_names {
        let name_clone = name.clone();
        let cipher = std::sync::Arc::clone(&state.cipher);
        let loaded_params = match state
            .db
            .call(move |db| apps::load_all_params_for_app(db, &cipher, &name_clone))
        {
            Ok(params) => params,
            Err(e) => {
                tracing::warn!(app = %name, "not re-evaluating: could not read the stored params: {e}");
                continue;
            }
        };
        // Re-evaluating each app's existing script after an allowlist change:
        // nothing downstream diffs the definition, so either outcome is fine.
        let _ = crate::runtime::apps::reload(
            &state.registry,
            &state.db,
            &state.cipher,
            name,
            &loaded_params,
            &state.script_limits,
        );
        let reg = state.registry.read();
        if let Some(entry) = reg.get(name.as_str()) {
            super::apps::sync_fault_state(&state.db, entry);
        }
    }
}

#[cfg(test)]
mod tests;
