use std::sync::Arc;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::oi::{
    error::{ErrorCode, HandlerResult, OiError},
    state::OiState,
};

// i[shell.resize]
pub(crate) fn resize_shell(state: &Arc<OiState>, params: Value) -> HandlerResult {
    let id_str = params
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: session_id"))?;
    let id = Uuid::parse_str(id_str)
        .map_err(|_| OiError::new(ErrorCode::RequirementsInvalid, "invalid session_id"))?;
    let rows = params
        .get("rows")
        .and_then(Value::as_u64)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: rows"))?
        as u16;
    let cols = params
        .get("cols")
        .and_then(Value::as_u64)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: cols"))?
        as u16;
    if !state.shells.resize(&id, rows, cols) {
        return Err(OiError::not_found(format!("session not found: {id_str}")));
    }
    Ok(json!({}))
}

// i[shell.list]
pub(crate) fn list_shells(state: &Arc<OiState>, params: Value) -> HandlerResult {
    let app = params.get("app").and_then(Value::as_str);
    let records = state.shells.list(app);
    let list: Vec<Value> = records
        .iter()
        .map(|r| {
            json!({
                "session_id": r.session_id.to_string(),
                "app": r.app,
                "name": r.name,
                "opened_at": r.opened_at.to_string(),
            })
        })
        .collect();
    Ok(json!({ "shells": list }))
}

// i[shell.stop]
pub(crate) fn stop_shell(state: &Arc<OiState>, params: Value) -> HandlerResult {
    let id_str = params
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: session_id"))?;
    let id = Uuid::parse_str(id_str)
        .map_err(|_| OiError::new(ErrorCode::RequirementsInvalid, "invalid session_id"))?;
    if !state.shells.stop(&id) {
        return Err(OiError::not_found(format!("session not found: {id_str}")));
    }
    Ok(json!({}))
}
