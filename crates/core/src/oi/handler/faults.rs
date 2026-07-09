use seedling_protocol::error::{ErrorCode, OiError};
use seedling_protocol::names::AppName;
use serde::Deserialize;
use serde_json::json;

use super::HandlerResult;
use crate::{oi::state::OiState, runtime::faults};

#[derive(Deserialize)]
pub(crate) struct ListFaultsParams {
    pub app: Option<AppName>,
}

// i[fault.list]
pub(crate) fn list_faults(state: &OiState, params: ListFaultsParams) -> HandlerResult {
    let app = params.app.clone();
    let records = state
        .db
        .call(move |db| faults::list_active_faults(db, app.as_ref()))
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db query: {e}")))?;
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
    pub app: AppName,
}

// i[fault.clear-app]
/// Clear every active fault for the given app. Returns the number of faults
/// cleared. The runtime never re-files cleared faults that were derived
/// observationally (e.g. `image_pull_failed`) — they will be re-filed on the
/// next tick if the underlying condition is still present.
pub(crate) fn clear_app_faults(state: &OiState, params: ClearAppFaultsParams) -> HandlerResult {
    let app = params.app.clone();
    let app_name = app.clone();
    let cleared = state
        .db
        .call(move |db| -> rusqlite::Result<usize> {
            let active = faults::list_active_faults(db, Some(&app_name))?;
            let mut count = 0;
            for f in active {
                faults::clear_fault(db, &f.id, &app_name)?;
                count += 1;
            }
            Ok(count)
        })
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("clear faults: {e}")))?;
    Ok(json!({ "app": app, "cleared": cleared }))
}

#[cfg(test)]
mod tests;
