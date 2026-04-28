use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};
use seedling_protocol::names::{AppName, AppServiceName, SiteIngressName};
use serde::Deserialize;
use serde_json::json;

use crate::oi::{handler::RequestCtx, state::OiState};
use crate::runtime::site_ingress_attachments::{
    self, AttachmentProtocol, AttachmentTarget, SiteIngressAttachment,
};
use crate::runtime::site_ingresses::{
    self, DiscoveryProvider, SiteIngressDef, SiteIngressSource, TlsProvider,
};

/// Validate an operator-supplied hostname using the same shape as
/// [`crate::defs::service::validate_hostname`] (which rejects wildcard
/// labels, leading/trailing hyphens, and oversize labels). Site
/// ingresses share the public DNS namespace with app ingresses, so the
/// rules must match — otherwise an operator could create a site
/// ingress with a hostname that an app could never declare.
fn validate_hostname(hostname: &str) -> Result<(), OiError> {
    if hostname.is_empty() || hostname.len() > 253 {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("hostname must be 1-253 characters, got {}", hostname.len()),
        ));
    }
    if hostname.contains('*') {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "wildcard hostnames are not permitted".to_owned(),
        ));
    }
    for label in hostname.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "each hostname label must be 1-63 characters, got '{}' ({})",
                    label,
                    label.len()
                ),
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!("hostname label must not start or end with a hyphen: '{label}'"),
            ));
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!("hostname label contains invalid characters: '{label}'"),
            ));
        }
    }
    Ok(())
}

fn render_ingress(
    def: &SiteIngressDef,
    attachments: &[SiteIngressAttachment],
) -> serde_json::Value {
    let (source_str, discovered_provider) = match &def.source {
        SiteIngressSource::Manual => ("manual", None),
        SiteIngressSource::Discovered { provider, .. } => ("discovered", Some(provider.as_str())),
    };
    let discovered_key = match &def.source {
        SiteIngressSource::Manual => None,
        SiteIngressSource::Discovered { key, .. } => Some(key.as_str()),
    };
    let attachments: Vec<_> = attachments
        .iter()
        .map(|a| {
            let mut obj = json!({
                "port": a.port,
                "protocol": a.protocol.as_str(),
                "target_kind": a.target.kind_str(),
                "created_at": a.created_at,
            });
            match &a.target {
                AttachmentTarget::Forward { app, service } => {
                    obj["target_app"] = json!(app);
                    obj["target_service"] = json!(service);
                }
                AttachmentTarget::Redirect {
                    url,
                    code,
                    preserve_path,
                } => {
                    obj["redirect_url"] = json!(url);
                    obj["redirect_code"] = json!(code);
                    obj["redirect_preserve_path"] = json!(preserve_path);
                }
            }
            obj
        })
        .collect();

    let mut obj = json!({
        "name": def.name,
        "hostname": def.hostname,
        "source": source_str,
        "tls_provider": def.tls_provider.as_str(),
        "stale": def.stale,
        "created_at": def.created_at,
        "attachments": attachments,
    });
    if let Some(p) = discovered_provider {
        obj["discovered_provider"] = json!(p);
    }
    if let Some(k) = discovered_key {
        obj["discovered_key"] = json!(k);
    }
    if let Some(d) = &def.description {
        obj["description"] = json!(d);
    }
    obj
}

pub(crate) fn list_site_ingresses(state: &OiState) -> HandlerResult {
    let (defs, attachments) = state
        .db
        .call(|db| {
            let defs = site_ingresses::list(db)?;
            let attachments = site_ingress_attachments::list_all(db)?;
            Ok::<_, rusqlite::Error>((defs, attachments))
        })
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to list site ingresses: {e}"),
            )
        })?;

    let items: Vec<_> = defs
        .iter()
        .map(|d| {
            let atts: Vec<_> = attachments
                .iter()
                .filter(|a| a.site_ingress == d.name)
                .cloned()
                .collect();
            render_ingress(d, &atts)
        })
        .collect();

    Ok(json!(items))
}

#[derive(Deserialize)]
pub(crate) struct GetSiteIngressParams {
    pub name: SiteIngressName,
}

