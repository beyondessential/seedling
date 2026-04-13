use serde_json::{Value, json};

use crate::oi::{
    error::{ErrorCode, OiError},
    state::OiState,
};

use super::HandlerResult;

// i[key.list]
pub(crate) fn list_keys(state: &OiState) -> HandlerResult {
    let db = state.db.lock();
    let rows = crate::oi::auth::list_keys(&db)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    let result: Vec<Value> = rows
        .into_iter()
        .map(|(fp, label, added_at)| {
            json!({ "fingerprint": fp, "label": label, "added_at": added_at })
        })
        .collect();
    Ok(json!(result))
}

// i[key.authorize]
pub(crate) fn authorize_key(state: &OiState, params: Value) -> HandlerResult {
    let fp = params
        .get("fingerprint")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OiError::new(ErrorCode::RequirementsInvalid, "missing param: fingerprint")
        })?;
    let label = params
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("unnamed");
    let db = state.db.lock();
    crate::oi::auth::authorize_key(&db, &state.trusted_keys, fp, label)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    tracing::info!(fingerprint = %fp, label = %label, "authorized key");
    Ok(json!({}))
}

// i[key.revoke]
pub(crate) fn revoke_key(state: &OiState, params: Value) -> HandlerResult {
    let fp = params
        .get("fingerprint")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::NotFound, "missing param: fingerprint"))?;
    let db = state.db.lock();
    let removed = crate::oi::auth::revoke_key(&db, &state.trusted_keys, fp)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    if removed {
        tracing::info!(fingerprint = %fp, "revoked key");
        Ok(json!({}))
    } else {
        Err(OiError::not_found(format!("key not found: {fp}")))
    }
}
