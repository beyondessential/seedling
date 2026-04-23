use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};
use seedling_protocol::events::ExternalServiceMappingSnapshot;
use seedling_protocol::names::{AppName, ExternalServiceName, ServiceRef, SiteServiceName};
use serde::Deserialize;
use serde_json::json;

use crate::oi::{handler::RequestCtx, state::OiState};
use crate::runtime::site_services::{SiteServiceDef, SiteServiceEndpoint, SiteServiceProtocol};

/// List services that apps have marked with `service.exported()`.
pub(crate) fn list_exported(state: &OiState) -> HandlerResult {
    let registry = state.registry.read();
    let mut exported = Vec::new();

    for (app_name, _status) in registry.list() {
        let Some(entry) = registry.get(app_name.as_str()) else {
            continue;
        };
        let def = entry.app.def.load();
        for (id, resource) in &def.resources {
            if let crate::defs::resource::Resource::Service(svc) = resource {
                let svc_def = svc.def.lock();
                if let Some(export_opts) = &svc_def.exported {
                    let mut item = json!({
                        "app": app_name,
                        "service_name": id.name.as_str(),
                        "http": svc_def.http.is_some(),
                    });
                    if let Some(desc) = &export_opts.description {
                        item["description"] = json!(desc);
                    }
                    exported.push(item);
                }
            }
        }
    }

    Ok(json!(exported))
}

/// Every named app service, whether or not it is exported. Mirrors
/// `volumes::list_app_volumes` for the service side.
pub(crate) fn list_app_services(state: &OiState) -> HandlerResult {
    let registry = state.registry.read();
    let mut services = Vec::new();

    for (app_name, _status) in registry.list() {
        let Some(entry) = registry.get(app_name.as_str()) else {
            continue;
        };
        let def = entry.app.def.load();
        for (id, resource) in &def.resources {
            if let crate::defs::resource::Resource::Service(svc) = resource {
                let svc_def = svc.def.lock();
                let mut item = json!({
                    "app": app_name,
                    "service_name": id.name.as_str(),
                    "http": svc_def.http.is_some(),
                    "exported": svc_def.exported.is_some(),
                });
                if let Some(desc) = svc_def
                    .exported
                    .as_ref()
                    .and_then(|e| e.description.as_ref())
                {
                    item["description"] = json!(desc);
                }
                services.push(item);
            }
        }
    }

    Ok(json!(services))
}

#[derive(Deserialize)]
pub(crate) struct EndpointParams {
    pub host: String,
    pub port: u16,
    pub protocol: SiteServiceProtocol,
}

impl From<EndpointParams> for SiteServiceEndpoint {
    fn from(p: EndpointParams) -> Self {
        Self {
            host: p.host,
            port: p.port,
            protocol: p.protocol,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct CreateSiteServiceParams {
    pub name: SiteServiceName,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub endpoints: Vec<EndpointParams>,
}

// r[impl service.site.lifecycle]
pub(crate) fn create_site_service(
    state: &OiState,
    params: CreateSiteServiceParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let endpoints: Vec<SiteServiceEndpoint> =
        params.endpoints.into_iter().map(Into::into).collect();
    let def = SiteServiceDef {
        name: params.name.clone(),
        description: params.description.clone(),
        endpoints: endpoints.clone(),
        created_at: jiff::Timestamp::now().to_string(),
    };

    state
        .db
        .call(move |db| crate::runtime::site_services::create(db, &def))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to store site service: {e}"),
            )
        })?;

    // r[impl service.site.lifecycle.events]
    ctx.events
        .site_service_created(params.name.as_str(), params.description.as_deref());
    for ep in &endpoints {
        ctx.events.site_service_endpoint_added(
            params.name.as_str(),
            &ep.host,
            ep.port,
            ep.protocol.as_str(),
        );
    }

    state.tick_notify.notify_one();
    Ok(json!({ "created": true }))
}

pub(crate) fn list_site_services(state: &OiState) -> HandlerResult {
    let services = state
        .db
        .call(crate::runtime::site_services::list)
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to list site services: {e}"),
            )
        })?;

    let items: Vec<_> = services
        .iter()
        .map(|s| {
            let endpoints: Vec<_> = s
                .endpoints
                .iter()
                .map(|e| {
                    json!({
                        "host": e.host,
                        "port": e.port,
                        "protocol": e.protocol.as_str(),
                    })
                })
                .collect();
            let mut obj = json!({
                "name": s.name,
                "created_at": s.created_at,
                "endpoints": endpoints,
            });
            if let Some(d) = &s.description {
                obj["description"] = json!(d);
            }
            obj
        })
        .collect();

    Ok(json!(items))
}

