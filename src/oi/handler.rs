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
        "GetStatus" => status::get_status(state),
        // i[app.list]
        "ListApps" => apps::list_apps(state),
        // i[app.describe]
        "DescribeApp" => apps::describe_app(state, req.params),
        "RegisterApp" => apps::register_app(state, req.params),
        "DeregisterApp" => apps::deregister_app(state, req.params),
        "UninstallApp" => apps::uninstall_app(state, req.params),
        "UpdateApp" => apps::update_app(state, req.params),
        // i[param.set]
        "SetParam" => params::set_param(state, req.params),
        // i[param.unset]
        "UnsetParam" => params::unset_param(state, req.params),
        // i[action.invoke]
        "InvokeAction" => actions::invoke_action(state, req.params),
        // i[action.invoke.install]
        "InvokeInstall" => actions::invoke_install(state, req.params),
        // i[key.list]
        "ListKeys" => key_mgmt::list_keys(state),
        // i[key.authorize]
        "AuthorizeKey" => key_mgmt::authorize_key(state, req.params),
        // i[key.revoke]
        "RevokeKey" => key_mgmt::revoke_key(state, req.params),
        // i[shell.resize]
        "ResizeShell" => super::shells::resize_shell(state, req.params),
        // i[shell.list]
        "ListShells" => super::shells::list_shells(state, req.params),
        // i[shell.stop]
        "StopShell" => super::shells::stop_shell(state, req.params),
        // i[forward.list]
        "ListForwards" => super::forwards::handler::list_forwards(state, req.params),
        // i[forward.stop]
        "StopForward" => super::forwards::handler::stop_forward(state, req.params),
        // i[fault.list]
        "ListFaults" => faults::list_faults(state, req.params),
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
