//! OI surface for the Canopy link: enrolment, status, and deregistration.
//! The reporting behaviour lives in [`crate::runtime::canopy`].

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};

use super::OiState;
use crate::runtime::canopy::{CanopyError, CanopyProvider};

fn provider(state: &OiState) -> Result<&Arc<CanopyProvider>, OiError> {
    state.canopy_provider.as_ref().ok_or_else(|| {
        OiError::new(
            ErrorCode::Internal,
            "canopy reporting is not available on this instance",
        )
    })
}

fn to_oi_error(e: CanopyError) -> OiError {
    let code = match &e {
        CanopyError::AlreadyEnrolled => ErrorCode::AlreadyEnrolled,
        CanopyError::InvalidTicket { .. }
        | CanopyError::Decrypt { .. }
        | CanopyError::Rejected { .. } => ErrorCode::RequirementsInvalid,
        CanopyError::Internal { .. } => ErrorCode::Internal,
    };
    OiError::new(code, e.to_string())
}

#[derive(Deserialize)]
pub(crate) struct EnrolParams {
    ticket: String,
    passphrase: String,
}

// i[canopy.enrol]
pub(crate) async fn enrol(state: &Arc<OiState>, params: EnrolParams) -> HandlerResult {
    let (server_id, device_id) = provider(state)?
        .enrol(&params.ticket, &params.passphrase)
        .await
        .map_err(to_oi_error)?;
    Ok(json!({ "server_id": server_id, "device_id": device_id }))
}

// i[impl canopy.status]
pub(crate) fn status(state: &OiState) -> HandlerResult {
    let provider = provider(state)?;
    let mut body = serde_json::Map::new();
    match provider.registration_info() {
        Some(info) => {
            body.insert("enrolled".into(), true.into());
            body.insert("server_id".into(), info.server_id.into());
            if let Some(device_id) = info.device_id {
                body.insert("device_id".into(), device_id.into());
            }
            body.insert("api_url".into(), info.api_url.into());
        }
        None => {
            body.insert("enrolled".into(), false.into());
        }
    }
    let push = provider.push_status();
    if let Some(at) = push.last_push_at {
        body.insert("last_push_at".into(), at.to_string().into());
    }
    if let Some(err) = push.last_push_error {
        body.insert("last_push_error".into(), err.into());
    }
    if let Some(response) = push.last_response {
        body.insert("last_response".into(), response);
    }
    Ok(body.into())
}

// i[canopy.deregister]
pub(crate) async fn deregister(state: &Arc<OiState>) -> HandlerResult {
    let deregistered = provider(state)?
        .deregister()
        .await
        .map_err(to_oi_error)?;
    Ok(json!({ "deregistered": deregistered }))
}

#[cfg(test)]
mod tests;
