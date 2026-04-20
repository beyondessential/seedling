use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use seedling_protocol::error::{ErrorCode, OiError};

use crate::oi::state::OiState;

use super::HandlerResult;

#[derive(Deserialize)]
pub(crate) struct AuthorizeKeyParams {
    pub fingerprint: String,
    #[serde(default = "default_label")]
    pub label: String,
}

fn default_label() -> String {
    "unnamed".to_owned()
}

#[derive(Deserialize)]
pub(crate) struct RevokeKeyParams {
    pub fingerprint: String,
}

// i[key.list]
pub(crate) fn list_keys(state: &OiState) -> HandlerResult {
    let rows = state
        .db
        .call(crate::oi::auth::list_keys)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(fp, label, added_at)| {
            json!({ "fingerprint": fp, "label": label, "added_at": added_at })
        })
        .collect();
    Ok(json!(result))
}

// i[key.authorize]
pub(crate) fn authorize_key(state: &OiState, params: AuthorizeKeyParams) -> HandlerResult {
    let trusted_keys = Arc::clone(&state.trusted_keys);
    let fingerprint = params.fingerprint.clone();
    let label = params.label.clone();
    state
        .db
        .call(move |db| crate::oi::auth::authorize_key(db, &trusted_keys, &fingerprint, &label))
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    tracing::info!(fingerprint = %params.fingerprint, label = %params.label, "authorized key");
    Ok(json!({}))
}

// i[key.revoke]
pub(crate) fn revoke_key(state: &OiState, params: RevokeKeyParams) -> HandlerResult {
    let trusted_keys = Arc::clone(&state.trusted_keys);
    let fingerprint = params.fingerprint.clone();
    let removed = state
        .db
        .call(move |db| crate::oi::auth::revoke_key(db, &trusted_keys, &fingerprint))
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    if removed {
        tracing::info!(fingerprint = %params.fingerprint, "revoked key");
        Ok(json!({}))
    } else {
        Err(OiError::not_found(format!(
            "key not found: {}",
            params.fingerprint
        )))
    }
}
