use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};
use seedling_protocol::events::ExternalServiceMappingSnapshot;
use seedling_protocol::names::{AppName, ExternalServiceName, ServiceRef, SiteServiceName};
use serde::Deserialize;
use serde_json::json;

use crate::oi::{handler::RequestCtx, state::OiState};
use crate::runtime::site_services::{
    self, EndpointChange, SiteServiceDef, SiteServiceEndpoint, SiteServiceProtocol,
};

fn internal(what: &str, e: impl std::fmt::Display) -> OiError {
    OiError::new(ErrorCode::Internal, format!("{what}: {e}"))
}

/// `not_found` unless the app is registered.
fn require_registered(state: &OiState, app: &AppName) -> Result<(), OiError> {
    if state.registry.read().get(app.as_str()).is_none() {
        return Err(OiError::new(
            ErrorCode::NotFound,
            format!("no app named {:?} is registered", app.as_str()),
        ));
    }
    Ok(())
}

/// List services that apps have marked with `service.exported()`.
pub(crate) fn list_exported(state: &OiState) -> HandlerResult {
    let registry = state.registry.read();
    let mut exported = Vec::new();

    for app_name in registry.names() {
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

    sort_by_app_then_service(&mut exported);
    Ok(json!(exported))
}

/// Order records by `app`, then `service_name`, each in ascending byte order.
fn sort_by_app_then_service(records: &mut [serde_json::Value]) {
    records.sort_by_key(|v| {
        (
            v["app"].as_str().unwrap_or_default().to_owned(),
            v["service_name"].as_str().unwrap_or_default().to_owned(),
        )
    });
}

/// Every named app service, whether or not it is exported. Mirrors
/// `volumes::list_app_volumes` for the service side.
pub(crate) fn list_app_services(state: &OiState) -> HandlerResult {
    let registry = state.registry.read();
    let mut services = Vec::new();

    for app_name in registry.names() {
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

    sort_by_app_then_service(&mut services);
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

/// Validate `remote_host` as an endpoint object requires: an IPv6 literal, an
/// IPv4 literal or a DNS name, none of which names the pod rather than the
/// operator's machine.
///
/// Names are resolved at runtime by the daemon's site-service resolver; the
/// reconciler turns failed resolution and missing-NAT64 routing into
/// structured faults rather than a mysterious blackhole.
// spec: SVCM#endpoint-objects
fn validate_remote_host(host: &str) -> Result<(), OiError> {
    let invalid = |why: &str| {
        OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("remote_host {host:?} {why}"),
        )
    };
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            if v4.is_loopback() || v4.is_unspecified() {
                return Err(invalid(
                    "is a loopback or unspecified address, which names the pod",
                ));
            }
            Ok(())
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            let mapped_loopback = v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback());
            if v6.is_loopback() || v6.is_unspecified() || mapped_loopback {
                return Err(invalid(
                    "is a loopback or unspecified address, which names the pod",
                ));
            }
            Ok(())
        }
        Err(_) if is_valid_dns_name(host) => Ok(()),
        Err(_) => Err(invalid(
            "must be an IPv6 literal, an IPv4 literal, or a valid DNS name",
        )),
    }
}

/// A DNS name as an endpoint's `remote_host` takes: the shape every DNS name in
/// the interface has, other than `localhost` and the `.localhost` names, which
/// a pod resolves to itself.
fn is_valid_dns_name(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower != "localhost" && !lower.ends_with(".localhost") && super::is_valid_dns_name(s)
}

/// Check an endpoint's members, naming the one that is wrong.
// spec: SVCM#endpoint-objects
fn validate_endpoint(ep: &SiteServiceEndpoint) -> Result<(), OiError> {
    super::validate_port("service_port", ep.service_port)?;
    super::validate_port("remote_port", ep.remote_port)?;
    validate_remote_host(&ep.remote_host)
}

#[derive(Deserialize)]
pub(crate) struct CreateSiteServiceParams {
    pub name: SiteServiceName,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub endpoints: Vec<EndpointParams>,
}