#[derive(Deserialize)]
pub(crate) struct DeleteSiteServiceParams {
    pub name: SiteServiceName,
}

// r[impl service.site.lifecycle]
pub(crate) fn delete_site_service(
    state: &OiState,
    params: DeleteSiteServiceParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name_for_check = params.name.clone();
    let in_use = state
        .db
        .call(move |db| {
            crate::runtime::external_service_mappings::list_for_site_target(db, &name_for_check)
        })
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to check mappings: {e}"),
            )
        })?;
    if !in_use.is_empty() {
        let first = &in_use[0];
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "site service {:?} is still mapped by {} external-service slot(s) \
                 (first: app={:?}, slot={:?}); unmap or remap them first",
                params.name.as_str(),
                in_use.len(),
                first.app.as_str(),
                first.external_name.as_str(),
            ),
        ));
    }

    let name_owned = params.name.clone();
    let deleted = state
        .db
        .call(move |db| crate::runtime::site_services::delete(db, &name_owned))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to delete site service: {e}"),
            )
        })?;

    if !deleted {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("no site service named {:?}", params.name.as_str()),
        ));
    }

    // r[impl service.site.lifecycle.events]
    ctx.events.site_service_deleted(params.name.as_str());

    Ok(json!({ "deleted": true }))
}

#[derive(Deserialize)]
pub(crate) struct SiteServiceEndpointParams {
    pub name: SiteServiceName,
    pub host: String,
    pub port: u16,
    pub protocol: SiteServiceProtocol,
}

// r[impl service.site.lifecycle]
pub(crate) fn add_site_service_endpoint(
    state: &OiState,
    params: SiteServiceEndpointParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name = params.name.clone();
    let ep = SiteServiceEndpoint {
        host: params.host.clone(),
        port: params.port,
        protocol: params.protocol,
    };
    let ep_for_db = ep.clone();
    state
        .db
        .call(move |db| crate::runtime::site_services::add_endpoint(db, &name, &ep_for_db))
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("failed to add endpoint: {e}")))?;

    // r[impl service.site.lifecycle.events]
    ctx.events.site_service_endpoint_added(
        params.name.as_str(),
        &params.host,
        params.port,
        params.protocol.as_str(),
    );

    state.tick_notify.notify_one();
    Ok(json!({ "added": true }))
}

// r[impl service.site.lifecycle]
pub(crate) fn remove_site_service_endpoint(
    state: &OiState,
    params: SiteServiceEndpointParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name = params.name.clone();
    let ep = SiteServiceEndpoint {
        host: params.host.clone(),
        port: params.port,
        protocol: params.protocol,
    };
    let removed = state
        .db
        .call(move |db| crate::runtime::site_services::remove_endpoint(db, &name, &ep))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to remove endpoint: {e}"),
            )
        })?;

    if !removed {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "no endpoint {:?}:{} ({}) on site service {:?}",
                params.host,
                params.port,
                params.protocol,
                params.name.as_str(),
            ),
        ));
    }

    // r[impl service.site.lifecycle.events]
    ctx.events.site_service_endpoint_removed(
        params.name.as_str(),
        &params.host,
        params.port,
        params.protocol.as_str(),
    );

    state.tick_notify.notify_one();
    Ok(json!({ "removed": true }))
}

#[derive(Deserialize)]
pub(crate) struct MapExternalServiceParams {
    pub app: AppName,
    pub external_name: ExternalServiceName,
    pub target: ServiceRef,
}

// r[impl service.external.mapping.events]
pub(crate) fn map_external_service(
    state: &OiState,
    params: MapExternalServiceParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::external_service_mappings::{self, ExternalServiceMapping};

    let app = params.app.clone();
    let external_name = params.external_name.clone();
    let event_target = params.target.clone();
    let mapping = ExternalServiceMapping {
        app: params.app,
        external_name: params.external_name,
        target: params.target,
    };

    state
        .db
        .call(move |db| external_service_mappings::create(db, &mapping))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to create mapping: {e}"),
            )
        })?;

    ctx.events
        .external_service_mapped(&app, &external_name, &event_target);

    state.tick_notify.notify_one();
    Ok(json!({ "mapped": true }))
}

