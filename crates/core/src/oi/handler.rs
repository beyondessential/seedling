use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use seedling_protocol::{
    error::{ErrorCode, HandlerResult, OiError},
    events::EventSenderWithActor,
};

use super::state::OiState;

pub mod actions;
mod appdef_json;
mod apps;
pub mod backups;
mod faults;
mod images;
mod key_mgmt;
mod params;
mod registries;
mod services;
mod status;
mod templates;
mod tls;
mod volumes;

pub(crate) use status::get_infra_status;

/// Context derived from an incoming OI request, passed through dispatch.
pub struct RequestCtx {
    pub events: EventSenderWithActor,
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
        // i[impl wire.response.ok]
        Ok(result) => json!({ "result": result }),
        // i[impl wire.response.error]
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
    // i[impl wire.request]
    #[derive(serde::Deserialize)]
    struct Request {
        method: String,
        #[serde(default)]
        params: Value,
    }
    let req: Request = serde_json::from_slice(buf)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("invalid request: {e}")))?;

    let result = match req.method.as_str() {
        // i[status.ping]
        "/server/ping" => status::ping(),
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
        // i[deployment.restart]
        "/apps/restart" => apps::restart_deployment(state, parse_params(req.params)?, ctx),
        // i[resource.stop]
        "/apps/resource/stop" => apps::stop_resource(state, parse_params(req.params)?, ctx),
        // i[resource.unstop]
        "/apps/resource/unstop" => apps::unstop_resource(state, parse_params(req.params)?, ctx),
        // i[resource.unstop-all]
        "/apps/unstop" => apps::unstop_all_resources(state, parse_params(req.params)?, ctx),
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
        // i[action.cancel]
        "/apps/action/cancel" => actions::cancel_action(state, parse_params(req.params)?),
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
        // i[fault.clear-app]
        "/faults/clear" => faults::clear_app_faults(state, parse_params(req.params)?),
        // i[registry.list]
        "/registries/list" => registries::list_registries(state),
        // i[registry.add]
        "/registries/add" => registries::add_registry(state, parse_params(req.params)?),
        // i[registry.remove]
        "/registries/remove" => registries::remove_registry(state, parse_params(req.params)?),
        // i[image.list]
        "/images/list" => images::list_images(state),
        // i[image.pull]
        "/images/pull" => images::pull_image(state, parse_params(req.params)?),
        // i[image.remove]
        "/images/remove" => images::remove_image(state, parse_params(req.params)?),
        // i[image.pin.list]
        "/images/pins/list" => images::list_pins(state, parse_params(req.params)?),
        // i[image.pin.clear]
        "/images/pins/clear" => images::clear_pins(state, parse_params(req.params)?),
        // i[image.discover]
        "/apps/images/discover" => images::discover_images(state, parse_params(req.params)?),
        "/volumes/held/list" => volumes::list_held(state),
        "/volumes/held/delete" => volumes::delete_held(state, parse_params(req.params)?, ctx),
        "/volumes/exported/list" => volumes::list_exported(state),
        "/volumes/app/list" => volumes::list_app_volumes(state),
        "/volumes/site/create" => {
            volumes::create_site_volume(state, parse_params(req.params)?, ctx)
        }
        "/volumes/site/list" => volumes::list_site_volumes(state),
        "/volumes/site/delete" => {
            volumes::delete_site_volume(state, parse_params(req.params)?, ctx)
        }
        "/volumes/site/snapshot" => {
            volumes::snapshot_site_volume(state, parse_params(req.params)?, ctx)
        }
        "/volumes/site/promote" => {
            volumes::promote_site_volume(state, parse_params(req.params)?, ctx)
        }
        "/volumes/external/map" => {
            volumes::map_external_volume(state, parse_params(req.params)?, ctx)
        }
        "/volumes/external/unmap" => {
            volumes::unmap_external_volume(state, parse_params(req.params)?, ctx)
        }
        "/volumes/external/remap" => {
            volumes::remap_external_volume(state, parse_params(req.params)?, ctx)
        }
        "/volumes/external/list" => {
            volumes::list_external_mappings(state, parse_params(req.params)?)
        }
        "/volumes/external/declared" => volumes::list_declared_external_volumes(state),
        "/services/exported/list" => services::list_exported(state),
        "/services/app/list" => services::list_app_services(state),
        "/services/site/create" => {
            services::create_site_service(state, parse_params(req.params)?, ctx)
        }
        "/services/site/list" => services::list_site_services(state),
        "/services/site/delete" => {
            services::delete_site_service(state, parse_params(req.params)?, ctx)
        }
        "/services/site/endpoint/add" => {
            services::add_site_service_endpoint(state, parse_params(req.params)?, ctx)
        }
        "/services/site/endpoint/remove" => {
            services::remove_site_service_endpoint(state, parse_params(req.params)?, ctx)
        }
        "/services/external/map" => {
            services::map_external_service(state, parse_params(req.params)?, ctx)
        }
        "/services/external/unmap" => {
            services::unmap_external_service(state, parse_params(req.params)?, ctx)
        }
        "/services/external/remap" => {
            services::remap_external_service(state, parse_params(req.params)?, ctx)
        }
        "/services/external/list" => {
            services::list_external_mappings(state, parse_params(req.params)?)
        }
        "/services/external/declared" => services::list_declared_external_services(state),
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
        // i[template.create]
        "/templates/create" => templates::create_template(state, parse_params(req.params)?, ctx),
        // i[template.list]
        "/templates/list" => templates::list_templates(state),
        // i[template.show]
        "/templates/show" => templates::show_template(state, parse_params(req.params)?),
        // i[template.update]
        "/templates/update" => templates::update_template(state, parse_params(req.params)?, ctx),
        // i[template.remove]
        "/templates/remove" => templates::remove_template(state, parse_params(req.params)?, ctx),
        // i[template.preview]
        "/templates/preview" => templates::preview_template(state, parse_params(req.params)?),
        // i[template.instantiate]
        "/templates/instantiate" => {
            templates::instantiate_template(state, parse_params(req.params)?, ctx)
        }
        // i[tls.dns-provider.list]
        "/tls/dns-providers/list" => tls::list_dns_providers(state),
        // i[tls.dns-provider.upsert]
        "/tls/dns-providers/upsert" => tls::upsert_dns_provider(state, parse_params(req.params)?),
        // i[tls.dns-provider.delete]
        "/tls/dns-providers/delete" => tls::delete_dns_provider(state, parse_params(req.params)?),
        // i[tls.policy.list]
        "/tls/policies/list" => tls::list_policies(state),
        // i[tls.policy.set-acme-dns]
        "/tls/policies/set-acme-dns" => tls::set_policy_acme_dns(state, parse_params(req.params)?),
        // i[tls.policy.set-manual]
        "/tls/policies/set-manual" => tls::set_policy_manual(state, parse_params(req.params)?),
        // i[tls.policy.clear]
        "/tls/policies/clear" => tls::clear_policy(state, parse_params(req.params)?),
        // i[tls.cert.list]
        "/tls/certificates/list" => tls::list_certificates(state),
        // i[tls.cert.issue-acme-dns]
        "/tls/certificates/issue-acme-dns" => tls::issue_acme_dns(state, parse_params(req.params)?),
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
