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
    pub service_port: u16,
    pub protocol: SiteServiceProtocol,
    pub remote_host: String,
    pub remote_port: u16,
}

impl From<EndpointParams> for SiteServiceEndpoint {
    fn from(p: EndpointParams) -> Self {
        Self {
            service_port: p.service_port,
            protocol: p.protocol,
            remote_host: p.remote_host,
            remote_port: p.remote_port,
        }
    }
}

/// Validate `remote_host`: accept any IP literal (v4 or v6) or a syntactically
/// valid DNS name. Names are resolved at runtime by the daemon's
/// site-service resolver; the reconciler turns failed resolution and
/// missing-NAT64 routing into structured faults rather than a mysterious
/// blackhole.
// r[impl service.site.address]
fn validate_remote_host(host: &str) -> Result<(), OiError> {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Ok(());
    }
    if !is_valid_dns_name(host) {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "remote_host {host:?} must be an IPv6 literal, an IPv4 literal, \
                 or a syntactically valid DNS name"
            ),
        ));
    }
    Ok(())
}

/// Checks whether `s` is a syntactically plausible DNS name: 1–253 chars
/// total, dot-separated labels of 1–63 chars each that match
/// `[A-Za-z0-9-]+` and don't start or end with `-`. Rejects trailing dots,
/// underscore labels, and `localhost` (the daemon resolves on the host;
/// localhost would loop back into the daemon's own networking).
fn is_valid_dns_name(s: &str) -> bool {
    if s.is_empty() || s.len() > 253 || s.eq_ignore_ascii_case("localhost") {
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
    for ep in &params.endpoints {
        validate_remote_host(&ep.remote_host)?;
    }
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
            ep.service_port,
            ep.protocol.as_str(),
            &ep.remote_host,
            ep.remote_port,
        );
    }

    if let Some(r) = state.site_resolver.as_deref() {
        r.kick();
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
                        "service_port": e.service_port,
                        "protocol": e.protocol.as_str(),
                        "remote_host": e.remote_host,
                        "remote_port": e.remote_port,
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

    if let Some(r) = state.site_resolver.as_deref() {
        r.kick();
    }
    state.tick_notify.notify_one();
    Ok(json!({ "deleted": true }))
}

#[derive(Deserialize)]
pub(crate) struct SiteServiceEndpointParams {
    pub name: SiteServiceName,
    pub service_port: u16,
    pub protocol: SiteServiceProtocol,
    pub remote_host: String,
    pub remote_port: u16,
}

// r[impl service.site.lifecycle]
pub(crate) fn add_site_service_endpoint(
    state: &OiState,
    params: SiteServiceEndpointParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    validate_remote_host(&params.remote_host)?;
    let name = params.name.clone();
    let ep = SiteServiceEndpoint {
        service_port: params.service_port,
        protocol: params.protocol,
        remote_host: params.remote_host.clone(),
        remote_port: params.remote_port,
    };
    let ep_for_db = ep.clone();
    state
        .db
        .call(move |db| crate::runtime::site_services::add_endpoint(db, &name, &ep_for_db))
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("failed to add endpoint: {e}")))?;

    // r[impl service.site.lifecycle.events]
    ctx.events.site_service_endpoint_added(
        params.name.as_str(),
        params.service_port,
        params.protocol.as_str(),
        &params.remote_host,
        params.remote_port,
    );

    if let Some(r) = state.site_resolver.as_deref() {
        r.kick();
    }
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
        service_port: params.service_port,
        protocol: params.protocol,
        remote_host: params.remote_host.clone(),
        remote_port: params.remote_port,
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
                "no endpoint {}:{} ({}) -> {}:{} on site service {:?}",
                params.name.as_str(),
                params.service_port,
                params.protocol,
                params.remote_host,
                params.remote_port,
                params.name.as_str(),
            ),
        ));
    }

    // r[impl service.site.lifecycle.events]
    ctx.events.site_service_endpoint_removed(
        params.name.as_str(),
        params.service_port,
        params.protocol.as_str(),
        &params.remote_host,
        params.remote_port,
    );

    if let Some(r) = state.site_resolver.as_deref() {
        r.kick();
    }
    state.tick_notify.notify_one();
    Ok(json!({ "removed": true }))
}