#[derive(Deserialize)]
pub(crate) struct UnmapExternalServiceParams {
    pub app: AppName,
    pub external_name: ExternalServiceName,
}

// r[impl service.external.mapping.events]
pub(crate) fn unmap_external_service(
    state: &OiState,
    params: UnmapExternalServiceParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::external_service_mappings;

    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(params.app.as_str()) {
            let def = entry.app.def.load();
            let has_slot = def.resources.keys().any(|id| {
                id.kind == crate::defs::resource::ResourceKind::ExternalService
                    && params.external_name == id.name.as_str()
            });
            if has_slot {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!(
                        "external service {:?} is declared by app {:?}; \
                         uninstall the app or remove the service reference first",
                        params.external_name, params.app
                    ),
                ));
            }
        }
    }

    let app_owned = params.app.clone();
    let external_name_owned = params.external_name.clone();
    let deleted = state
        .db
        .call(move |db| external_service_mappings::delete(db, &app_owned, &external_name_owned))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to delete mapping: {e}"),
            )
        })?;

    if !deleted {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "no mapping for {:?} in app {:?}",
                params.external_name, params.app
            ),
        ));
    }

    ctx.events
        .external_service_unmapped(&params.app, &params.external_name);

    Ok(json!({ "unmapped": true }))
}

// r[impl service.external.mapping.events]
pub(crate) fn remap_external_service(
    state: &OiState,
    params: MapExternalServiceParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::external_service_mappings::{self, ExternalServiceMapping};

    let mapping = ExternalServiceMapping {
        app: params.app.clone(),
        external_name: params.external_name.clone(),
        target: params.target.clone(),
    };

    let app_for_prev = params.app.clone();
    let external_name_for_prev = params.external_name.clone();
    let (updated, previous) = state
        .db
        .call(move |db| {
            let prev = external_service_mappings::get(db, &app_for_prev, &external_name_for_prev)?;
            let updated = external_service_mappings::update(db, &mapping)?;
            Ok::<_, rusqlite::Error>((updated, prev))
        })
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to update mapping: {e}"),
            )
        })?;

    if !updated {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "no existing mapping for {:?} in app {:?}",
                params.external_name, params.app
            ),
        ));
    }

    let previous = previous.expect("update succeeded so prior row existed");
    ctx.events.external_service_remapped(
        &params.app,
        &params.external_name,
        ExternalServiceMappingSnapshot {
            target: &params.target,
        },
        ExternalServiceMappingSnapshot {
            target: &previous.target,
        },
    );

    state.tick_notify.notify_one();
    Ok(json!({ "remapped": true }))
}

#[derive(Deserialize)]
pub(crate) struct ListExternalMappingsParams {
    pub app: Option<AppName>,
}

pub(crate) fn list_external_mappings(
    state: &OiState,
    params: ListExternalMappingsParams,
) -> HandlerResult {
    use crate::runtime::external_service_mappings;

    let app_filter = params.app.clone();
    let mappings = state
        .db
        .call(move |db| {
            if let Some(app) = &app_filter {
                external_service_mappings::list_for_app(db, app)
            } else {
                external_service_mappings::list_all(db)
            }
        })
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("failed to list mappings: {e}")))?;

    let items: Vec<_> = mappings
        .iter()
        .map(|m| {
            json!({
                "app": m.app,
                "external_name": m.external_name,
                "target": m.target,
            })
        })
        .collect();

    Ok(json!(items))
}

pub(crate) fn list_declared_external_services(state: &OiState) -> HandlerResult {
    use crate::defs::resource::ResourceKind;

    let reg = state.registry.read();
    let mut items: Vec<serde_json::Value> = reg
        .iter()
        .flat_map(|entry| {
            let def = entry.app.def.load();
            def.resources
                .keys()
                .filter(|id| id.kind == ResourceKind::ExternalService)
                .map(|id| json!({ "app": entry.name, "name": id.name.as_str() }))
                .collect::<Vec<_>>()
        })
        .collect();
    items.sort_by(|a, b| {
        let ak = (
            a["app"].as_str().unwrap_or(""),
            a["name"].as_str().unwrap_or(""),
        );
        let bk = (
            b["app"].as_str().unwrap_or(""),
            b["name"].as_str().unwrap_or(""),
        );
        ak.cmp(&bk)
    });
    Ok(json!(items))
}
