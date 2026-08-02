use seedling_protocol::error::{ErrorCode, OiError};
use seedling_protocol::names::AppName;
use serde::Deserialize;
use serde_json::json;

use super::HandlerResult;
use crate::{oi::state::OiState, runtime::restarts};

/// Records returned when the caller does not ask for a specific number, and
/// the ceiling on what it may ask for. The cap keeps a single request from
/// pulling the whole retained history of a busy host over the wire.
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

#[derive(Deserialize)]
pub(crate) struct ListRestartsParams {
    pub app: Option<AppName>,
    pub instance: Option<String>,
    pub limit: Option<usize>,
}

// i[impl restart.list]
pub(crate) fn list_restarts(state: &OiState, params: ListRestartsParams) -> HandlerResult {
    let ListRestartsParams {
        app,
        instance,
        limit,
    } = params;
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let records = state
        .db
        .call(move |db| restarts::list(db, app.as_ref(), instance.as_deref(), limit))
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db query: {e}")))?;

    // i[impl restart.record]
    let result: Vec<serde_json::Value> = records
        .into_iter()
        .map(|r| {
            json!({
                "id": r.id,
                "app": r.app,
                "instance_id": r.instance_id,
                "resource_type": r.resource_type,
                "resource_name": r.resource_name,
                "generation": r.generation,
                "timestamp": r.timestamp.to_string(),
                "cause": r.cause,
                "exit_code": r.exit_code,
                "exit_kind": r.exit_kind,
            })
        })
        .collect();
    Ok(json!(result))
}

// i[impl restart.settings]
pub(crate) fn get_settings(state: &OiState) -> HandlerResult {
    let s = state
        .db
        .call(restarts::settings)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db query: {e}")))?;
    Ok(json!({ "threshold": s.threshold, "window_secs": s.window_secs }))
}

#[derive(Deserialize)]
pub(crate) struct SetSettingsParams {
    #[serde(default)]
    pub threshold: Option<i64>,
    #[serde(default)]
    pub window_secs: Option<i64>,
}

// i[impl restart.settings]
pub(crate) fn set_settings(state: &OiState, params: SetSettingsParams) -> HandlerResult {
    let SetSettingsParams {
        threshold,
        window_secs,
    } = params;
    let s = state
        .db
        .call(move |db| restarts::set_settings(db, threshold, window_secs))
        .map_err(|e| OiError::new(ErrorCode::RequirementsInvalid, e.to_string()))?;
    Ok(json!({ "threshold": s.threshold, "window_secs": s.window_secs }))
}

#[cfg(test)]
mod tests;