// spec: SVCM#site-service-creation
pub(crate) fn create_site_service(
    state: &OiState,
    params: CreateSiteServiceParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let endpoints: Vec<SiteServiceEndpoint> =
        params.endpoints.into_iter().map(Into::into).collect();
    for (i, ep) in endpoints.iter().enumerate() {
        validate_endpoint(ep)?;
        if endpoints[..i].contains(ep) {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "endpoints holds the endpoint {}/{} -> {}:{} twice",
                    ep.service_port, ep.protocol, ep.remote_host, ep.remote_port
                ),
            ));
        }
    }
    let def = SiteServiceDef {
        name: params.name.clone(),
        description: params.description.clone(),
        endpoints: endpoints.clone(),
        created_at: jiff::Timestamp::now().to_string(),
    };

    let created = state
        .db
        .call(move |db| site_services::create_if_absent(db, &def))
        .map_err(|e| internal("failed to store site service", e))?;
    if !created {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "a site service named {:?} already exists",
                params.name.as_str()
            ),
        ));
    }

    // spec: SITE#site-service-lifecycle
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

// spec: SVCM#site-service-deletion
pub(crate) fn delete_site_service(
    state: &OiState,
    params: DeleteSiteServiceParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name_owned = params.name.clone();
    match state
        .db
        .call(move |db| site_services::delete_unmapped(db, &name_owned))
        .map_err(|e| internal("failed to delete site service", e))?
    {
        site_services::Deletion::Deleted => {}
        site_services::Deletion::Absent => {
            return Err(OiError::new(
                ErrorCode::NotFound,
                format!("no site service named {:?}", params.name.as_str()),
            ));
        }
        site_services::Deletion::Mapped { app, external_name } => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "site service {:?} is the target of the mapping of external service {:?} \
                     of app {:?}; unmap or remap it first",
                    params.name.as_str(),
                    external_name.as_str(),
                    app.as_str(),
                ),
            ));
        }
    }

    // spec: SITE#site-service-lifecycle
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

// spec: SVCM#site-service-endpoint-changes
pub(crate) fn add_site_service_endpoint(
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
    validate_endpoint(&ep)?;
    let ep_for_db = ep.clone();
    match state
        .db
        .call(move |db| site_services::add_endpoint_checked(db, &name, &ep_for_db))
        .map_err(|e| internal("failed to add endpoint", e))?
    {
        EndpointChange::Done => {}
        EndpointChange::NoService => return Err(no_site_service(&params.name)),
        EndpointChange::Endpoint => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "site service {:?} already has the endpoint {}/{} -> {}:{}",
                    params.name.as_str(),
                    ep.service_port,
                    ep.protocol,
                    ep.remote_host,
                    ep.remote_port
                ),
            ));
        }
    }

    // spec: SITE#site-service-lifecycle
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

fn no_site_service(name: &SiteServiceName) -> OiError {
    OiError::new(
        ErrorCode::NotFound,
        format!("no site service named {:?}", name.as_str()),
    )
}

// spec: SVCM#site-service-endpoint-changes
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
    match state
        .db
        .call(move |db| site_services::remove_endpoint_checked(db, &name, &ep))
        .map_err(|e| internal("failed to remove endpoint", e))?
    {
        EndpointChange::Done => {}
        EndpointChange::NoService => return Err(no_site_service(&params.name)),
        EndpointChange::Endpoint => {
            return Err(OiError::new(
                ErrorCode::NotFound,
                format!(
                    "site service {:?} has no endpoint {}/{} -> {}:{}",
                    params.name.as_str(),
                    params.service_port,
                    params.protocol,
                    params.remote_host,
                    params.remote_port,
                ),
            ));
        }
    }

    // spec: SITE#site-service-lifecycle
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
// spec: SVCM#resolver-status
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
                "failed_families": e.failed_families.iter().map(|f| f.record_type()).collect::<Vec<_>>(),
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