pub(crate) fn get_site_ingress(state: &OiState, params: GetSiteIngressParams) -> HandlerResult {
    let name_for_get = params.name.clone();
    let name_for_atts = params.name.clone();
    let (def, attachments) = state
        .db
        .call(move |db| {
            let def = site_ingresses::get(db, &name_for_get)?;
            let attachments = site_ingress_attachments::list_for_ingress(db, &name_for_atts)?;
            Ok::<_, rusqlite::Error>((def, attachments))
        })
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to get site ingress: {e}"),
            )
        })?;

    let Some(def) = def else {
        return Err(OiError::new(
            ErrorCode::NotFound,
            format!("no site ingress named {:?}", params.name.as_str()),
        ));
    };
    Ok(render_ingress(&def, &attachments))
}

#[derive(Deserialize)]
pub(crate) struct CreateSiteIngressParams {
    pub name: SiteIngressName,
    pub hostname: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_manual_tls_provider")]
    pub tls_provider: TlsProvider,
}

fn default_manual_tls_provider() -> TlsProvider {
    TlsProvider::Acme
}

// r[impl ingress.site.lifecycle]
pub(crate) fn create_site_ingress(
    state: &OiState,
    params: CreateSiteIngressParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    validate_hostname(&params.hostname)?;
    if matches!(params.tls_provider, TlsProvider::Tailscale) {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "tls_provider 'tailscale' is only legal on a discovered Tailscale ingress".to_owned(),
        ));
    }

    let def = SiteIngressDef {
        name: params.name.clone(),
        hostname: params.hostname.clone(),
        description: params.description.clone(),
        source: SiteIngressSource::Manual,
        tls_provider: params.tls_provider,
        stale: false,
        created_at: jiff::Timestamp::now().to_string(),
    };
    let def_for_db = def.clone();
    state
        .db
        .call(move |db| site_ingresses::create(db, &def_for_db))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to create site ingress: {e}"),
            )
        })?;

    // r[impl ingress.site.lifecycle.events]
    ctx.events.site_ingress_created(
        &def.name,
        &def.hostname,
        "manual",
        None,
        def.tls_provider.as_str(),
        def.description.as_deref(),
    );

    state.tick_notify.notify_one();
    Ok(json!({ "created": true }))
}

#[derive(Deserialize)]
pub(crate) struct DeleteSiteIngressParams {
    pub name: SiteIngressName,
}

// r[impl ingress.site.lifecycle]
pub(crate) fn delete_site_ingress(
    state: &OiState,
    params: DeleteSiteIngressParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name_for_get = params.name.clone();
    let existing = state
        .db
        .call(move |db| site_ingresses::get(db, &name_for_get))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to look up site ingress: {e}"),
            )
        })?;
    let Some(existing) = existing else {
        return Err(OiError::new(
            ErrorCode::NotFound,
            format!("no site ingress named {:?}", params.name.as_str()),
        ));
    };
    if existing.source.is_discovered() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "site ingress {:?} is owned by a discovery provider \
                 and cannot be deleted while its source is active",
                params.name.as_str()
            ),
        ));
    }

    let name_for_db = params.name.clone();
    let deleted = state
        .db
        .call(move |db| site_ingresses::delete(db, &name_for_db))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to delete site ingress: {e}"),
            )
        })?;
    if !deleted {
        // Could happen if a parallel request raced us; treat as NotFound
        // for consistency with the `existing` lookup above.
        return Err(OiError::new(
            ErrorCode::NotFound,
            format!("no site ingress named {:?}", params.name.as_str()),
        ));
    }

    // r[impl ingress.site.lifecycle.events]
    ctx.events.site_ingress_deleted(&existing.name, "manual");

    state.tick_notify.notify_one();
    Ok(json!({ "deleted": true }))
}

#[derive(Deserialize)]
pub(crate) struct UpdateSiteIngressParams {
    pub name: SiteIngressName,
    /// If `Some`, sets the description (use empty string is rejected here —
    /// pass `null` to clear).
    #[serde(default)]
    pub description: Option<Option<String>>,
    /// If `Some`, sets the TLS provider. Tailscale is rejected for manual
    /// ingresses; manual ingresses can move freely between acme / internal /
    /// none.
    #[serde(default)]
    pub tls_provider: Option<TlsProvider>,
}