/// Snapshot of the site-service DNS resolver cache. Operators inspect
/// this to confirm what addresses a DNS-named endpoint is currently
/// routing to.
// r[impl service.site.address]
pub(crate) fn site_service_resolver_status(state: &OiState) -> HandlerResult {
    let entries = state
        .site_resolver
        .as_deref()
        .map(|r| r.status())
        .unwrap_or_default();
    let items: Vec<_> = entries
        .iter()
        .map(|e| {
            json!({
                "host": e.host,
                "aaaa": e.aaaa.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                "a":    e.a.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                "last_attempt_failed": e.last_attempt_failed,
                "age_seconds": e.age.as_secs(),
                "ttl_remaining_seconds": e.ttl_remaining.as_secs(),
            })
        })
        .collect();
    Ok(json!({ "entries": items }))
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
    use crate::defs::resource::{Resource, ResourceKind};

    let reg = state.registry.read();
    let mut items: Vec<serde_json::Value> = reg
        .iter()
        .flat_map(|entry| {
            let def = entry.app.def.load();
            def.resources
                .iter()
                .filter(|(id, _)| id.kind == ResourceKind::ExternalService)
                .map(|(id, resource)| {
                    let mut item = json!({ "app": entry.name, "name": id.name.as_str() });
                    if let Resource::ExternalService(es) = resource
                        && let Some(desc) = es.def.lock().description.clone()
                    {
                        item["description"] = json!(desc);
                    }
                    item
                })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_ipv6_literal() {
        validate_remote_host("2001:db8::1").unwrap();
        validate_remote_host("fd5e::42").unwrap();
    }

    #[test]
    fn validate_accepts_ipv4_literal() {
        validate_remote_host("10.0.0.1").unwrap();
        validate_remote_host("192.0.2.10").unwrap();
    }

    #[test]
    fn validate_accepts_dns_name() {
        validate_remote_host("db.example.com").unwrap();
        validate_remote_host("internal-host").unwrap();
        validate_remote_host("a-b-c.example.co.uk").unwrap();
    }

    #[test]
    fn validate_rejects_localhost() {
        validate_remote_host("localhost").unwrap_err();
        validate_remote_host("LocalHost").unwrap_err();
    }

    #[test]
    fn validate_rejects_underscore_label() {
        validate_remote_host("bad_underscore.example").unwrap_err();
    }

    #[test]
    fn validate_rejects_empty() {
        validate_remote_host("").unwrap_err();
    }

    #[test]
    fn validate_rejects_label_starting_with_dash() {
        validate_remote_host("-bad.example").unwrap_err();
    }

    #[test]
    fn validate_rejects_numeric_only_string() {
        // Looks like an IP but has only three parts; falls into the DNS
        // shape, where all-numeric labels are rejected.
        validate_remote_host("123.456").unwrap_err();
    }

    use serde_json::{Value, json};

    use crate::oi::test_support::TestOi;

    fn register_app(oi: &TestOi, name: &str, script: &str) {
        oi.call("/apps/create", json!({ "app": name, "script": script }))
            .expect("app registration succeeds");
    }

    fn site_services(oi: &TestOi) -> Vec<Value> {
        oi.call("/services/site/list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    // r[verify service.site.lifecycle]
    #[test]
    fn site_service_create_list_delete_roundtrip() {
        let oi = TestOi::new();
        let created = oi
            .call(
                "/services/site/create",
                json!({
                    "name": "lab-db",
                    "description": "lab database",
                    "endpoints": [{
                        "service_port": 5432,
                        "protocol": "tcp",
                        "remote_host": "10.0.0.5",
                        "remote_port": 5432,
                    }],
                }),
            )
            .unwrap();
        assert_eq!(created["created"], true);

        let listed = site_services(&oi);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["name"], "lab-db");
        assert_eq!(listed[0]["description"], "lab database");
        let eps = listed[0]["endpoints"].as_array().unwrap();
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0]["service_port"], 5432);
        assert_eq!(eps[0]["protocol"], "tcp");
        assert_eq!(eps[0]["remote_host"], "10.0.0.5");

        let deleted = oi
            .call("/services/site/delete", json!({ "name": "lab-db" }))
            .unwrap();
        assert_eq!(deleted["deleted"], true);
        assert!(site_services(&oi).is_empty());

        let (code, _) = oi
            .call("/services/site/delete", json!({ "name": "lab-db" }))
            .unwrap_err();
        assert_eq!(code, "requirements_invalid");
    }

    // r[verify service.site.address]
    #[test]
    fn site_service_create_rejects_invalid_remote_host() {
        let oi = TestOi::new();
        let (code, msg) = oi
            .call(
                "/services/site/create",
                json!({
                    "name": "loop-svc",
                    "endpoints": [{
                        "service_port": 80,
                        "protocol": "http",
                        "remote_host": "localhost",
                        "remote_port": 8080,
                    }],
                }),
            )
            .unwrap_err();
        assert_eq!(code, "requirements_invalid");
        assert!(msg.contains("localhost"), "{msg}");
        assert!(site_services(&oi).is_empty());
    }

    // r[verify service.site.lifecycle]
    #[test]
    fn site_service_endpoint_add_and_remove() {
        let oi = TestOi::new();
        oi.call("/services/site/create", json!({ "name": "multi-ep" }))
            .unwrap();

        let ep = json!({
            "name": "multi-ep",
            "service_port": 53,
            "protocol": "udp",
            "remote_host": "dns.example.com",
            "remote_port": 5353,
        });
        assert_eq!(oi.call("/services/site/endpoint/add", ep.clone()).unwrap()["added"], true);
        assert_eq!(site_services(&oi)[0]["endpoints"].as_array().unwrap().len(), 1);

        assert_eq!(
            oi.call("/services/site/endpoint/remove", ep.clone()).unwrap()["removed"],
            true
        );
        assert!(site_services(&oi)[0]["endpoints"].as_array().unwrap().is_empty());

        let (code, _) = oi
            .call("/services/site/endpoint/remove", ep)
            .unwrap_err();
        assert_eq!(code, "requirements_invalid");
    }

    // r[verify service.site.lifecycle]
    #[test]
    fn site_service_delete_blocked_while_mapped() {
        let oi = TestOi::new();
        oi.call("/services/site/create", json!({ "name": "shared-api" }))
            .unwrap();
        oi.call(
            "/services/external/map",
            json!({
                "app": "consumer",
                "external_name": "api-slot",
                "target": { "kind": "site", "name": "shared-api" },
            }),
        )
        .unwrap();

        let (code, msg) = oi
            .call("/services/site/delete", json!({ "name": "shared-api" }))
            .unwrap_err();
        assert_eq!(code, "requirements_invalid");
        assert!(msg.contains("still mapped"), "{msg}");

        oi.call(
            "/services/external/unmap",
            json!({ "app": "consumer", "external_name": "api-slot" }),
        )
        .unwrap();
        assert_eq!(
            oi.call("/services/site/delete", json!({ "name": "shared-api" }))
                .unwrap()["deleted"],
            true
        );
    }

    // r[verify service.site.address]
    #[test]
    fn resolver_status_empty_without_resolver() {
        let oi = TestOi::new();
        let status = oi
            .call("/services/site/resolver-status", json!({}))
            .unwrap();
        assert_eq!(status["entries"], json!([]));
    }

    #[test]
    fn app_and_exported_service_lists_reflect_registered_apps() {
        let oi = TestOi::new();
        assert_eq!(oi.call("/services/app/list", json!({})).unwrap(), json!([]));
        assert_eq!(
            oi.call("/services/exported/list", json!({})).unwrap(),
            json!([])
        );

        register_app(
            &oi,
            "svc-app",
            r#"
            let api = app.service("api").exported(#{ description: "main API" });
            api.http(8080);
            let metrics = app.service("metrics");
            "#,
        );

        let all = oi.call("/services/app/list", json!({})).unwrap();
        let all = all.as_array().unwrap();
        assert_eq!(all.len(), 2, "{all:?}");
        let api = all.iter().find(|s| s["service_name"] == "api").unwrap();
        assert_eq!(api["exported"], true);
        assert_eq!(api["http"], true);
        assert_eq!(api["description"], "main API");
        let metrics = all.iter().find(|s| s["service_name"] == "metrics").unwrap();
        assert_eq!(metrics["exported"], false);
        assert_eq!(metrics["http"], false);

        let exported = oi.call("/services/exported/list", json!({})).unwrap();
        let exported = exported.as_array().unwrap();
        assert_eq!(exported.len(), 1);
        assert_eq!(exported[0]["service_name"], "api");
        assert_eq!(exported[0]["app"], "svc-app");
    }

    // r[verify service.external.mapping.events]
    #[test]
    fn external_service_mapping_map_remap_unmap_flow() {
        let oi = TestOi::new();
        oi.call("/services/site/create", json!({ "name": "lab-db" }))
            .unwrap();

        oi.call(
            "/services/external/map",
            json!({
                "app": "consumer",
                "external_name": "db-conn",
                "target": { "kind": "site", "name": "lab-db" },
            }),
        )
        .unwrap();

        let list = oi.call("/services/external/list", json!({})).unwrap();
        let list = list.as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["external_name"], "db-conn");
        assert_eq!(list[0]["target"]["kind"], "site");

        assert_eq!(
            oi.call("/services/external/list", json!({ "app": "someone-else" }))
                .unwrap(),
            json!([])
        );

        oi.call(
            "/services/external/remap",
            json!({
                "app": "consumer",
                "external_name": "db-conn",
                "target": { "kind": "app", "app": "postgres", "service": "sql" },
            }),
        )
        .unwrap();
        let list = oi.call("/services/external/list", json!({})).unwrap();
        assert_eq!(list[0]["target"]["kind"], "app");

        let (code, _) = oi
            .call(
                "/services/external/remap",
                json!({
                    "app": "consumer",
                    "external_name": "no-such",
                    "target": { "kind": "site", "name": "lab-db" },
                }),
            )
            .unwrap_err();
        assert_eq!(code, "requirements_invalid");

        assert_eq!(
            oi.call(
                "/services/external/unmap",
                json!({ "app": "consumer", "external_name": "db-conn" }),
            )
            .unwrap()["unmapped"],
            true
        );
        let (code, _) = oi
            .call(
                "/services/external/unmap",
                json!({ "app": "consumer", "external_name": "db-conn" }),
            )
            .unwrap_err();
        assert_eq!(code, "requirements_invalid");
    }

    #[test]
    fn declared_external_services_guard_unmap() {
        let oi = TestOi::new();
        assert_eq!(
            oi.call("/services/external/declared", json!({})).unwrap(),
            json!([])
        );

        register_app(&oi, "web-app", r#"let auth = app.external_service("auth");"#);
        oi.call("/services/site/create", json!({ "name": "sso" }))
            .unwrap();

        let declared = oi.call("/services/external/declared", json!({})).unwrap();
        let declared = declared.as_array().unwrap();
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0]["app"], "web-app");
        assert_eq!(declared[0]["name"], "auth");

        oi.call(
            "/services/external/map",
            json!({
                "app": "web-app",
                "external_name": "auth",
                "target": { "kind": "site", "name": "sso" },
            }),
        )
        .unwrap();

        let (code, msg) = oi
            .call(
                "/services/external/unmap",
                json!({ "app": "web-app", "external_name": "auth" }),
            )
            .unwrap_err();
        assert_eq!(code, "requirements_invalid");
        assert!(msg.contains("declared by app"), "{msg}");
    }
}
