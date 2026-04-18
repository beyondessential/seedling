use serde::Deserialize;
use serde_json::json;

use seedling_protocol::error::{ErrorCode, OiError};

use crate::{oi::state::OiState, runtime::faults};

use super::HandlerResult;

#[derive(Deserialize)]
pub(crate) struct ListFaultsParams {
    pub app: Option<String>,
}

// i[fault.list]
pub(crate) fn list_faults(state: &OiState, params: ListFaultsParams) -> HandlerResult {
    let app = params.app.as_deref();
    let db = state.db.lock();
    let records = faults::list_active_faults(&db, app)
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