// r[impl ingress.site.lifecycle]
pub(crate) fn update_site_ingress(
    state: &OiState,
    params: UpdateSiteIngressParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name_for_get = params.name.clone();
    let existing = state
        .db
        .call(move |db| site_ingresses::get(db, &name_for_get))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to look up site ingress: {e}"),
            )
        })?;
    let Some(existing) = existing else {
        return Err(OiError::new(
            ErrorCode::NotFound,
            format!("no site ingress named {:?}", params.name.as_str()),
        ));
    };
    if existing.source.is_discovered() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "site ingress {:?} is managed by a discovery provider; \
                 description and TLS provider are not editable",
                params.name.as_str()
            ),
        ));
    }
    if let Some(p) = params.tls_provider
        && matches!(p, TlsProvider::Tailscale)
    {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "tls_provider 'tailscale' is only legal on a discovered Tailscale ingress".to_owned(),
        ));
    }

    let new_description = match &params.description {
        Some(opt) => opt.clone(),
        None => existing.description.clone(),
    };
    let new_tls = params.tls_provider.unwrap_or(existing.tls_provider);

    let name_d = params.name.clone();
    let desc_for_db = new_description.clone();
    let tls_for_db = new_tls;
    state
        .db
        .call(move |db| {
            if params.description.is_some() {
                site_ingresses::update_description(db, &name_d, desc_for_db.as_deref())?;
            }
            site_ingresses::update_tls_provider(db, &name_d, tls_for_db)?;
            Ok::<_, rusqlite::Error>(())
        })
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to update site ingress: {e}"),
            )
        })?;

    ctx.events.site_ingress_updated(
        &existing.name,
        &existing.hostname,
        new_tls.as_str(),
        new_description.as_deref(),
    );

    state.tick_notify.notify_one();
    Ok(json!({ "updated": true }))
}

#[derive(Deserialize)]
pub(crate) struct AttachmentForwardParams {
    pub name: SiteIngressName,
    pub port: u16,
    pub protocol: AttachmentProtocol,
    pub target_app: AppName,
    pub target_service: AppServiceName,
}

#[derive(Deserialize)]
pub(crate) struct AttachmentRedirectParams {
    pub name: SiteIngressName,
    pub port: u16,
    pub protocol: AttachmentProtocol,
    pub redirect_url: String,
    #[serde(default = "default_redirect_code")]
    pub redirect_code: u16,
    #[serde(default = "default_preserve_path")]
    pub preserve_path: bool,
}

fn default_redirect_code() -> u16 {
    307
}

fn default_preserve_path() -> bool {
    true
}

#[derive(Deserialize)]
pub(crate) struct DetachAttachmentParams {
    pub name: SiteIngressName,
    pub port: u16,
    pub protocol: AttachmentProtocol,
}

fn ensure_ingress_exists(
    state: &OiState,
    name: &SiteIngressName,
) -> Result<SiteIngressDef, OiError> {
    let n = name.clone();
    let def = state
        .db
        .call(move |db| site_ingresses::get(db, &n))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to look up site ingress: {e}"),
            )
        })?;
    def.ok_or_else(|| {
        OiError::new(
            ErrorCode::NotFound,
            format!("no site ingress named {:?}", name.as_str()),
        )
    })
}

fn validate_redirect_code(code: u16) -> Result<(), OiError> {
    match code {
        301 | 302 | 307 | 308 => Ok(()),
        _ => Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("redirect_code must be one of 301, 302, 307, 308; got {code}"),
        )),
    }
}

fn validate_redirect_url(url: &str) -> Result<(), OiError> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("redirect_url must start with 'http://' or 'https://'; got {url:?}"),
        ));
    }
    Ok(())
}

// r[impl ingress.site.attachment]
pub(crate) fn attach_forward(
    state: &OiState,
    params: AttachmentForwardParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let _ = ensure_ingress_exists(state, &params.name)?;
    let att = SiteIngressAttachment {
        site_ingress: params.name.clone(),
        port: params.port,
        protocol: params.protocol,
        target: AttachmentTarget::Forward {
            app: params.target_app.clone(),
            service: params.target_service.clone(),
        },
        created_at: jiff::Timestamp::now().to_string(),
    };
    let att_for_db = att.clone();
    state
        .db
        .call(move |db| site_ingress_attachments::attach(db, &att_for_db))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to add site ingress attachment: {e}"),
            )
        })?;

    ctx.events.site_ingress_attachment_added(
        &params.name,
        params.port,
        params.protocol.as_str(),
        "forward",
        Some(&params.target_app),
        Some(&params.target_service),
        None,
        None,
    );

    state.tick_notify.notify_one();
    Ok(json!({ "attached": true }))
}

