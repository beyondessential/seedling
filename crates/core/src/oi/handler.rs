use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use seedling_protocol::{
    error::{ErrorCode, HandlerResult, OiError},
    events::EventSenderWithActor,
};

use super::state::OiState;

/// Reject port 0.
///
/// These arrive as plain `u16`, so 0 deserialises happily — but it is not a
/// routable listener or backend port. It reached the site-proxy config as
/// `listen :0` / dial `:0` semantics instead of being refused at the
/// interface, where the operator could still be told which field was wrong.
/// The BSL side has rejected it all along, via `Port::new`.
// spec: SITE#endpoint-addressing
pub(super) fn validate_port(field: &str, port: u16) -> Result<(), OiError> {
    if port == 0 {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("{field} must be between 1 and 65535, got 0"),
        ));
    }
    Ok(())
}

/// Whether `s` is a syntactically valid DNS name.
///
/// Shape only: label lengths, permitted characters, no leading or trailing
/// hyphen, and at least one alphabetic character somewhere so an all-numeric
/// string is not mistaken for a name. Callers add their own policy on top —
/// a site-service remote host also rejects `localhost`, a TLS policy pattern
/// does not, since an exact policy may deliberately name it.
pub(super) fn is_valid_dns_name(s: &str) -> bool {
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    let mut any_alpha = false;
    for label in s.split('.') {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        for c in label.chars() {
            if !(c.is_ascii_alphanumeric() || c == '-') {
                return false;
            }
            if c.is_ascii_alphabetic() {
                any_alpha = true;
            }
        }
    }
    // Reject all-numeric strings (e.g. "12345"); legitimate names always
    // carry at least one alphabetic character somewhere.
    any_alpha
}

pub mod actions;
mod appdef_json;
mod apps;
mod canopy;
mod definition;
mod faults;
mod images;
mod ingresses;
mod key_mgmt;
mod params;
mod registries;
mod restarts;
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

// `Option<Option<T>>` distinguishes "field absent" from "field present
// but null". serde-json's default Option deserialiser collapses both
// into None, which would make it impossible to clear a value via the
// same endpoint that updates it. This wrapper preserves the three-state
// semantics; pair it with `#[serde(default)]` so absence stays `None`.
pub(crate) fn deserialize_optional_field<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

/// Wrap a handler outcome in the response envelope and serialise it.
fn envelope(result: HandlerResult) -> Vec<u8> {
    let response = match result {
        // spec: WIRE#control-requests
        Ok(result) => json!({ "result": result }),
        // spec: WIRE#control-requests
        Err(e) => json!({
            "error": {
                "code": e.code,
                "message": e.message,
            }
        }),
    };
    serde_json::to_vec(&response).expect("response serialisation never fails")
}

/// Parse the newline-terminated JSON request from `buf`, dispatch to a handler,
/// and return the serialised JSON response (no trailing newline).
pub fn dispatch(state: &Arc<OiState>, buf: &[u8], ctx: &RequestCtx) -> Vec<u8> {
    envelope(parse_and_dispatch(state, buf, ctx))
}

/// Dispatch the methods that cannot go through [`dispatch`], because they need
/// the connection the request arrived on.
///
/// Returns `None` when `buf` names none of them, so the caller falls through to
/// the shared table rather than this having to know every other method.
pub fn dispatch_connection_bound(
    state: &Arc<OiState>,
    conn: &quinn::Connection,
    buf: &[u8],
    ctx: &RequestCtx,
) -> Option<Vec<u8>> {
    #[derive(serde::Deserialize)]
    struct Request {
        method: String,
        #[serde(default)]
        params: Value,
    }
    let req: Request = serde_json::from_slice(buf).ok()?;

    /// The methods handled here. Listed rather than matched by prefix because
    /// the rest of the Canopy surface is ordinary and belongs in the shared
    /// table; swallowing it here would leave those methods unreachable.
    const CONNECTION_BOUND: [&str; 2] = ["/canopy/offer", "/canopy/withdraw"];
    if !CONNECTION_BOUND.contains(&req.method.as_str()) {
        return None;
    }

    let result = (|| match req.method.as_str() {
        // spec: RLY#offers
        "/canopy/offer" => canopy::offer(state, conn, parse_params(req.params)?, ctx),
        // spec: RLY#offer-lifetime
        "/canopy/withdraw" => canopy::withdraw(state, conn, parse_params(req.params)?, ctx),
        other => Err(OiError::new(
            ErrorCode::NotFound,
            format!("unknown method: {other}"),
        )),
    })();
    Some(envelope(result))
}

