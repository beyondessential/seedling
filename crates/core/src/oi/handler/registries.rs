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

// i[registry.list]
pub(crate) fn list_registries(state: &OiState) -> HandlerResult {
    let registries = state
        .db
        .call(registries::list_allowed_registries)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    Ok(json!({ "registries": registries }))
}

// i[registry.add]
pub(crate) fn add_registry(state: &OiState, params: RegistryParams) -> HandlerResult {
    let registry = params.registry.clone();
    state
        .db
        .call(move |db| registries::add_allowed_registry(db, &registry))
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    Ok(json!({ "ok": true }))
}

// i[registry.remove]
pub(crate) fn remove_registry(state: &OiState, params: RegistryParams) -> HandlerResult {
    let registry = params.registry.clone();
    let removed = state
        .db
        .call(move |db| registries::remove_allowed_registry(db, &registry))
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    if !removed {
        return Err(OiError::not_found(format!(
            "registry not in allowlist: {}",
            params.registry
        )));
    }

    re_evaluate_all_apps(state);

    Ok(json!({ "ok": true }))
}

fn re_evaluate_all_apps(state: &OiState) {
    let app_names: Vec<AppName> = {
        let reg = state.registry.read();
        reg.list().into_iter().map(|(name, _)| name).collect()
    };
    for name in &app_names {
        let script = {
            let reg = state.registry.read();
            match reg.get(name.as_str()) {
                Some(entry) => entry.script.clone(),
                None => continue,
            }
        };
        let name_clone = name.clone();
        let cipher = std::sync::Arc::clone(&state.cipher);
        let loaded_params = state
            .db
            .call(move |db| apps::load_all_params_for_app(db, &cipher, &name_clone));
        state
            .registry
            .write()
            .reload(name, script, &loaded_params, &state.script_limits);
        {
            let reg = state.registry.read();
            if let Some(entry) = reg.get(name.as_str()) {
                // Extract data from entry before the db.call since AppEntry is not Send.
                let app_name = entry.name.clone();
                let script_error = entry.script_error.clone();
                let used_registries: std::collections::BTreeSet<String> = {
                    use crate::defs::{container::image_registry, resource::Resource};
                    let def = entry.app.def.load();
                    let mut regs = std::collections::BTreeSet::new();
                    for resource in def.resources.values() {
                        let image = match resource {
                            Resource::Deployment(d) => {
                                let dd = d.def.lock();
                                let pod = dd.pod.lock();
                                pod.container.lock().image.clone()
                            }
                            Resource::Job(j) => {
                                let jd = j.def.lock();
                                let pod = jd.pod.lock();
                                pod.container.lock().image.clone()
                            }
                            _ => None,
                        };
                        if let Some(ref img) = image
                            && let Some(reg) = image_registry(img)
                        {
                            regs.insert(reg.to_owned());
                        }
                    }
                    regs
                };
                state.db.call(move |db| {
                    // Inline sync_script_error_fault logic.
                    {
                        use crate::runtime::faults;
                        let existing: Vec<_> = faults::list_active_faults(db, Some(&app_name))
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|f| f.kind == "script_error")
                            .collect();
                        match &script_error {
                            Some((msg, _)) => {
                                let dominated = existing.iter().any(|f| f.description == *msg);
                                if !dominated {
                                    for f in &existing {
                                        if let Err(e) = faults::clear_fault(db, &f.id, &app_name) {
                                            tracing::warn!(app = %app_name, fault_id = %f.id, "failed to clear stale script-error fault: {e}");
                                        }
                                    }
                                    if let Err(e) = faults::file_fault(db, &app_name, None, None, None, "script_error", msg) {
                                        tracing::warn!(app = %app_name, "failed to file script-error fault: {e}");
                                    }
                                }
                            }
                            None => {
                                for f in &existing {
                                    if let Err(e) = faults::clear_fault(db, &f.id, &app_name) {
                                        tracing::warn!(app = %app_name, fault_id = %f.id, "failed to clear script-error fault: {e}");
                                    }
                                }
                            }
                        }
                    }
                    // Inline sync_registry_faults logic.
                    {
                        use crate::runtime::{faults, registries as reg_mod};
                        let allowed: std::collections::BTreeSet<String> =
                            reg_mod::list_allowed_registries(db)
                                .unwrap_or_default()
                                .into_iter()
                                .collect();
                        let disallowed: Vec<&str> = used_registries
                            .iter()
                            .filter(|r| !allowed.contains(*r))
                            .map(String::as_str)
                            .collect();
                        let existing: Vec<_> = faults::list_active_faults(db, Some(&app_name))
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|f| f.kind == "disallowed_registry")
                            .collect();
                        if disallowed.is_empty() {
                            for f in &existing {
                                if let Err(e) = faults::clear_fault(db, &f.id, &app_name) {
                                    tracing::warn!(app = %app_name, fault_id = %f.id, "failed to clear disallowed_registry fault: {e}");
                                }
                            }
                        } else {
                            let description = format!("image references use disallowed registries: {}", disallowed.join(", "));
                            if !existing.iter().any(|f| f.description == description) {
                                for f in &existing {
                                    if let Err(e) = faults::clear_fault(db, &f.id, &app_name) {
                                        tracing::warn!(app = %app_name, fault_id = %f.id, "failed to clear stale disallowed_registry fault: {e}");
                                    }
                                }
                                if let Err(e) = faults::file_fault(db, &app_name, None, None, None, "disallowed_registry", &description) {
                                    tracing::warn!(app = %app_name, "failed to file disallowed_registry fault: {e}");
                                }
                            }
                        }
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
