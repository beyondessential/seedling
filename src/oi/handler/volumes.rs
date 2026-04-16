use serde::Deserialize;
use serde_json::json;

use crate::oi::{
    error::{ErrorCode, HandlerResult, OiError},
    state::OiState,
};

pub(crate) fn list_held(state: &OiState) -> HandlerResult {
    let held = state.driver.volume_store.list_held().map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to list held volumes: {e}"),
        )
    })?;

    let items: Vec<_> = held
        .iter()
        .map(|h| {
            json!({
                "id": h.id,
                "app": h.app,
                "volume_name": h.volume_name,
                "display_name": h.display_name,
                "reason": h.reason,
                "held_at": h.held_at,
            })
        })
        .collect();

    Ok(json!(items))
}

#[derive(Deserialize)]
pub(crate) struct DeleteHeldParams {
    pub id: String,
}

pub(crate) fn delete_held(state: &OiState, params: DeleteHeldParams) -> HandlerResult {
    // confirm_delete_held is async, but OI handlers are sync.
    // Use block_in_place to run the async operation.
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            state
                .driver
                .volume_store
                .confirm_delete_held(&params.id)
                .await
                .map_err(|e| {
                    OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("failed to delete held volume: {e}"),
                    )
                })
        })
    })?;

    Ok(json!({ "deleted": true }))
}
