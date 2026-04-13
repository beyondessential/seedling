use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::oi::{
    error::{HandlerResult, OiError},
    state::OiState,
};

use super::registry::ForwardId;

#[derive(Deserialize)]
pub(crate) struct ListForwardsParams {
    pub app: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct StopForwardParams {
    pub forward_id: String,
}

// i[forward.list]
pub(crate) fn list_forwards(state: &Arc<OiState>, params: ListForwardsParams) -> HandlerResult {
    let app = params.app.as_deref();
    let records = state.forwards.lock().list(app);
    let list: Vec<serde_json::Value> = records
        .iter()
        .map(|r| {
            json!({
                "forward_id": r.forward_id.to_string(),
                "app": r.app,
                "service": r.service,
                "port": r.port,
                "proto": r.proto,
                "opened_at": r.opened_at.to_string(),
            })
        })
        .collect();
    Ok(json!({ "forwards": list }))
}

// i[forward.stop]
pub(crate) fn stop_forward(state: &Arc<OiState>, params: StopForwardParams) -> HandlerResult {
    let id_str = &params.forward_id;
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
