use seedling_protocol::error::{ErrorCode, OiError};
use seedling_protocol::names::AppName;
use serde::Deserialize;
use serde_json::json;

use super::{HandlerResult, RequestCtx};
use crate::{
    oi::state::OiState,
    runtime::{breakers, faults},
};

#[derive(Deserialize)]
pub(crate) struct ListFaultsParams {
    pub app: Option<String>,
}

/// The names faults are filed against that are not registered apps.
const HOST_FAULT_APPS: [&str; 2] = ["_system", "seedling"];

/// The app name a fault request names: a registered app, `_system` or
/// `seedling`.
fn fault_app(state: &OiState, raw: &str) -> Result<AppName, OiError> {
    if HOST_FAULT_APPS.contains(&raw) {
        return Ok(AppName::new_unchecked(raw));
    }
    let name = AppName::new(raw).map_err(|e| {
        OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("invalid app name {raw:?}: {e}"),
        )
    })?;
    if !state.registry.read().is_registered(name.as_str()) {
        return Err(OiError::not_found(format!("app not found: {raw}")));
    }
    Ok(name)
}

// spec: FLTS#listing-faults
pub(crate) fn list_faults(state: &OiState, params: ListFaultsParams) -> HandlerResult {
    let app = params
        .app
        .as_deref()
        .map(|raw| fault_app(state, raw))
        .transpose()?;
    let records = state
        .db
        .call(move |db| faults::list_active_faults(db, app.as_ref()))
        .map_err(|e| OiError::internal(format!("db query: {e}")))?;
    let result: Vec<serde_json::Value> = records
        .into_iter()
        .map(|f| {
            json!({
                "id": f.id,
                "app": f.app,
                "resource_type": f.resource_type,
                "resource_name": f.resource_name,
                "instance_id": f.instance_id,
                "kind": f.kind,
                "timestamp": f.timestamp.to_string(),
                "description": f.description,
            })
        })
        .collect();
    Ok(json!(result))
}

#[derive(Deserialize)]
pub(crate) struct ClearAppFaultsParams {
    pub app: String,
}

// spec: FLTS#clearing-faults
/// Clear every active fault filed against the given name: a registered app,
/// `_system` or `seedling`. Returns the number of faults cleared. A cleared
/// fault whose condition still holds is filed again by the runtime.
pub(crate) fn clear_app_faults(
    state: &OiState,
    params: ClearAppFaultsParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let app = fault_app(state, &params.app)?;
    let app_name = app.clone();
    let actor = std::sync::Arc::clone(&ctx.events.actor);
    let (cleared, definition_reset) = state
        .db
        .call(move |db| -> rusqlite::Result<(usize, bool)> {
            let active = faults::list_active_faults(db, Some(&app_name))?;
            let mut count = 0;
            let mut definition_reset = false;
            for f in active {
                faults::clear_fault_by(db, &f.id, &app_name, Some(std::sync::Arc::clone(&actor)))?;
                // spec: LIM#breakers
                // Clearing a breaker's fault is what resets it.
                definition_reset |= f.kind == breakers::EVALUATION_RUNAWAY
                    && breakers::Thing::from_subject(&f.subject)
                        == Some(breakers::Thing::Definition);
                count += 1;
            }
            Ok((count, definition_reset))
        })
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("clear faults: {e}")))?;
    // spec: LIM#breakers
    // The definition's breaker reset, so the definition is evaluated again
    // now rather than at whatever next asks for it.
    if definition_reset {
        super::params::reevaluate_current(state, &app)?;
    }
    Ok(json!({ "app": app, "cleared": cleared }))
}

#[cfg(test)]
mod tests;