/// A service target that must exist, as far as the registry can say: an app
/// target names a registered app whose current definition declares the service.
/// A site target is checked where the mapping is written, in the same step.
// spec: SVCM#service-targets
fn check_service_target(state: &OiState, target: &ServiceRef) -> Result<(), OiError> {
    use crate::defs::resource::ResourceKind;

    let ServiceRef::App { app, service } = target else {
        return Ok(());
    };
    let invalid = |why: String| OiError::new(ErrorCode::RequirementsInvalid, why);
    let reg = state.registry.read();
    let entry = reg
        .get(app.as_str())
        .ok_or_else(|| invalid(format!("no app named {:?} is registered", app.as_str())))?;
    let def = entry.app.def.load();
    let declared = def
        .resources
        .keys()
        .any(|id| id.kind == ResourceKind::Service && id.name.as_str() == service.as_str());
    if !declared {
        return Err(invalid(format!(
            "app {:?} declares no service named {:?}",
            app.as_str(),
            service.as_str()
        )));
    }
    Ok(())
}

fn site_target_missing(target: &ServiceRef) -> OiError {
    let name = match target {
        ServiceRef::Site { name } => name.as_str(),
        ServiceRef::App { .. } => unreachable!("only a site target can be missing from the table"),
    };
    OiError::new(
        ErrorCode::RequirementsInvalid,
        format!("no site service named {name:?}"),
    )
}

// spec: SVCM#mapping-creation
pub(crate) fn map_external_service(
    state: &OiState,
    params: MapExternalServiceParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::external_service_mappings::{self, ExternalServiceMapping, Written};

    require_registered(state, &params.app)?;
    check_service_target(state, &params.target)?;

    let app = params.app.clone();
    let external_name = params.external_name.clone();
    let event_target = params.target.clone();
    let mapping = ExternalServiceMapping {
        app: params.app,
        external_name: params.external_name,
        target: params.target,
    };

    let target = mapping.target.clone();
    match state
        .db
        .call(move |db| external_service_mappings::create_unique(db, &mapping))
        .map_err(|e| internal("failed to create mapping", e))?
    {
        Written::Done => {}
        Written::Duplicate | Written::Missing => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "external service {:?} of app {:?} is already mapped",
                    external_name.as_str(),
                    app.as_str()
                ),
            ));
        }
        Written::SiteTargetMissing => return Err(site_target_missing(&target)),
    }

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

// spec: SVCM#unmapping
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
        } else {
            return Err(OiError::new(
                ErrorCode::NotFound,
                format!("no app named {:?} is registered", params.app.as_str()),
            ));
        }
    }

    let app_owned = params.app.clone();
    let external_name_owned = params.external_name.clone();
    let deleted = state
        .db
        .call(move |db| external_service_mappings::delete(db, &app_owned, &external_name_owned))
        .map_err(|e| internal("failed to delete mapping", e))?;

    if !deleted {
        return Err(OiError::new(
            ErrorCode::NotFound,
            format!(
                "no mapping for {:?} in app {:?}",
                params.external_name, params.app
            ),
        ));
    }

    ctx.events
        .external_service_unmapped(&params.app, &params.external_name);

    state.tick_notify.notify_one();
    Ok(json!({ "unmapped": true }))
}

