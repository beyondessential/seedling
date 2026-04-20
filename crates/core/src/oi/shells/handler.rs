use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};

use crate::oi::state::OiState;

#[derive(Deserialize)]
pub(crate) struct ResizeShellParams {
    pub session_id: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Deserialize)]
pub(crate) struct ListShellsParams {
    pub app: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct StopShellParams {
    pub session_id: String,
}

// i[shell.resize]
pub(crate) fn resize_shell(state: &Arc<OiState>, params: ResizeShellParams) -> HandlerResult {
    let id = Uuid::parse_str(&params.session_id)
        .map_err(|_| OiError::new(ErrorCode::RequirementsInvalid, "invalid session_id"))?;
    if !state.shells.resize(&id, params.rows, params.cols) {
        return Err(OiError::not_found(format!(
            "session not found: {}",
            params.session_id
        )));
    }
    Ok(json!({}))
}

// i[shell.list]
pub(crate) fn list_shells(state: &Arc<OiState>, params: ListShellsParams) -> HandlerResult {
    let records = state.shells.list(params.app.as_deref());
    let list: Vec<serde_json::Value> = records
        .iter()
        .map(|r| {
            json!({
                "session_id": r.session_id.to_string(),
                "app": r.app,
                "name": r.name,
                "opened_at": r.opened_at.to_string(),
                "actor": r.actor,
            })
        })
        .collect();
    Ok(json!({ "shells": list }))
}

// i[shell.stop]
pub(crate) fn stop_shell(state: &Arc<OiState>, params: StopShellParams) -> HandlerResult {
    let id = Uuid::parse_str(&params.session_id)
        .map_err(|_| OiError::new(ErrorCode::RequirementsInvalid, "invalid session_id"))?;
    if !state.shells.stop(&id) {
        return Err(OiError::not_found(format!(
            "session not found: {}",
            params.session_id
        )));
    }
    Ok(json!({}))
}
