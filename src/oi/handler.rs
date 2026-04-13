use std::sync::Arc;

use serde_json::{Value, json};

use super::{
    error::{ErrorCode, HandlerResult, OiError},
    state::OiState,
};

mod actions;
mod apps;
mod faults;
mod key_mgmt;
mod params;
mod status;

/// Parse the newline-terminated JSON request from `buf`, dispatch to a handler,
/// and return the serialised JSON response (no trailing newline).
pub fn dispatch(state: &Arc<OiState>, buf: &[u8]) -> Vec<u8> {
    let response = match parse_and_dispatch(state, buf) {
        Ok(result) => json!({ "result": result }),
        Err(e) => json!({
            "error": {
                "code": e.code,
                "message": e.message,
            }
        }),
    };
    serde_json::to_vec(&response).expect("response serialisation never fails")
}

fn parse_and_dispatch(state: &Arc<OiState>, buf: &[u8]) -> HandlerResult {
    #[derive(serde::Deserialize)]
    struct Request {
        method: String,
        #[serde(default)]
        params: Value,
    }
    let req: Request = serde_json::from_slice(buf)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("invalid request: {e}")))?;

    let result = match req.method.as_str() {
        // i[status.get]
        "/server/status" => status::get_status(state),
        // i[app.list]
        "/apps/list" => apps::list_apps(state),
        // i[app.describe]
        "/apps/show" => apps::describe_app(state, req.params),
        "/apps/create" => apps::register_app(state, req.params),
        "/apps/remove" => apps::deregister_app(state, req.params),
        "/apps/uninstall" => apps::uninstall_app(state, req.params),
        "/apps/update" => apps::update_app(state, req.params),
        // i[param.set]
        "/apps/params/set" => {
            let p: params::SetParamParams = serde_json::from_value(req.params).map_err(|e| {
                OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!("invalid params: {e}"),
                )
            })?;
            params::set_param(state, p)
        }
        // i[param.unset]
        "/apps/params/unset" => {
            let p: params::UnsetParamParams = serde_json::from_value(req.params).map_err(|e| {
                OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!("invalid params: {e}"),
                )
            })?;
            params::unset_param(state, p)
        }
        // i[action.invoke]
        "/apps/action/invoke" => {
            let p: actions::InvokeActionParams =
                serde_json::from_value(req.params).map_err(|e| {
                    OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("invalid params: {e}"),
                    )
                })?;
            actions::invoke_action(state, p)
        }
        // i[action.invoke.install]
        "/apps/install/invoke" => {
            let p: actions::install::InvokeInstallParams = serde_json::from_value(req.params)
                .map_err(|e| {
                    OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("invalid params: {e}"),
                    )
                })?;
            actions::install::invoke_install(state, p)
        }
        // i[key.list]
        "/keys/list" => key_mgmt::list_keys(state),
        // i[key.authorize]
        "/keys/authorise" => {
            let p: key_mgmt::AuthorizeKeyParams =
                serde_json::from_value(req.params).map_err(|e| {
                    OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("invalid params: {e}"),
                    )
                })?;
            key_mgmt::authorize_key(state, p)
        }
        // i[key.revoke]
        "/keys/revoke" => {
            let p: key_mgmt::RevokeKeyParams = serde_json::from_value(req.params).map_err(|e| {
                OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!("invalid params: {e}"),
                )
            })?;
            key_mgmt::revoke_key(state, p)
        }
        // i[shell.resize]
        "/shells/resize" => {
            let p: super::shells::handler::ResizeShellParams = serde_json::from_value(req.params)
                .map_err(|e| {
                OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!("invalid params: {e}"),
                )
            })?;
            super::shells::resize_shell(state, p)
        }
        // i[shell.list]
        "/shells/list" => {
            let p: super::shells::handler::ListShellsParams = serde_json::from_value(req.params)
                .map_err(|e| {
                    OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("invalid params: {e}"),
                    )
                })?;
            super::shells::list_shells(state, p)
        }
        // i[shell.stop]
        "/shells/stop" => {
            let p: super::shells::handler::StopShellParams = serde_json::from_value(req.params)
                .map_err(|e| {
                    OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("invalid params: {e}"),
                    )
                })?;
            super::shells::stop_shell(state, p)
        }
        // i[forward.list]
        "/forwards/list" => {
            let p: super::forwards::handler::ListForwardsParams =
                serde_json::from_value(req.params).map_err(|e| {
                    OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("invalid params: {e}"),
                    )
                })?;
            super::forwards::handler::list_forwards(state, p)
        }
        // i[forward.stop]
        "/forwards/stop" => {
            let p: super::forwards::handler::StopForwardParams = serde_json::from_value(req.params)
                .map_err(|e| {
                    OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("invalid params: {e}"),
                    )
                })?;
            super::forwards::handler::stop_forward(state, p)
        }
        // i[fault.list]
        "/faults/list" => {
            let p: faults::ListFaultsParams = serde_json::from_value(req.params).map_err(|e| {
                OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!("invalid params: {e}"),
                )
            })?;
            faults::list_faults(state, p)
        }
        other => Err(OiError::not_found(format!("unknown method: {other}"))),
    };

    match &result {
        Ok(_) => tracing::info!(method = %req.method, "ok"),
        Err(e) => tracing::info!(
            method = %req.method,
            code = ?e.code,
            error = %e.message,
            "error",
        ),
    }

    result
}
