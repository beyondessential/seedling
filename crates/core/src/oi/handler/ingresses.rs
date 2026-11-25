use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};
use seedling_protocol::events::AttachmentTargetRef;
use seedling_protocol::names::{AppName, AppServiceName, SiteIngressName};
use serde::Deserialize;
use serde_json::json;

use crate::defs::ingress::check_hostname;
use crate::oi::{handler::RequestCtx, state::OiState};
use crate::runtime::site_ingress_attachments::{
    self, AttachmentProtocol, AttachmentTarget, SiteIngressAttachment,
};
use crate::runtime::site_ingresses::{
    self, DiscoveryProvider, SiteIngressDef, SiteIngressSource, TlsProvider,
};

#[cfg(test)]
mod tests;

fn internal(what: &str, e: impl std::fmt::Display) -> OiError {
    OiError::new(ErrorCode::Internal, format!("{what}: {e}"))
}

fn no_site_ingress(name: &SiteIngressName) -> OiError {
    OiError::new(
        ErrorCode::NotFound,
        format!("no site ingress named {:?}", name.as_str()),
    )
}

/// An attachment's target, as its events report it.
fn event_target(target: &AttachmentTarget) -> AttachmentTargetRef<'_> {
    match target {
        AttachmentTarget::Forward { app, service } => AttachmentTargetRef::Forward { app, service },
        AttachmentTarget::Redirect {
            url,
            code,
            preserve_path,
        } => AttachmentTargetRef::Redirect {
            url,
            code: *code,
            preserve_path: *preserve_path,
        },
    }
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

// spec: INGM#creation
pub(crate) fn create_site_ingress(
    state: &OiState,
    params: CreateSiteIngressParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    // Site ingresses share the public DNS namespace with app ingresses, so
    // a hostname an app could never declare is refused here too.
    // spec: ING#hostnames
    check_hostname(&params.hostname)
        .map_err(|e| OiError::new(ErrorCode::RequirementsInvalid, e))?;
    // spec: IDN#reserved-names
    crate::reserved::check_site_ingress_name(&params.name)
        .map_err(|e| OiError::new(ErrorCode::RequirementsInvalid, e.to_string()))?;
    if matches!(params.tls_provider, TlsProvider::Tailscale) {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "tls_provider 'tailscale' is only legal on a discovered Tailscale ingress".to_owned(),
        ));
    }

    let def = SiteIngressDef {
        name: params.name.clone(),
        // The same one form an app's ingress is held in, so the two sides of a
        // (hostname, port) claim are comparable.
        hostname: params.hostname.to_ascii_lowercase(),
        description: params.description.clone(),
        source: SiteIngressSource::Manual,
        tls_provider: params.tls_provider,
        stale: false,
        created_at: jiff::Timestamp::now().to_string(),
    };
    let def_for_db = def.clone();
    let created = state
        .db
        .call(move |db| site_ingresses::create_if_absent(db, &def_for_db))
        .map_err(|e| internal("failed to create site ingress", e))?;
    if !created {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "a site ingress named {:?} already exists",
                params.name.as_str()
            ),
        ));
    }

    // spec: SITE#attachments
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

// spec: INGM#deletion
pub(crate) fn delete_site_ingress(
    state: &OiState,
    params: DeleteSiteIngressParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name_for_db = params.name.clone();
    let (def, attachments) = match state
        .db
        .call(move |db| site_ingresses::delete_unreported(db, &name_for_db))
        .map_err(|e| internal("failed to delete site ingress", e))?
    {
        site_ingresses::Deletion::Deleted { def, attachments } => (def, attachments),
        site_ingresses::Deletion::Absent => return Err(no_site_ingress(&params.name)),
        site_ingresses::Deletion::StillReported(_) => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "site ingress {:?} is owned by a discovery provider that still reports it, \
                     so it cannot be deleted",
                    params.name.as_str()
                ),
            ));
        }
    };

    // spec: SITE#attachments
    for att in &attachments {
        ctx.events.site_ingress_attachment_removed(
            &def.name,
            att.port,
            att.protocol.as_str(),
            event_target(&att.target),
        );
    }
    let source = match def.source {
        SiteIngressSource::Manual => "manual",
        SiteIngressSource::Discovered { .. } => "discovered",
    };
    ctx.events.site_ingress_deleted(&def.name, source);

    state.tick_notify.notify_one();
    Ok(json!({ "deleted": true }))
}

#[derive(Deserialize)]
pub(crate) struct UpdateSiteIngressParams {
    pub name: SiteIngressName,
    /// If `Some`, sets the description; pass `null` to clear.
    #[serde(default, deserialize_with = "super::deserialize_optional_field")]
    pub description: Option<Option<String>>,
    /// If `Some`, sets the TLS provider. Tailscale is rejected for manual
    /// ingresses; manual ingresses can move freely between acme / internal /
    /// none.
    #[serde(default)]
    pub tls_provider: Option<TlsProvider>,
}

// spec: INGM#update
pub(crate) fn update_site_ingress(
    state: &OiState,
    params: UpdateSiteIngressParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    if let Some(p) = params.tls_provider
        && matches!(p, TlsProvider::Tailscale)
    {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "tls_provider 'tailscale' is only legal on a discovered Tailscale ingress".to_owned(),
        ));
    }

    let name = params.name.clone();
    let description = params.description.clone();
    let tls_provider = params.tls_provider;
    let updated = state
        .db
        .call(move |db| {
            site_ingresses::update_manual(
                db,
                &name,
                description.as_ref().map(|d| d.as_deref()),
                tls_provider,
            )
        })
        .map_err(|e| internal("failed to update site ingress", e))?;

    match updated {
        site_ingresses::Update::Absent => return Err(no_site_ingress(&params.name)),
        site_ingresses::Update::Discovered => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "site ingress {:?} is managed by a discovery provider; \
                     description and TLS provider are not editable",
                    params.name.as_str()
                ),
            ));
        }
        site_ingresses::Update::Unchanged(_) => {}
        site_ingresses::Update::Changed(def) => {
            ctx.events.site_ingress_updated(
                &def.name,
                &def.hostname,
                def.tls_provider.as_str(),
                def.description.as_deref(),
            );
            state.tick_notify.notify_one();
        }
    }

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
    pub redirect_preserve_path: bool,
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

