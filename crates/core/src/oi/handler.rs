use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use seedling_protocol::{
    actor::Actor,
    error::{ErrorCode, HandlerResult, OiError},
};

use super::state::OiState;

pub mod actions;
mod apps;
pub mod backups;
mod faults;
mod key_mgmt;
mod params;
mod registries;
mod status;
mod volumes;

/// Context derived from an incoming OI request, passed through dispatch.
pub struct RequestCtx {
    /// The resolved actor for this request. Always present — synthesised from
    /// the client's mTLS identity when absent from the request JSON.
    pub actor: Arc<Actor>,
}

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
pub fn dispatch(state: &Arc<OiState>, buf: &[u8], ctx: &RequestCtx) -> Vec<u8> {
    let response = match parse_and_dispatch(state, buf, ctx) {
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

fn parse_and_dispatch(state: &Arc<OiState>, buf: &[u8], ctx: &RequestCtx) -> HandlerResult {
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
        "/apps/create" => apps::register_app(state, parse_params(req.params)?, ctx),
        "/apps/remove" => apps::deregister_app(state, parse_params(req.params)?, ctx),
        "/apps/uninstall" => apps::uninstall_app(state, parse_params(req.params)?),
        "/apps/update" => apps::update_app(state, parse_params(req.params)?, ctx),
        // i[scale.set]
        "/apps/scale" => apps::scale_app(state, parse_params(req.params)?, ctx),
        // i[app.script]
        "/apps/script" => apps::get_app_script(state, parse_params(req.params)?),
        // i[generation.history]
        "/apps/generations" => apps::list_generations(state, parse_params(req.params)?),
        // i[plan.dry-run]
        "/apps/plan" => apps::dry_run_plan(state, parse_params(req.params)?),
        // i[param.set]
        "/apps/params/set" => params::set_param(state, parse_params(req.params)?, ctx),
        // i[param.unset]
        "/apps/params/unset" => params::unset_param(state, parse_params(req.params)?, ctx),
        // i[action.invoke]
        "/apps/action/invoke" => actions::invoke_action(state, parse_params(req.params)?, ctx),
        // i[action.invoke.install]
        "/apps/install/invoke" => {
            actions::install::invoke_install(state, parse_params(req.params)?, ctx)
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
        "/volumes/held/list" => volumes::list_held(state),
        "/volumes/held/delete" => volumes::delete_held(state, parse_params(req.params)?),
        "/volumes/exported/list" => volumes::list_exported(state),
        "/volumes/site/create" => volumes::create_site_volume(state, parse_params(req.params)?),
        "/volumes/site/list" => volumes::list_site_volumes(state),
        "/volumes/site/delete" => volumes::delete_site_volume(state, parse_params(req.params)?),
        "/volumes/site/snapshot" => volumes::snapshot_site_volume(state, parse_params(req.params)?),
        "/volumes/external/map" => volumes::map_external_volume(state, parse_params(req.params)?),
        "/volumes/external/unmap" => {
            volumes::unmap_external_volume(state, parse_params(req.params)?)
        }
        "/volumes/external/remap" => {
            volumes::remap_external_volume(state, parse_params(req.params)?)
        }
        "/volumes/external/list" => {
            volumes::list_external_mappings(state, parse_params(req.params)?)
        }
        // i[backup.app.register]
        "/backups/apps/register" => backups::register_backup_app(state, parse_params(req.params)?),
        // i[backup.app.deregister]
        "/backups/apps/deregister" => {
            backups::deregister_backup_app(state, parse_params(req.params)?)
        }
        // i[backup.app.list]
        "/backups/apps/list" => backups::list_backup_apps(state),
        // i[backup.strategy.create]
        "/backups/strategies/create" => backups::create_strategy(state, parse_params(req.params)?),
        // i[backup.strategy.list]
        "/backups/strategies/list" => backups::list_strategies(state),
        // i[backup.strategy.show]
        "/backups/strategies/show" => backups::show_strategy(state, parse_params(req.params)?),
        // i[backup.strategy.update]
        "/backups/strategies/update" => backups::update_strategy(state, parse_params(req.params)?),
        // i[backup.strategy.delete]
        "/backups/strategies/delete" => backups::delete_strategy(state, parse_params(req.params)?),
        // i[backup.run]
        "/backups/run" => backups::run_backup(state, parse_params(req.params)?),
        // i[backup.snapshots.list]
        "/backups/snapshots/list" => backups::list_snapshots(state, parse_params(req.params)?),
        // i[backup.restore]
        "/backups/restore" => backups::restore_backup(state, parse_params(req.params)?),
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
