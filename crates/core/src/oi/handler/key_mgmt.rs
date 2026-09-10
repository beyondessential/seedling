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
    // The verifier compares byte-for-byte against a lowercase-hex SHA-256, so
    // an uppercase, whitespace-padded, `sha256:`-prefixed or wrong-length
    // value was stored and listed as authorised while never matching any
    // client. Normalise what can be, and refuse what cannot.
    let fingerprint = seedling_protocol::keys::parse_fingerprint(&params.fingerprint)
        .map_err(|e| OiError::new(ErrorCode::RequirementsInvalid, e.to_string()))?;
    let trusted_keys = Arc::clone(&state.trusted_keys);
    let label = params.label.clone();
    let fingerprint_for_db = fingerprint.clone();
    state
        .db
        .call(move |db| {
            crate::oi::auth::authorize_key(db, &trusted_keys, &fingerprint_for_db, &label)
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    tracing::info!(fingerprint = %fingerprint, label = %params.label, "authorized key");
    Ok(json!({}))
}

// i[key.revoke]
pub(crate) fn revoke_key(state: &OiState, params: RevokeKeyParams) -> HandlerResult {
    // Normalised the same way as on the way in, so a key authorised from a
    // `sha256:`-prefixed or uppercase paste can be revoked with the string
    // the operator actually typed.
    let fingerprint = seedling_protocol::keys::parse_fingerprint(&params.fingerprint)
        .map_err(|e| OiError::new(ErrorCode::RequirementsInvalid, e.to_string()))?;
    let trusted_keys = Arc::clone(&state.trusted_keys);
    let fingerprint_for_db = fingerprint.clone();
    let removed = state
        .db
        .call(move |db| crate::oi::auth::revoke_key(db, &trusted_keys, &fingerprint_for_db))
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    if removed {
        tracing::info!(fingerprint = %fingerprint, "revoked key");
        Ok(json!({}))
    } else {
        Err(OiError::not_found(format!("key not found: {fingerprint}")))
    }
}

#[cfg(test)]
mod tests;
