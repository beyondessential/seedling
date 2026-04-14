use std::sync::Arc;

use serde::de::DeserializeOwned;
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
mod registries;
mod status;

fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, OiError> {
    serde_json::from_value(params).map_err(|e| {
        OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("invalid params: {e}"),
        )
    })
}

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
        "/apps/show" => apps::describe_app(state, parse_params(req.params)?),
        "/apps/create" => apps::register_app(state, parse_params(req.params)?),
        "/apps/remove" => apps::deregister_app(state, parse_params(req.params)?),
        "/apps/uninstall" => apps::uninstall_app(state, parse_params(req.params)?),
        "/apps/update" => apps::update_app(state, parse_params(req.params)?),
        // i[app.script]
        "/apps/script" => apps::get_app_script(state, parse_params(req.params)?),
        // i[param.set]
        "/apps/params/set" => params::set_param(state, parse_params(req.params)?),
        // i[param.unset]
        "/apps/params/unset" => params::unset_param(state, parse_params(req.params)?),
        // i[action.invoke]
        "/apps/action/invoke" => actions::invoke_action(state, parse_params(req.params)?),
        // i[action.invoke.install]
        "/apps/install/invoke" => {
            actions::install::invoke_install(state, parse_params(req.params)?)
        }
        // i[key.list]
        "/keys/list" => key_mgmt::list_keys(state),
        // i[key.authorize]
        "/keys/authorise" => key_mgmt::authorize_key(state, parse_params(req.params)?),
        // i[key.revoke]
        "/keys/revoke" => key_mgmt::revoke_key(state, parse_params(req.params)?),
        // i[shell.resize]
        "/shells/resize" => super::shells::resize_shell(state, parse_params(req.params)?),
        // i[shell.list]
        "/shells/list" => super::shells::list_shells(state, parse_params(req.params)?),
        // i[shell.stop]
        "/shells/stop" => super::shells::stop_shell(state, parse_params(req.params)?),
        // i[forward.list]
        "/forwards/list" => {
            super::forwards::handler::list_forwards(state, parse_params(req.params)?)
        }
        // i[forward.stop]
        "/forwards/stop" => {
            super::forwards::handler::stop_forward(state, parse_params(req.params)?)
        }
        // i[fault.list]
        "/faults/list" => faults::list_faults(state, parse_params(req.params)?),
        // i[registry.list]
        "/registries/list" => registries::list_registries(state),
        // i[registry.add]
        "/registries/add" => registries::add_registry(state, parse_params(req.params)?),
        // i[registry.remove]
        "/registries/remove" => registries::remove_registry(state, parse_params(req.params)?),
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
