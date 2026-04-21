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
