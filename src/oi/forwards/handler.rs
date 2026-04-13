use std::sync::Arc;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::oi::{
    error::{ErrorCode, HandlerResult, OiError},
    state::OiState,
};

use super::registry::ForwardId;

// i[forward.list]
pub(crate) fn list_forwards(state: &Arc<OiState>, params: Value) -> HandlerResult {
    let app = params.get("app").and_then(Value::as_str);
    let records = state.forwards.lock().list(app);
    let list: Vec<Value> = records
        .iter()
        .map(|r| {
            json!({
                "forward_id": r.forward_id.to_string(),
                "app": r.app,
                "service": r.service,
                "port": r.port,
                "proto": r.proto,
                "opened_at": r.opened_at.to_rfc3339(),
            })
        })
        .collect();
    Ok(json!({ "forwards": list }))
}

// i[forward.stop]
pub(crate) fn stop_forward(state: &Arc<OiState>, params: Value) -> HandlerResult {
    let id_str = params
        .get("forward_id")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::NotFound, "missing param: forward_id"))?;
    let forward_id: ForwardId = Uuid::parse_str(id_str)
        .map_err(|_| OiError::not_found(format!("invalid forward_id: {id_str}")))?;
    let entry = state
        .forwards
        .lock()
        .remove(&forward_id)
        .ok_or_else(|| OiError::not_found(format!("forward not found: {id_str}")))?;
    let _ = entry.stop_tx.send(true);
    tracing::info!(forward_id = %forward_id, "stopped forward");
    Ok(json!({}))
}
