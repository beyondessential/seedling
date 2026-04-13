use serde_json::{Value, json};

use crate::{
    oi::{error::{ErrorCode, OiError}, state::OiState},
    runtime::faults,
};

use super::HandlerResult;

// i[fault.list]
pub(crate) fn list_faults(state: &OiState, params: Value) -> HandlerResult {
    let app = params.get("app").and_then(Value::as_str);
    let db = state.db.lock();
    let records = faults::list_active_faults(&db, app)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db query: {e}")))?;
    let result: Vec<Value> = records
        .into_iter()
        .map(|f| {
            json!({
                "id": f.id,
                "app": f.app,
                "resource_type": f.resource_type,
                "resource_name": f.resource_name,
                "instance_id": f.instance_id,
                "kind": f.kind,
                "timestamp": f.timestamp.to_rfc3339(),
                "description": f.description,
            })
        })
        .collect();
    Ok(json!(result))
}
