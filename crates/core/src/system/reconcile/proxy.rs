use ipnet::Ipv6Net;
use seedling_protocol::names::AppName;

use crate::{
    defs::{
        app::AppDef,
        ingress::IngressDef,
        pod::PodDef,
        resource::{Resource, ResourceKind},
        service::{HttpServiceDef, ProxySettings, RateLimitScope, ResolvedRouteProxy, resolve},
    },
    runtime::{
        InstanceRegistry, desired::DesiredState, identity::ResourceInstance,
        lifecycle::LifecycleState, registry::RegistryError,
    },
    system::{
        translate::proxy::{HttpForwardRoute, ServiceUpstream, instance_ipv6},
        types::{L4Proto, L4Route, RouteProxy, RouteZone},
    },
};

use super::RunningPod;

pub(super) struct ProxyBuildOutput {
    pub pairs: Vec<(IngressDef, ServiceUpstream)>,
    pub l4_routes: Vec<L4Route>,
    /// Observations to persist regardless of apply outcome (ingress_configured + uninstall).
    pub observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    /// Observations to persist only after successful apply (ingress_ready).
    pub ready_observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
}

// r[autonomous.ingress]
// r[fault.non-blocking]
pub(super) fn collect(
    snapshot: &AppDef,
    desired: &DesiredState,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    app_name: &AppName,
    running_pods: &[RunningPod],
) -> Result<ProxyBuildOutput, RegistryError> {
    let mut observations: Vec<(ResourceInstance, &'static str, serde_json::Value)> = Vec::new();
    let mut ready_observations: Vec<(ResourceInstance, &'static str, serde_json::Value)> =
        Vec::new();
    let mut pairs: Vec<(IngressDef, ServiceUpstream)> = Vec::new();
    let mut l4_routes: Vec<L4Route> = Vec::new();

    for resource in snapshot.resources.values() {
        let ingress = match resource {
            Resource::Ingress(i) => i,
            _ => continue,
        };

        let def = ingress.def.lock().clone();
        let svc_name = ingress.service.name.as_str();

        let svc_instance =
            registry.get_or_create_singleton(app_name, ResourceKind::Service, Some(svc_name))?;
        let service_ip = instance_ipv6(node_prefix, &svc_instance);

        let upstream_port = find_upstream_port(snapshot, svc_name, def.port.get());

        // The ingress's resource name is "<hostname>:<port>" (multiple
        // ingresses can hang off one service). The instance must be keyed
        // by the same name the AppDef uses, otherwise the lifecycle
        // lookup in `effective_app_status` finds nothing and the app
        // sits at Degraded with the ingress wedged at Pending.
        let ingress_instance = registry.get_or_create_singleton(
            app_name,
            ResourceKind::Ingress,
            Some(ingress.name.as_str()),
        )?;

        if def.http_terminate.is_none() {
            let upstream_url = format!("[{}]:{}", service_ip, upstream_port);

            if def.dtls {
                l4_routes.push(L4Route {
                    port: def.port.get(),
                    proto: L4Proto::Tcp,
                    upstreams: vec![upstream_url.clone()],
                });
                l4_routes.push(L4Route {
                    port: def.port.get(),
                    proto: L4Proto::Udp,
                    upstreams: vec![upstream_url],
                });
            } else {
                l4_routes.push(L4Route {
                    port: def.port.get(),
                    proto: L4Proto::Tcp,
                    upstreams: vec![upstream_url],
                });
            };

            observations.push((
                ingress_instance.clone(),
                "ingress_configured",
                serde_json::json!({}),
            ));
            ready_observations.push((ingress_instance, "ingress_ready", serde_json::json!({})));

            continue;
        }

        observations.push((
            ingress_instance.clone(),
            "ingress_configured",
            serde_json::json!({}),
        ));

        ready_observations.push((ingress_instance, "ingress_ready", serde_json::json!({})));

        // r[impl service.http.route.routing]
        // Per-prefix routes derived from pod http_bindings. Caddy will
        // longest-prefix match these; the service IP fallback below only
        // kicks in if the service has no http_bindings at all (e.g. an
        // HTTPS-fronted TCP service).
        let routes = collect_http_routes(snapshot, svc_name, running_pods);

        pairs.push((
            def,
            ServiceUpstream {
                routes,
                service_ip,
                service_port: upstream_port,
                proxy: service_level_proxy(snapshot, svc_name),
            },
        ));
    }

    for dr in &desired.resources {
        let ingress = match &dr.definition {
            Resource::Ingress(i) => i,
            _ => continue,
        };
        if dr.desired != LifecycleState::Unscheduled {
            continue;
        }
        let ingress_instance = registry.get_or_create_singleton(
            app_name,
            ResourceKind::Ingress,
            Some(ingress.name.as_str()),
        )?;
        observations.push((ingress_instance.clone(), "stop_sent", serde_json::json!({})));
        observations.push((
            ingress_instance.clone(),
            "ingress_removed",
            serde_json::json!({}),
        ));
        observations.push((
            ingress_instance.clone(),
            "ingress_cleaned_up",
            serde_json::json!({}),
        ));
    }

    Ok(ProxyBuildOutput {
        pairs,
        l4_routes,
        observations,
        ready_observations,
    })
}

/// Scan all Deployment and Job pod definitions for the first TCP or HTTP
/// binding that references `service_name`, and return the pod port.
///
/// Falls back to `fallback_port` (the ingress's declared port) when no
/// binding is found.
///
/// # Port translation
/// Service DNAT rules handle translation from service port to pod port,
/// so the returned port is always the service-facing port.
fn find_upstream_port(snapshot: &AppDef, service_name: &str, fallback_port: u16) -> u16 {
    for resource in snapshot.resources.values() {
        let result = match resource {
            Resource::Deployment(dep) => {
                let def = dep.def.lock();
                let pod = def.pod.lock();
                scan_pod_for_port(&pod, service_name)
            }
            Resource::Job(job) => {
                let def = job.def.lock();
                let pod = def.pod.lock();
                scan_pod_for_port(&pod, service_name)
            }
            _ => None,
        };

        if let Some(port) = result {
            return port;
        }
    }

    fallback_port
}

fn scan_pod_for_port(pod: &PodDef, service_name: &str) -> Option<u16> {
    for b in &pod.tcp_bindings {
        if b.service_port.service.name().as_str() == service_name {
            return Some(b.service_port.port.get());
        }
    }

    for b in &pod.http_bindings {
        if b.route.http.service.name().as_str() == service_name {
            return Some(b.route.http.port.get());
        }
    }

    None
}

/// The `HttpServiceDef` an app declared for `service_name`, if any. Both the
/// app's own services and external-service slots can back an HTTP route.
fn proxy_settings_for(snapshot: &AppDef, service_name: &str) -> (ProxySettings, RouteMap) {
    snapshot
        .resources
        .values()
        .find_map(|r| match r {
            Resource::Service(s) if s.name.as_str() == service_name => {
                let def = s.def.lock();
                Some((def.proxy_settings(), routes_of(def.http.as_ref())))
            }
            Resource::ExternalService(e) if e.name.as_str() == service_name => {
                let def = e.def.lock();
                Some((def.proxy_settings(), routes_of(def.http.as_ref())))
            }
            _ => None,
        })
        .unwrap_or_default()
}

type RouteMap = std::collections::BTreeMap<String, ProxySettings>;

fn routes_of(http: Option<&HttpServiceDef>) -> RouteMap {
    http.map(|h| h.routes.clone()).unwrap_or_default()
}

/// Settings for a route the service declares nothing specific about, which is
/// also what the synthesised `/` route of a binding-less service takes.
// r[impl service.http.route.compression]
// r[impl service.http.route.balancing]
pub(super) fn service_level_proxy(snapshot: &AppDef, service_name: &str) -> RouteProxy {
    let (service, routes) = proxy_settings_for(snapshot, service_name);
    // These settings are used for the synthesised `/` route, which is a real
    // route the service may have declared settings for even when no pod is
    // bound to it yet. Resolving against `None` would ignore those and, worse,
    // apply a service-level limit to a `/` that opted out of it.
    let resolved = resolve(&service, routes.get("/"));
    let zone = zone_for(snapshot.name.as_str(), service_name, "/", &resolved);
    RouteProxy::from_resolved(resolved, zone)
}

/// Names the budget a resolved limit counts against.
///
/// The name is the identity of the declaration, so a limit the service
/// declared is one budget for the service however many routes inherit it,
/// while a limit a route declared is that route's own. See
/// [`RouteRateLimit::zone`].
fn zone_for(app: &str, service: &str, prefix: &str, resolved: &ResolvedRouteProxy) -> RouteZone {
    match resolved.rate_limit.map(|rl| rl.scope) {
        Some(RateLimitScope::Route) => RouteZone(format!("{app}/{service}{prefix}")),
        // No prefix: every route inheriting the service's limit names the same
        // budget, which is what makes it one budget rather than one each.
        Some(RateLimitScope::Service) | None => RouteZone(format!("{app}/{service}")),
    }
}

/// Build per-prefix HTTP routes for an ingress backed by `service_name`
/// declared in `snapshot` (the AppDef of the app that owns the service).
///
/// For each (pod, http_binding) on that app where the binding targets the
/// named service, group by the binding's URL prefix and collect the running
/// pod IPs as upstreams. Caddy then picks the right pods by longest-prefix
/// match.
///
/// The pod-level binding is the source of truth: a single deployment's
/// http_bindings can target the same service at multiple prefixes (e.g. an
/// API server bound at both `/api` and `/v1`), and multiple deployments can
/// claim different prefixes on the same service (e.g. an API on `/api` and
/// a frontend on `/`). Without this fan-out, the reconciler would route all
/// service traffic to all backing pods and the URL-prefix model would be a
/// no-op.
///
/// Used by both app ingresses (where snapshot/running_pods belong to the
/// same app as the ingress) and site ingresses (where snapshot/running_pods
/// belong to whichever app actually declares the targeted service).
// r[impl service.http.route.routing]
pub(super) fn collect_http_routes(
    snapshot: &AppDef,
    service_name: &str,
    running_pods: &[RunningPod],
) -> Vec<HttpForwardRoute> {
    use std::collections::BTreeMap;

    // Map pod resource name → list of pod IPs (one per running replica) for
    // the running pods in this app. Scaled deployments contribute multiple
    // entries under the same name, all of which become upstreams for any
    // prefix the deployment claims.
    let mut pod_ips_for_resource: BTreeMap<String, Vec<std::net::Ipv6Addr>> = BTreeMap::new();
    for p in running_pods {
        if let Some(name) = p.instance.name.as_ref() {
            pod_ips_for_resource
                .entry(name.clone())
                .or_default()
                .push(p.pod_ip);
        }
    }

    // prefix -> list of "ip:pod_port" upstream strings.
    let mut by_prefix: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for resource in snapshot.resources.values() {
        let (resource_name, http_bindings) = match resource {
            Resource::Deployment(dep) => {
                let def = dep.def.lock();
                let pod = def.pod.lock();
                (dep.name.to_string(), pod.http_bindings.clone())
            }
            Resource::Job(job) => {
                let def = job.def.lock();
                let pod = def.pod.lock();
                (job.name.to_string(), pod.http_bindings.clone())
            }
            _ => continue,
        };

        for binding in &http_bindings {
            if binding.route.http.service.name().as_str() != service_name {
                continue;
            }
            // Only resources whose pods are observed running this tick
            // contribute upstreams. A binding with no live pod yields an
            // empty upstream list for its prefix; Caddy responds with 502
            // until a pod comes up, which is the right behaviour during
            // rollouts.
            let entry = by_prefix.entry(binding.route.prefix.clone()).or_default();
            if let Some(ips) = pod_ips_for_resource.get(&resource_name) {
                for ip in ips {
                    entry.push(format!("[{}]:{}", ip, binding.pod_port.get()));
                }
            }
        }
    }

    // r[impl service.http.route.compression]
    // r[impl service.http.route.balancing]
    // Every emitted route carries settings, so a service that declared none
    // still gets the defaults rather than a bare proxy handler.
    let (service, routes) = proxy_settings_for(snapshot, service_name);
    by_prefix
        .into_iter()
        .map(|(prefix, upstreams)| {
            let resolved = resolve(&service, routes.get(&prefix));
            let zone = zone_for(snapshot.name.as_str(), service_name, &prefix, &resolved);
            let proxy = RouteProxy::from_resolved(resolved, zone);
            HttpForwardRoute {
                prefix,
                upstreams,
                proxy,
            }
        })
        .collect()
}

#[cfg(test)]
mod zone_tests {
    use super::*;
    use crate::defs::service::{RateLimitSettings, ResolvedRateLimit};

    fn resolved(scope: Option<RateLimitScope>) -> ResolvedRouteProxy {
        ResolvedRouteProxy {
            rate_limit: scope.map(|scope| ResolvedRateLimit {
                settings: RateLimitSettings {
                    max_events: 10,
                    window_secs: 1.0,
                },
                scope,
            }),
            ..Default::default()
        }
    }

    // r[verify service.http.route.rate-limiting]
    #[test]
    fn an_inherited_limit_names_one_budget_whatever_prefix_resolves_it() {
        // The failure this guards is a service declaring one limit and getting
        // one budget per route: three routes would let a client spend the
        // limit three times over, and a fourth route would raise it again.
        let service = resolved(Some(RateLimitScope::Service));
        let api = zone_for("demo", "web", "/api", &service);
        let v1 = zone_for("demo", "web", "/v1", &service);
        let root = zone_for("demo", "web", "/", &service);

        assert_eq!(api, v1);
        assert_eq!(api, root);
        assert_eq!(api, RouteZone("demo/web".to_string()));
    }

    // r[verify service.http.route.rate-limiting]
    #[test]
    fn a_route_declared_limit_names_that_routes_own_budget() {
        let route = resolved(Some(RateLimitScope::Route));
        assert_eq!(
            zone_for("demo", "web", "/api/login", &route),
            RouteZone("demo/web/api/login".to_string())
        );
        // And is distinct from a sibling's, so a tighter limit on one prefix
        // does not draw on the budget of another.
        assert_ne!(
            zone_for("demo", "web", "/api/login", &route),
            zone_for("demo", "web", "/api", &route)
        );
    }

    // r[verify service.http.route.rate-limiting]
    #[test]
    fn budgets_of_different_services_and_apps_stay_apart() {
        let route = resolved(Some(RateLimitScope::Route));
        let a = zone_for("app-one", "web", "/api", &route);
        let b = zone_for("app-two", "web", "/api", &route);
        let c = zone_for("app-one", "portal", "/api", &route);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    // The vhost serving a request is not a parameter of `zone_for` at all, so
    // a budget cannot vary by hostname or termination. That is the structural
    // form of the guarantee, which no test could state more strongly.
}