// r[impl ingress.site.attachment]
pub(crate) fn attach_redirect(
    state: &OiState,
    params: AttachmentRedirectParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let _ = ensure_ingress_exists(state, &params.name)?;
    validate_redirect_code(params.redirect_code)?;
    validate_redirect_url(&params.redirect_url)?;

    let att = SiteIngressAttachment {
        site_ingress: params.name.clone(),
        port: params.port,
        protocol: params.protocol,
        target: AttachmentTarget::Redirect {
            url: params.redirect_url.clone(),
            code: params.redirect_code,
            preserve_path: params.preserve_path,
        },
        created_at: jiff::Timestamp::now().to_string(),
    };
    let att_for_db = att.clone();
    state
        .db
        .call(move |db| site_ingress_attachments::attach(db, &att_for_db))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to add site ingress attachment: {e}"),
            )
        })?;

    ctx.events.site_ingress_attachment_added(
        &params.name,
        params.port,
        params.protocol.as_str(),
        "redirect",
        None,
        None,
        Some(&params.redirect_url),
        Some(params.redirect_code),
    );

    state.tick_notify.notify_one();
    Ok(json!({ "attached": true }))
}

// r[impl ingress.site.attachment]
pub(crate) fn detach(
    state: &OiState,
    params: DetachAttachmentParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name = params.name.clone();
    let port = params.port;
    let protocol = params.protocol;
    let removed = state
        .db
        .call(move |db| site_ingress_attachments::detach(db, &name, port, protocol))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to remove site ingress attachment: {e}"),
            )
        })?;
    if !removed {
        return Err(OiError::new(
            ErrorCode::NotFound,
            format!(
                "no attachment {}:{} ({}) on site ingress {:?}",
                params.name.as_str(),
                params.port,
                params.protocol,
                params.name.as_str()
            ),
        ));
    }

    ctx.events
        .site_ingress_attachment_removed(&params.name, params.port, params.protocol.as_str());

    state.tick_notify.notify_one();
    Ok(json!({ "detached": true }))
}

/// Read-only summary of the discovery providers the runtime knows about.
/// In v1 the only entry is Tailscale; the row is materialised by the
/// Tailscale provider's poll loop and reported here with health, last
/// poll time, and any pending error so the operator can see *why*
/// discovery is unhealthy without grepping logs.
// r[impl ingress.site.tailscale]
pub(crate) fn discovery_status(state: &OiState) -> HandlerResult {
    let discovered = state
        .db
        .call(|db| site_ingresses::list(db))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to read discovery state: {e}"),
            )
        })?;

    let tailscale_ingresses: Vec<_> = discovered
        .iter()
        .filter_map(|d| match &d.source {
            SiteIngressSource::Discovered { provider, key }
                if matches!(provider, DiscoveryProvider::Tailscale) =>
            {
                Some(json!({
                    "name": d.name,
                    "provider": provider.as_str(),
                    "key": key,
                    "hostname": d.hostname,
                    "stale": d.stale,
                }))
            }
            _ => None,
        })
        .collect();

    let mut tailscale_obj = json!({
        "name": "tailscale",
        "ingresses": tailscale_ingresses,
    });
    if let Some(provider) = &state.tailscale_provider {
        let snap = provider.status().snapshot();
        tailscale_obj["healthy"] = json!(snap.healthy);
        if let Some(ts) = snap.last_poll_at {
            tailscale_obj["last_poll_at"] = json!(ts.to_string());
        }
        if let Some(err) = snap.last_error {
            tailscale_obj["last_error"] = json!(err);
        }
        if let Some(id) = snap.identity {
            tailscale_obj["identity"] = json!({
                "hostname": id.hostname,
                "node_id": id.node_id,
                "backend_running": id.backend_running,
            });
        }
    } else {
        tailscale_obj["healthy"] = json!(false);
        tailscale_obj["last_error"] = json!("Tailscale provider is not configured");
    }

    Ok(json!({
        "providers": [tailscale_obj]
    }))
}

/// Force the Tailscale provider's poll loop to run now instead of
/// waiting for the next tick. Returns `{"refreshed": true}` when the
/// kick was delivered, or an error if no provider is configured.
// r[impl ingress.site.tailscale]
pub(crate) fn discovery_refresh(state: &OiState) -> HandlerResult {
    match &state.tailscale_provider {
        Some(p) => {
            p.refresh_now();
            Ok(json!({ "refreshed": true }))
        }
        None => Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "Tailscale provider is not configured on this daemon".to_owned(),
        )),
    }
}