/// Give an attachment's `(port, protocol)` its target and emit the event for
/// what that did.
// spec: INGM#attachments
fn attach(state: &OiState, att: SiteIngressAttachment, ctx: &RequestCtx) -> Result<(), OiError> {
    let name = att.site_ingress.clone();
    let att_for_db = att.clone();
    let attached = state
        .db
        .call(move |db| site_ingress_attachments::set_target(db, &att_for_db))
        .map_err(|e| internal("failed to set the site ingress attachment", e))?;
    let target = event_target(&att.target);
    match attached {
        site_ingress_attachments::Attached::NoIngress => return Err(no_site_ingress(&name)),
        site_ingress_attachments::Attached::Unchanged => return Ok(()),
        site_ingress_attachments::Attached::Added => {
            ctx.events
                .site_ingress_attachment_added(&name, att.port, att.protocol.as_str(), target)
        }
        site_ingress_attachments::Attached::Updated => ctx.events.site_ingress_attachment_updated(
            &name,
            att.port,
            att.protocol.as_str(),
            target,
        ),
    }
    state.tick_notify.notify_one();
    Ok(())
}

/// `not_found` unless the site ingress exists, so a request for one that does
/// not is refused before its other members are looked at.
fn require_site_ingress(state: &OiState, name: &SiteIngressName) -> Result<(), OiError> {
    let n = name.clone();
    state
        .db
        .call(move |db| site_ingresses::get(db, &n))
        .map_err(|e| internal("failed to look up site ingress", e))?
        .map(|_| ())
        .ok_or_else(|| no_site_ingress(name))
}

// spec: INGM#attachments
pub(crate) fn attach_forward(
    state: &OiState,
    params: AttachmentForwardParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    require_site_ingress(state, &params.name)?;
    super::validate_port("port", params.port)?;
    attach(
        state,
        SiteIngressAttachment {
            site_ingress: params.name,
            port: params.port,
            protocol: params.protocol,
            target: AttachmentTarget::Forward {
                app: params.target_app,
                service: params.target_service,
            },
            created_at: jiff::Timestamp::now().to_string(),
        },
        ctx,
    )?;
    Ok(json!({ "attached": true }))
}

// spec: INGM#attachments
pub(crate) fn attach_redirect(
    state: &OiState,
    params: AttachmentRedirectParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    require_site_ingress(state, &params.name)?;
    super::validate_port("port", params.port)?;
    validate_redirect_code(params.redirect_code)?;
    validate_redirect_url(&params.redirect_url)?;
    if matches!(
        params.protocol,
        AttachmentProtocol::Tcp | AttachmentProtocol::Udp
    ) {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "redirect attachments must use protocol 'http' or 'http2'; got {}",
                params.protocol
            ),
        ));
    }

    attach(
        state,
        SiteIngressAttachment {
            site_ingress: params.name,
            port: params.port,
            protocol: params.protocol,
            target: AttachmentTarget::Redirect {
                url: params.redirect_url,
                code: params.redirect_code,
                preserve_path: params.redirect_preserve_path,
            },
            created_at: jiff::Timestamp::now().to_string(),
        },
        ctx,
    )?;
    Ok(json!({ "attached": true }))
}

// spec: INGM#attachments
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
        .call(move |db| site_ingress_attachments::detach_returning(db, &name, port, protocol))
        .map_err(|e| internal("failed to remove site ingress attachment", e))?;
    let Some(removed) = removed else {
        return Err(OiError::new(
            ErrorCode::NotFound,
            format!(
                "site ingress {:?} has no attachment on {}:{}",
                params.name.as_str(),
                params.port,
                params.protocol
            ),
        ));
    };

    ctx.events.site_ingress_attachment_removed(
        &params.name,
        params.port,
        params.protocol.as_str(),
        event_target(&removed.target),
    );

    state.tick_notify.notify_one();
    Ok(json!({ "detached": true }))
}

/// Read-only summary of the discovery providers the runtime knows about.
/// In v1 the only entry is Tailscale; the row is materialised by the
/// Tailscale provider's poll loop and reported here with health, last
/// poll time, and any pending error so the operator can see *why*
/// discovery is unhealthy without grepping logs.
// spec: SITE#tailscale-discovery
pub(crate) fn discovery_status(state: &OiState) -> HandlerResult {
    let discovered = state.db.call(site_ingresses::list).map_err(|e| {
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

    // One object for each provider the runtime runs.
    let mut providers = Vec::new();
    if let Some(provider) = &state.tailscale_provider {
        let mut tailscale_obj = json!({
            "name": "tailscale",
            "ingresses": tailscale_ingresses,
        });
        let snap = provider.status().snapshot();
        // `null` until the provider's first poll has finished.
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
        providers.push(tailscale_obj);
    }

    Ok(json!({ "providers": providers }))
}

/// Force the Tailscale provider's poll loop to run now instead of
/// waiting for the next tick. Returns `{"refreshed": true}` when the
/// kick was delivered, or an error if no provider is configured.
// spec: SITE#tailscale-discovery
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