fn parse_and_dispatch(state: &Arc<OiState>, buf: &[u8], ctx: &RequestCtx) -> HandlerResult {
    // spec: WIRE#control-requests
    #[derive(serde::Deserialize)]
    struct Request {
        method: String,
        #[serde(default)]
        params: Value,
    }
    let req: Request = serde_json::from_slice(buf)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("invalid request: {e}")))?;

    let result = match req.method.as_str() {
        // spec: STS#liveness
        "/server/ping" => status::ping(),
        // spec: STS#server-status
        "/server/status" => status::get_status(state),
        // spec: APPS#listing
        "/apps/list" => apps::list_apps(state),
        // spec: APPS#description
        "/apps/show" => apps::describe_app(state, parse_params(req.params)?),
        "/apps/create" => apps::register_app(state, parse_params(req.params)?, ctx),
        "/apps/remove" => apps::deregister_app(state, parse_params(req.params)?, ctx),
        "/apps/uninstall" => apps::uninstall_app(state, parse_params(req.params)?),
        "/apps/update" => apps::update_app(state, parse_params(req.params)?, ctx),
        // spec: CTL#scaling
        "/apps/scale" => apps::scale_app(state, parse_params(req.params)?, ctx),
        // spec: CTL#app-priority
        "/apps/priority" => apps::set_priority(state, parse_params(req.params)?, ctx),
        // spec: CTL#deployment-restart
        "/apps/restart" => apps::restart_deployment(state, parse_params(req.params)?, ctx),
        // spec: CTL#resource-stop
        "/apps/resource/stop" => apps::stop_resource(state, parse_params(req.params)?, ctx),
        // spec: CTL#resource-stop
        "/apps/resource/unstop" => apps::unstop_resource(state, parse_params(req.params)?, ctx),
        // spec: CTL#resource-stop
        "/apps/unstop" => apps::unstop_all_resources(state, parse_params(req.params)?, ctx),
        // spec: APPS#definition-retrieval
        "/apps/script" => apps::get_app_script(state, parse_params(req.params)?),
        // spec: APPS#definition-retrieval
        "/apps/bundle" => apps::get_app_bundle(state, parse_params(req.params)?),
        // spec: APPS#generation-history
        "/apps/generations" => apps::list_generations(state, parse_params(req.params)?),
        // spec: PRM#change-planning
        "/apps/plan" => apps::dry_run_plan(state, parse_params(req.params)?, ctx),
        // spec: PRM#parameter-set
        "/apps/params/set" => params::set_param(state, parse_params(req.params)?, ctx),
        // spec: PRM#parameter-unset
        "/apps/params/unset" => params::unset_param(state, parse_params(req.params)?, ctx),
        // spec: ACI#invoking-an-action
        "/apps/action/invoke" => actions::invoke_action(state, parse_params(req.params)?, ctx),
        // spec: ACI#cancellation
        "/apps/action/cancel" => actions::cancel_action(state, parse_params(req.params)?),
        // spec: ACI#install
        "/apps/install/invoke" => {
            actions::install::invoke_install(state, parse_params(req.params)?, ctx)
        }
        // spec: KEY#listing-keys
        "/keys/list" => key_mgmt::list_keys(state),
        // spec: KEY#authorising-a-key
        "/keys/authorise" => key_mgmt::authorize_key(state, parse_params(req.params)?),
        // spec: KEY#revoking-a-key
        "/keys/revoke" => key_mgmt::revoke_key(state, parse_params(req.params)?),
        // spec: SHL#resizing-a-terminal
        "/shells/resize" => super::shells::resize_shell(state, parse_params(req.params)?),
        // spec: SHL#listing-sessions
        "/shells/list" => super::shells::list_shells(state, parse_params(req.params)?),
        // spec: SHL#stopping-a-session
        "/shells/stop" => super::shells::stop_shell(state, parse_params(req.params)?),
        // spec: FWD#listing-forwards
        "/forwards/list" => {
            super::forwards::handler::list_forwards(state, parse_params(req.params)?)
        }
        // spec: FWD#stopping-a-forward
        "/forwards/stop" => {
            super::forwards::handler::stop_forward(state, parse_params(req.params)?)
        }
        // spec: FLTS#listing-faults
        "/faults/list" => faults::list_faults(state, parse_params(req.params)?),
        // spec: FLTS#clearing-faults
        "/faults/clear" => faults::clear_app_faults(state, parse_params(req.params)?),
        // spec: FLTS#listing-restarts
        "/restarts/list" => restarts::list_restarts(state, parse_params(req.params)?),
        // spec: FLTS#crash-loop-settings
        "/restarts/settings/get" => restarts::get_settings(state),
        // spec: FLTS#crash-loop-settings
        "/restarts/settings/set" => restarts::set_settings(state, parse_params(req.params)?),
        // spec: RLY#status
        "/canopy/status" => canopy::status(state),
        // spec: RLY#settings
        "/canopy/settings/get" => canopy::get_settings(state),
        // spec: RLY#settings
        "/canopy/settings/set" => canopy::set_settings(state, parse_params(req.params)?, ctx),
        // spec: IMGM#registry-allowlist
        "/registries/list" => registries::list_registries(state),
        // spec: IMGM#registry-allowlist
        "/registries/add" => registries::add_registry(state, parse_params(req.params)?),
        // spec: IMGM#registry-allowlist
        "/registries/remove" => registries::remove_registry(state, parse_params(req.params)?),
        // spec: DEFN#repository-tags
        "/registries/tags" => registries::list_tags(state, parse_params(req.params)?),
        // spec: IMGM#listing-images
        "/images/list" => images::list_images(state),
        // spec: IMGM#pulling-an-image
        "/images/pull" => images::pull_image(state, parse_params(req.params)?),
        // spec: IMGM#removing-an-image
        "/images/remove" => images::remove_image(state, parse_params(req.params)?),
        // spec: IMGM#image-pins
        "/images/pins/list" => images::list_pins(state, parse_params(req.params)?),
        // spec: IMGM#image-pins
        "/images/pins/clear" => images::clear_pins(state, parse_params(req.params)?),
        // spec: IMGM#image-discovery
        "/apps/images/discover" => images::discover_images(state, parse_params(req.params)?),
        "/volumes/held/list" => volumes::list_held(state),
        "/volumes/held/delete" => volumes::delete_held(state, parse_params(req.params)?, ctx),
        "/volumes/held/restore" => volumes::restore_held(state, parse_params(req.params)?, ctx),
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
        "/services/site/resolver-status" => services::site_service_resolver_status(state),
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
        // spec: SITE#site-ingresses
        "/ingresses/site/list" => ingresses::list_site_ingresses(state),
        "/ingresses/site/show" => ingresses::get_site_ingress(state, parse_params(req.params)?),
        // spec: SITE#sources
        "/ingresses/site/create" => {
            ingresses::create_site_ingress(state, parse_params(req.params)?, ctx)
        }
        // spec: SITE#sources
        "/ingresses/site/delete" => {
            ingresses::delete_site_ingress(state, parse_params(req.params)?, ctx)
        }
        // spec: SITE#sources
        "/ingresses/site/update" => {
            ingresses::update_site_ingress(state, parse_params(req.params)?, ctx)
        }
        // spec: SITE#attachments
        "/ingresses/site/attach/forward" => {
            ingresses::attach_forward(state, parse_params(req.params)?, ctx)
        }
        // spec: SITE#attachments
        "/ingresses/site/attach/redirect" => {
            ingresses::attach_redirect(state, parse_params(req.params)?, ctx)
        }
        // spec: SITE#attachments
        "/ingresses/site/detach" => ingresses::detach(state, parse_params(req.params)?, ctx),
        "/ingresses/site/discovery/status" => ingresses::discovery_status(state),
        // spec: SITE#tailscale-discovery
        "/ingresses/site/discovery/refresh" => ingresses::discovery_refresh(state),
        // spec: TPL#creation
        "/templates/create" => templates::create_template(state, parse_params(req.params)?, ctx),
        // spec: TPL#listing
        "/templates/list" => templates::list_templates(state),
        // spec: TPL#inspection
        "/templates/show" => templates::show_template(state, parse_params(req.params)?),
        // spec: TPL#update
        "/templates/update" => templates::update_template(state, parse_params(req.params)?, ctx),
        // spec: TPL#removal
        "/templates/remove" => templates::remove_template(state, parse_params(req.params)?, ctx),
        // spec: TPL#preview
        "/templates/preview" => templates::preview_template(state, parse_params(req.params)?, ctx),
        // spec: TPL#instantiation
        "/templates/instantiate" => {
            templates::instantiate_template(state, parse_params(req.params)?, ctx)
        }
        // spec: TLSM#dns-providers
        "/tls/dns-providers/list" => tls::list_dns_providers(state),
        // spec: TLSM#dns-providers
        "/tls/dns-providers/upsert" => tls::upsert_dns_provider(state, parse_params(req.params)?),
        // spec: TLSM#dns-providers
        "/tls/dns-providers/delete" => tls::delete_dns_provider(state, parse_params(req.params)?),
        // spec: TLSM#policies
        "/tls/policies/list" => tls::list_policies(state),
        // spec: TLSM#policies
        "/tls/policies/set-acme-dns" => tls::set_policy_acme_dns(state, parse_params(req.params)?),
        // spec: TLSM#policies
        "/tls/policies/set-canopy" => tls::set_policy_canopy(state, parse_params(req.params)?),
        // spec: TLSM#policies
        "/tls/policies/clear" => tls::clear_policy(state, parse_params(req.params)?),
        // spec: TLSM#certificates
        "/tls/certificates/list" => tls::list_certificates(state),
        // spec: TLSM#certificates
        "/tls/certificates/upload-manual" => tls::upload_manual(state, parse_params(req.params)?),
        // spec: TLSM#certificates
        "/tls/certificates/delete" => tls::delete_certificate(state, parse_params(req.params)?),
        // spec: TLSM#certificate-signing-requests
        "/tls/certificates/csr/begin" => tls::csr_begin(state, parse_params(req.params)?),
        // spec: TLSM#certificate-signing-requests
        "/tls/certificates/csr/get" => tls::csr_get(state, parse_params(req.params)?),
        // spec: TLSM#certificate-signing-requests
        "/tls/certificates/csr/upload-cert" => {
            tls::csr_upload_cert(state, parse_params(req.params)?)
        }
        // spec: TLSM#certificate-signing-requests
        "/tls/certificates/csr/cancel" => tls::csr_cancel(state, parse_params(req.params)?),
        // spec: TLSM#operator-driven-issuance
        "/tls/certificates/issue-acme-dns" => tls::issue_acme_dns(state, parse_params(req.params)?),
        // spec: TLSM#operator-driven-issuance
        "/tls/certificates/retry" => tls::retry(state, parse_params(req.params)?),
        // spec: TLSM#settings
        "/tls/settings/get" => tls::get_settings(state),
        // spec: TLSM#settings
        "/tls/settings/set" => tls::set_settings(state, parse_params(req.params)?),
        // spec: TLSM#hostname-view
        "/tls/hostnames/list" => tls::list_hostnames(state, parse_params(req.params)?),
        // spec: TLSM#issuance-attempts
        "/tls/certificates/attempts/list" => tls::list_attempts(state, parse_params(req.params)?),
        // spec: TLSM#retry-blocks
        "/tls/retry-blocks/list" => tls::list_retry_blocks(state),
        // spec: TLSM#retry-blocks
        "/tls/retry-blocks/set" => tls::set_retry_block(state, parse_params(req.params)?),
        // spec: TLSM#retry-blocks
        "/tls/retry-blocks/clear" => tls::clear_retry_block(state, parse_params(req.params)?),
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