// spec: SVCM#remapping
pub(crate) fn remap_external_service(
    state: &OiState,
    params: MapExternalServiceParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::external_service_mappings::{self, ExternalServiceMapping, Written};

    require_registered(state, &params.app)?;
    check_service_target(state, &params.target)?;

    let mapping = ExternalServiceMapping {
        app: params.app.clone(),
        external_name: params.external_name.clone(),
        target: params.target.clone(),
    };

    let (written, previous) = state
        .db
        .call(move |db| external_service_mappings::update_existing(db, &mapping))
        .map_err(|e| internal("failed to update mapping", e))?;

    match written {
        Written::Done => {}
        Written::Missing | Written::Duplicate => {
            return Err(OiError::new(
                ErrorCode::NotFound,
                format!(
                    "no existing mapping for {:?} in app {:?}",
                    params.external_name, params.app
                ),
            ));
        }
        Written::SiteTargetMissing => return Err(site_target_missing(&params.target)),
    }

    let previous = previous.expect("a mapping that was updated had a previous one");
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

    // spec: SITE#site-service-lifecycle
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
        assert_eq!(code, "not_found");
    }

    // spec: SITE#endpoint-addressing
    // Port 0 deserialises happily into a plain `u16` but is not a routable
    // listener or backend port; it used to reach the site-proxy config as
    // `listen :0` / dial `:0` rather than being named as a bad field here.
    #[test]
    fn site_service_create_rejects_port_zero() {
        for (svc, remote) in [(0, 5432), (5432, 0)] {
            let oi = TestOi::new();
            let (code, msg) = oi
                .call(
                    "/services/site/create",
                    json!({
                        "name": "zero-port",
                        "endpoints": [{
                            "service_port": svc,
                            "protocol": "tcp",
                            "remote_host": "10.0.0.5",
                            "remote_port": remote,
                        }],
                    }),
                )
                .unwrap_err();
            assert_eq!(code, "requirements_invalid", "svc={svc} remote={remote}");
            assert!(
                msg.contains("got 0"),
                "message should name the value: {msg}"
            );
        }
    }

    // spec: SITE#endpoint-addressing
    #[test]
    fn adding_an_endpoint_rejects_port_zero() {
        let oi = TestOi::new();
        oi.call("/services/site/create", json!({ "name": "svc" }))
            .unwrap();
        let (code, _) = oi
            .call(
                "/services/site/endpoint/add",
                json!({
                    "name": "svc",
                    "service_port": 0,
                    "protocol": "tcp",
                    "remote_host": "10.0.0.5",
                    "remote_port": 5432,
                }),
            )
            .unwrap_err();
        assert_eq!(code, "requirements_invalid");
    }

    // spec: SITE#endpoint-addressing
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

    // spec: SITE#site-service-lifecycle
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
        assert_eq!(
            oi.call("/services/site/endpoint/add", ep.clone()).unwrap()["added"],
            true
        );
        assert_eq!(
            site_services(&oi)[0]["endpoints"].as_array().unwrap().len(),
            1
        );

        assert_eq!(
            oi.call("/services/site/endpoint/remove", ep.clone())
                .unwrap()["removed"],
            true
        );
        assert!(
            site_services(&oi)[0]["endpoints"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let (code, _) = oi.call("/services/site/endpoint/remove", ep).unwrap_err();
        assert_eq!(code, "not_found");
    }

    // spec: SITE#site-service-lifecycle
    #[test]
    fn site_service_delete_blocked_while_mapped() {
        let oi = TestOi::new();
        register_app(&oi, "consumer", "");
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
        assert!(
            msg.contains("consumer") && msg.contains("api-slot"),
            "{msg}"
        );

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

    // spec: SITE#endpoint-addressing
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

    // spec: SITE#external-mappings
    #[test]
    fn external_service_mapping_map_remap_unmap_flow() {
        let oi = TestOi::new();
        register_app(&oi, "consumer", "");
        register_app(&oi, "postgres", r#"app.service("sql");"#);
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
        assert_eq!(code, "not_found");

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
        assert_eq!(code, "not_found");
    }

    #[test]
    fn declared_external_services_guard_unmap() {
        let oi = TestOi::new();
        assert_eq!(
            oi.call("/services/external/declared", json!({})).unwrap(),
            json!([])
        );

        register_app(
            &oi,
            "web-app",
            r#"let auth = app.external_service("auth");"#,
        );
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

    fn endpoint(service_port: u16, host: &str, remote_port: u16) -> Value {
        json!({
            "service_port": service_port,
            "protocol": "tcp",
            "remote_host": host,
            "remote_port": remote_port,
        })
    }

    // spec: SVCM#endpoint-objects
    #[test]
    fn remote_hosts_that_name_the_pod_or_are_malformed_are_refused() {
        for host in [
            "::1",
            "::",
            "::ffff:127.0.0.1",
            "::ffff:127.255.0.9",
            "127.0.0.1",
            "127.1.2.3",
            "0.0.0.0",
            "localhost",
            "LOCALHOST",
            "db.localhost",
            "Db.LocalHost",
            "[2001:db8::1]",
            "fe80::1%eth0",
            "010.0.0.1",
            "1.2.3",
            ".example.com",
            "example.com.",
            "a..b",
            "-lead.example",
            "trail-.example",
        ] {
            let oi = TestOi::new();
            let (code, msg) = oi
                .call(
                    "/services/site/create",
                    json!({ "name": "bad-host", "endpoints": [endpoint(80, host, 80)] }),
                )
                .unwrap_err();
            assert_eq!(code, "requirements_invalid", "{host}: {msg}");
            assert!(msg.contains("remote_host"), "{host}: {msg}");
        }
        for host in [
            "::2",
            "::ffff:128.0.0.1",
            "126.255.255.255",
            "db.example.com",
        ] {
            let oi = TestOi::new();
            oi.call(
                "/services/site/create",
                json!({ "name": "good-host", "endpoints": [endpoint(80, host, 80)] }),
            )
            .unwrap_or_else(|e| panic!("{host}: {e:?}"));
        }
    }

    // spec: SVCM#site-service-creation
    #[test]
    fn creating_a_site_service_checks_the_name_and_the_endpoints() {
        let oi = TestOi::new();
        oi.call("/services/site/create", json!({ "name": "lab-db" }))
            .unwrap();
        let (code, _) = oi
            .call("/services/site/create", json!({ "name": "lab-db" }))
            .unwrap_err();
        assert_eq!(code, "requirements_invalid", "a name already taken");

        let (code, _) = oi
            .call(
                "/services/site/create",
                json!({
                    "name": "twice",
                    "endpoints": [endpoint(80, "10.0.0.1", 80), endpoint(80, "10.0.0.1", 80)],
                }),
            )
            .unwrap_err();
        assert_eq!(code, "requirements_invalid", "two identical endpoints");
        assert_eq!(site_services(&oi).len(), 1, "nothing was stored");

        // The same pair on different remotes is one service across backends.
        oi.call(
            "/services/site/create",
            json!({
                "name": "pooled",
                "endpoints": [endpoint(80, "10.0.0.1", 80), endpoint(80, "10.0.0.2", 80)],
            }),
        )
        .unwrap();
    }

    // spec: SVCM#site-service-endpoint-changes
    #[test]
    fn endpoint_changes_check_the_service_and_the_endpoint() {
        let oi = TestOi::new();
        let with = |name: &str| {
            let mut e = endpoint(53, "10.0.0.5", 53);
            e["name"] = json!(name);
            e
        };
        let (code, _) = oi
            .call("/services/site/endpoint/add", with("ghost"))
            .unwrap_err();
        assert_eq!(code, "not_found", "adding to a service that is not there");
        let (code, _) = oi
            .call("/services/site/endpoint/remove", with("ghost"))
            .unwrap_err();
        assert_eq!(
            code, "not_found",
            "removing from a service that is not there"
        );

        oi.call("/services/site/create", json!({ "name": "dns" }))
            .unwrap();
        oi.call("/services/site/endpoint/add", with("dns")).unwrap();
        let (code, _) = oi
            .call("/services/site/endpoint/add", with("dns"))
            .unwrap_err();
        assert_eq!(code, "requirements_invalid", "an endpoint it already has");
        assert_eq!(
            site_services(&oi)[0]["endpoints"].as_array().unwrap().len(),
            1
        );
    }

    // spec: SVCM#mapping-creation
    #[test]
    fn mapping_checks_the_app_the_target_and_duplicates() {
        let oi = TestOi::new();
        let map = |app: &str, target: Value| {
            oi.call(
                "/services/external/map",
                json!({ "app": app, "external_name": "dbsvc", "target": target }),
            )
        };
        let site = |name: &str| json!({ "kind": "site", "name": name });

        let (code, _) = map("ghost", site("lab")).unwrap_err();
        assert_eq!(code, "not_found", "an app that is not registered");

        register_app(&oi, "consumer", "");
        let (code, _) = map("consumer", site("lab")).unwrap_err();
        assert_eq!(
            code, "requirements_invalid",
            "a site service that is not there"
        );
        let (code, _) = map(
            "consumer",
            json!({ "kind": "app", "app": "ghost", "service": "sql" }),
        )
        .unwrap_err();
        assert_eq!(
            code, "requirements_invalid",
            "a target app that is not registered"
        );

        register_app(&oi, "postgres", r#"app.service("sql");"#);
        let (code, _) = map(
            "consumer",
            json!({ "kind": "app", "app": "postgres", "service": "missing" }),
        )
        .unwrap_err();
        assert_eq!(
            code, "requirements_invalid",
            "a service the app does not declare"
        );
        map(
            "consumer",
            json!({ "kind": "app", "app": "postgres", "service": "sql" }),
        )
        .unwrap();

        oi.call("/services/site/create", json!({ "name": "lab" }))
            .unwrap();
        let (code, _) = map("consumer", site("lab")).unwrap_err();
        assert_eq!(code, "requirements_invalid", "already mapped");
    }

    // spec: SVCM#remapping
    #[test]
    fn remapping_checks_the_app_the_mapping_and_the_target() {
        let oi = TestOi::new();
        oi.call("/services/site/create", json!({ "name": "lab" }))
            .unwrap();
        let remap = |app: &str, target: Value| {
            oi.call(
                "/services/external/remap",
                json!({ "app": app, "external_name": "dbsvc", "target": target }),
            )
        };
        let (code, _) = remap("ghost", json!({ "kind": "site", "name": "lab" })).unwrap_err();
        assert_eq!(code, "not_found", "an app that is not registered");

        register_app(&oi, "consumer", "");
        oi.call(
            "/services/external/map",
            json!({
                "app": "consumer",
                "external_name": "dbsvc",
                "target": { "kind": "site", "name": "lab" },
            }),
        )
        .unwrap();
        let (code, _) = remap("consumer", json!({ "kind": "site", "name": "ghost" })).unwrap_err();
        assert_eq!(code, "requirements_invalid");
        let list = oi.call("/services/external/list", json!({})).unwrap();
        assert_eq!(list[0]["target"]["name"], "lab", "unchanged");
    }

    // spec: SVCM#unmapping
    #[test]
    fn unmapping_for_an_app_that_is_not_registered_is_not_found_and_begins_an_iteration() {
        let oi = TestOi::new();
        let (code, _) = oi
            .call(
                "/services/external/unmap",
                json!({ "app": "ghost", "external_name": "dbsvc" }),
            )
            .unwrap_err();
        assert_eq!(code, "not_found");

        register_app(&oi, "consumer", "");
        oi.call("/services/site/create", json!({ "name": "lab" }))
            .unwrap();
        oi.call(
            "/services/external/map",
            json!({
                "app": "consumer",
                "external_name": "dbsvc",
                "target": { "kind": "site", "name": "lab" },
            }),
        )
        .unwrap();
        oi.block_on(oi.state.tick_notify.notified());
        oi.call(
            "/services/external/unmap",
            json!({ "app": "consumer", "external_name": "dbsvc" }),
        )
        .unwrap();
        oi.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                oi.state.tick_notify.notified(),
            )
            .await
            .expect("an iteration was asked for")
        });
    }

    // spec: SVCM#app-service-listing
    #[test]
    fn app_services_are_listed_by_app_then_service() {
        let oi = TestOi::new();
        register_app(
            &oi,
            "zeta",
            r#"app.service("aaa").exported(#{ description: "x" });"#,
        );
        register_app(
            &oi,
            "alpha",
            r#"app.service("zzz").exported(#{ description: "x" }); app.service("bbb").exported(#{ description: "x" });"#,
        );
        for method in ["/services/app/list", "/services/exported/list"] {
            let list = oi.call(method, json!({})).unwrap();
            let keys: Vec<(String, String)> = list
                .as_array()
                .unwrap()
                .iter()
                .map(|s| {
                    (
                        s["app"].as_str().unwrap().to_owned(),
                        s["service_name"].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
            assert_eq!(
                keys,
                [
                    ("alpha".to_owned(), "bbb".to_owned()),
                    ("alpha".to_owned(), "zzz".to_owned()),
                    ("zeta".to_owned(), "aaa".to_owned()),
                ],
                "{method}"
            );
        }
    }
}
