use ipnet::Ipv6Net;
use seedling_protocol::names::AppName;

use crate::{
    defs::{
        app::AppDef,
        ingress::IngressDef,
        resource::{Resource, ResourceKind},
        service::{
            HttpServiceDef, ProxySettings, REDIRECT_OWNED_HEADER, RateLimitScope, RouteDecl,
            RouteKind, resolve,
        },
    },
    runtime::{
        InstanceRegistry, desired::DesiredState, identity::ResourceInstance,
        lifecycle::LifecycleState, registry::RegistryError,
    },
    system::{
        translate::proxy::{HttpForwardRoute, HttpRedirectRoute, ServiceUpstream, instance_ipv6},
        types::{L4Proto, L4Route, RouteProxy, RouteZone},
    },
};

use super::RunningPod;

pub(super) struct ProxyBuildOutput {
    pub pairs: Vec<(IngressDef, ServiceUpstream)>,
    pub l4_routes: Vec<L4Route>,
    /// Hostnames of ingresses that terminate TLS or DTLS without terminating
    /// HTTP. They have no virtual host for the proxy to acquire a certificate
    /// through, so each is handed to it as a certificate to obtain.
    // spec: ING#certificates
    pub certificate_hostnames: Vec<String>,
    /// Observations to persist regardless of apply outcome (ingress_configured + uninstall).
    pub observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    /// Observations to persist only after successful apply (ingress_ready).
    pub ready_observations: Vec<(ResourceInstance, &'static str, serde_json::Value)>,
}

// spec: AUTO#ingress-and-network-repair
// spec: FLT#filing-and-clearing
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
    let mut certificate_hostnames: Vec<String> = Vec::new();
    let mut service_defs: std::collections::HashMap<String, ServiceProxyDef> =
        std::collections::HashMap::new();

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

        // The ingress's resource name is "<hostname>:<port>" (multiple
        // ingresses can hang off one service). The instance must be keyed
        // by the same name the AppDef uses, otherwise the status rollup
        // (`runtime::apps::status`) finds no instance for it and the app
        // sits at Degraded with the ingress wedged at Pending.
        let ingress_instance = registry.get_or_create_singleton(
            app_name,
            ResourceKind::Ingress,
            Some(ingress.name.as_str()),
        )?;

        if def.http_terminate.is_none() {
            if def.tls || def.dtls {
                certificate_hostnames.push(def.hostname.clone());
            }
            // spec: ING#declaration
            // Delivered to the Service at the ingress's own port number.
            let upstream_url = format!("[{}]:{}", service_ip, def.port.get());

            // spec: ING#termination
            // A DTLS ingress accepts DTLS over UDP and nothing on TCP; one with
            // no termination forwards TCP alone. A TLS ingress's connection is
            // terminated by the proxy, so its Service receives the plaintext
            // stream. The proxy has no DTLS termination, so a DTLS ingress's
            // datagrams reach the Service as they arrive.
            l4_routes.push(L4Route {
                port: def.port.get(),
                proto: if def.dtls { L4Proto::Udp } else { L4Proto::Tcp },
                upstreams: vec![upstream_url],
                terminate_tls: def.tls,
            });

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

        // One scan and one clone per distinct service rather than per
        // ingress: several ingresses commonly hang off one service (the
        // `:80` + `:443` pair, or one per hostname), and this runs on every
        // reconciliation tick.
        let service_def = service_defs
            .entry(svc_name.to_owned())
            .or_insert_with(|| service_proxy_def(snapshot, svc_name));

        // spec: PRX#route-emission
        // Per-prefix routes derived from pod http_bindings. Caddy will
        // longest-prefix match these; the service IP fallback below only
        // kicks in if the service has no http_bindings at all (e.g. an
        // HTTPS-fronted TCP service).
        let routes = collect_http_routes(snapshot, svc_name, running_pods, service_def);

        // spec: PRX#redirect-routes
        // Read from the service's own declaration rather than from the pods,
        // there being no pod behind a redirect to read it from.
        let redirects = collect_redirect_routes(service_def);
        let service_port = service_def.fallback_port(def.port.get());

        pairs.push((
            def,
            ServiceUpstream {
                routes,
                redirects,
                service_ip,
                service_port,
                proxy: service_level_proxy(snapshot, svc_name, service_def),
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
        certificate_hostnames,
        observations,
        ready_observations,
    })
}

/// Everything the emitters need from one service's declaration.
///
/// Read in a single pass under a single lock, and handed to each collector,
/// rather than each of them finding the service for itself: they run per
/// ingress on every reconciliation tick, and each walk was a scan of the
/// app's whole resource map to reach the same `HttpServiceDef`.
#[derive(Default)]
pub(super) struct ServiceProxyDef {
    /// The service's own settings, which its routes resolve against.
    pub service: ProxySettings,
    /// Every prefix the service declared, and what each is.
    pub routes: RouteMap,
    /// The lowest Service Port the service is viewed on as an HTTP service.
    pub lowest_http_port: Option<u16>,
}

type RouteMap = std::collections::BTreeMap<String, RouteDecl>;

impl ServiceProxyDef {
    /// The port of the fallback route's upstream: the lowest Service Port the
    /// service is viewed on as an HTTP service, or the ingress's own port
    /// where it is viewed on none.
    // spec: PRX#route-emission
    pub(super) fn fallback_port(&self, ingress_port: u16) -> u16 {
        self.lowest_http_port.unwrap_or(ingress_port)
    }

    /// What one prefix declared for itself, for resolution against the
    /// service's own values.
    fn settings(&self, prefix: &str) -> Option<&ProxySettings> {
        self.routes.get(prefix).map(|r| &r.settings)
    }
}

/// The `HttpServiceDef` an app declared for `service_name`, if any. Both the
/// app's own services and external-service slots can back an HTTP route.
pub(super) fn service_proxy_def(snapshot: &AppDef, service_name: &str) -> ServiceProxyDef {
    let gather = |settings: ProxySettings, http: Option<&HttpServiceDef>| ServiceProxyDef {
        service: settings,
        routes: http.map(|h| h.routes.clone()).unwrap_or_default(),
        lowest_http_port: http.and_then(|h| h.ports.first().copied()),
    };
    snapshot
        .resources
        .values()
        .find_map(|r| match r {
            Resource::Service(s) if s.name.as_str() == service_name => {
                let def = s.def.lock();
                Some(gather(def.proxy_settings(), def.http.as_ref()))
            }
            Resource::ExternalService(e) if e.name.as_str() == service_name => {
                let def = e.def.lock();
                Some(gather(def.proxy_settings(), def.http.as_ref()))
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// Settings for the synthesised `/` route, which a service is served through
/// when no pod binds a prefix. A service may have declared settings for `/`
/// even so, and they are what that route takes.
// spec: PRX#compression
// spec: PRX#balancing
pub(super) fn service_level_proxy(
    snapshot: &AppDef,
    service_name: &str,
    def: &ServiceProxyDef,
) -> RouteProxy {
    let service = &def.service;
    // These settings are used for the synthesised `/` route, which is a real
    // route the service may have declared settings for even when no pod is
    // bound to it yet. Resolving against `None` would ignore those and, worse,
    // apply a service-level limit to a `/` that opted out of it.
    let resolved = resolve(service, def.settings("/"));
    let scope = resolved.rate_limit.map(|rl| rl.scope);
    RouteProxy::from_resolved(resolved, || {
        zone_for(
            snapshot.name.as_str(),
            service_name,
            "/",
            scope.expect("only called where a limit is in force"),
        )
    })
}

/// Names the budget a resolved limit counts against.
///
/// The name is the identity of the declaration, so a limit the service
/// declared is one budget for the service however many routes inherit it,
/// while a limit a route declared is that route's own. See
/// [`RouteRateLimit::zone`].
fn zone_for(app: &str, service: &str, prefix: &str, scope: RateLimitScope) -> RouteZone {
    match scope {
        RateLimitScope::Route => RouteZone(format!("{app}/{service}{prefix}")),
        // No prefix: every route inheriting the service's limit names the same
        // budget, which is what makes it one budget rather than one each.
        RateLimitScope::Service => RouteZone(format!("{app}/{service}")),
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
// spec: PRX#route-emission
pub(super) fn collect_http_routes(
    snapshot: &AppDef,
    service_name: &str,
    running_pods: &[RunningPod],
    def: &ServiceProxyDef,
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

    // spec: PRX#compression
    // spec: PRX#balancing
    // Every emitted route carries settings, so a service that declared none
    // still gets the defaults rather than a bare proxy handler.
    by_prefix
        .into_iter()
        .map(|(prefix, upstreams)| {
            let resolved = resolve(&def.service, def.settings(&prefix));
            let scope = resolved.rate_limit.map(|rl| rl.scope);
            let proxy = RouteProxy::from_resolved(resolved, || {
                zone_for(
                    snapshot.name.as_str(),
                    service_name,
                    &prefix,
                    scope.expect("only called where a limit is in force"),
                )
            });
            HttpForwardRoute {
                prefix,
                upstreams,
                proxy,
            }
        })
        .collect()
}

/// The redirect routes a service declares.
///
/// Unlike [`collect_http_routes`] this reads the service rather than the pods:
/// a redirect answers for itself, so there is no binding to derive it from and
/// no running pod for it to wait on.
// spec: PRX#redirect-routes
pub(super) fn collect_redirect_routes(def: &ServiceProxyDef) -> Vec<HttpRedirectRoute> {
    def.routes
        .iter()
        .filter_map(|(prefix, decl)| match &decl.kind {
            RouteKind::Redirect(redirect) => Some((prefix, decl, redirect)),
            RouteKind::Proxied { .. } => None,
        })
        .map(|(prefix, decl, redirect)| {
            let resolved = resolve(&def.service, Some(&decl.settings));
            HttpRedirectRoute {
                prefix: prefix.clone(),
                target: redirect.target.iter().map(Into::into).collect(),
                code: redirect.code,
                // spec: PRX#header-operations
                // The response direction alone: a redirect sends no request
                // onward, and a request operation the service declared is
                // ignored on a redirect route rather than refused, so a
                // service-wide declaration need not be written around it.
                //
                // `Location` is dropped by the same rule. The route may not
                // declare an operation on it, but the service may — for its
                // proxied routes — and layering that over a redirect would
                // displace the target the redirect computed, sending every
                // request under the prefix somewhere other than what `to`
                // declares.
                headers: resolved
                    .headers
                    .response
                    .without(REDIRECT_OWNED_HEADER)
                    .into_grouped()
                    .into(),
            }
        })
        .collect()
}

#[cfg(test)]
mod zone_tests {
    use super::*;
    use crate::defs::service::{CompressDecl, LbPolicy};

    // spec: PRX#compression
    // spec: PRX#balancing
    #[test]
    fn the_fallback_route_takes_what_the_service_declared_for_slash() {
        // `service_level_proxy` resolves against the declared `/` route, not
        // against nothing, so a service that said something specific about `/`
        // is honoured on the synthesised route as well — for compression and
        // balancing, not only for the limit that motivated the change.
        let service = ProxySettings {
            compress: Some(CompressDecl::Enabled(Default::default())),
            balance: crate::defs::service::BalanceSettings {
                policy: Some(LbPolicy::LeastConn),
                ..Default::default()
            },
            ..Default::default()
        };
        let root = ProxySettings {
            compress: Some(CompressDecl::Disabled),
            balance: crate::defs::service::BalanceSettings {
                policy: Some(LbPolicy::First),
                ..Default::default()
            },
            ..Default::default()
        };

        let resolved = resolve(&service, Some(&root));
        assert!(
            resolved.compress.is_none(),
            "a `/` route that switched compression off keeps it off on the \
             fallback route"
        );
        assert_eq!(resolved.balance.policy, LbPolicy::First);

        // Against no route at all the service's own values stand, which is
        // what the fallback used to take unconditionally.
        let ignoring_root = resolve(&service, None);
        assert!(ignoring_root.compress.is_some());
        assert_eq!(ignoring_root.balance.policy, LbPolicy::LeastConn);
    }

    // spec: PRX#rate-limiting
    #[test]
    fn an_inherited_limit_names_one_budget_whatever_prefix_resolves_it() {
        // The failure this guards is a service declaring one limit and getting
        // one budget per route: three routes would let a client spend the
        // limit three times over, and a fourth route would raise it again.
        let service = RateLimitScope::Service;
        let api = zone_for("demo", "web", "/api", service);
        let v1 = zone_for("demo", "web", "/v1", service);
        let root = zone_for("demo", "web", "/", service);

        assert_eq!(api, v1);
        assert_eq!(api, root);
        assert_eq!(api, RouteZone("demo/web".to_string()));
    }

    // spec: PRX#rate-limiting
    #[test]
    fn a_route_declared_limit_names_that_routes_own_budget() {
        let route = RateLimitScope::Route;
        assert_eq!(
            zone_for("demo", "web", "/api/login", route),
            RouteZone("demo/web/api/login".to_string())
        );
        // And is distinct from a sibling's, so a tighter limit on one prefix
        // does not draw on the budget of another.
        assert_ne!(
            zone_for("demo", "web", "/api/login", route),
            zone_for("demo", "web", "/api", route)
        );
    }

    // spec: PRX#rate-limiting
    #[test]
    fn budgets_of_different_services_and_apps_stay_apart() {
        let route = RateLimitScope::Route;
        let a = zone_for("app-one", "web", "/api", route);
        let b = zone_for("app-two", "web", "/api", route);
        let c = zone_for("app-one", "portal", "/api", route);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    // The vhost serving a request is not a parameter of `zone_for` at all, so
    // a budget cannot vary by hostname or termination. That is the structural
    // form of the guarantee, which no test could state more strongly.
}

#[cfg(test)]
mod certificate_tests {

    use super::*;
    use crate::{runtime::EphemeralInstanceRegistry, tests::run_test_script_app};

    // spec: ING#certificates
    #[test]
    fn ingresses_terminating_tls_or_dtls_without_http_hand_their_hostnames_to_the_proxy() {
        let app = run_test_script_app(
            r#"
            let web = app.service("web");
            web.ingress("db.example.com", 5432).tls(Terminate.Tls, Output.Tcp);
            web.ingress("voip.example.com", 5684).tls(Terminate.Dtls, Output.Udp);
            web.ingress("site.example.com", 443).tls(Terminate.Https, Output.Http1);
        "#,
        );
        let out = collect(
            &app.def.load(),
            &DesiredState::default(),
            &"fd5e:ed12:3456::/48".parse().unwrap(),
            &EphemeralInstanceRegistry::new(),
            &AppName::new_unchecked("demo"),
            &[],
        )
        .expect("collects");
        let mut hostnames = out.certificate_hostnames;
        hostnames.sort();
        assert_eq!(hostnames, ["db.example.com", "voip.example.com"]);
        assert_eq!(out.pairs.len(), 1, "HTTPS is routed through a vhost");
    }

    // spec: ING#termination
    #[test]
    fn a_tls_ingress_is_terminated_and_a_dtls_ingress_listens_on_udp_alone() {
        let app = run_test_script_app(
            r#"
            let web = app.service("web");
            web.ingress("db.example.com", 5432).tls(Terminate.Tls, Output.Tcp);
            web.ingress("voip.example.com", 5684).tls(Terminate.Dtls, Output.Udp);
            web.ingress("raw.example.com", 2222);
        "#,
        );
        let out = collect(
            &app.def.load(),
            &DesiredState::default(),
            &"fd5e:ed12:3456::/48".parse().unwrap(),
            &EphemeralInstanceRegistry::new(),
            &AppName::new_unchecked("demo"),
            &[],
        )
        .expect("collects");
        let mut routes: Vec<(u16, L4Proto, bool)> = out
            .l4_routes
            .iter()
            .map(|r| (r.port, r.proto, r.terminate_tls))
            .collect();
        routes.sort_by_key(|(port, _, _)| *port);
        assert_eq!(
            routes,
            [
                (2222, L4Proto::Tcp, false),
                (5432, L4Proto::Tcp, true),
                (5684, L4Proto::Udp, false),
            ]
        );
    }
}

#[cfg(test)]
mod upstream_port_tests {
    use super::*;
    use crate::{runtime::EphemeralInstanceRegistry, tests::run_test_script_app};

    fn collect_for(script: &str) -> ProxyBuildOutput {
        let app = run_test_script_app(script);
        collect(
            &app.def.load(),
            &DesiredState::default(),
            &"fd5e:ed12:3456::/48".parse().unwrap(),
            &EphemeralInstanceRegistry::new(),
            &AppName::new_unchecked("demo"),
            &[],
        )
        .expect("collects")
    }

    // spec: PRX#route-emission
    #[test]
    fn the_fallback_route_reaches_the_lowest_port_the_service_is_viewed_on_over_http() {
        let out = collect_for(
            r#"
            let web = app.service("web");
            web.http(9090);
            web.http(8080).ingress("site.example.com", 443).tls(Terminate.Https, Output.Http1);
            app.deployment("app").tcp(5432, web.port(7000));
        "#,
        );
        let [(_, upstream)] = out.pairs.as_slice() else {
            panic!("one HTTPS ingress, got {}", out.pairs.len());
        };
        assert!(upstream.routes.is_empty(), "no pod binds a prefix");
        assert_eq!(upstream.service_port, 8080);
    }

    // spec: PRX#route-emission
    #[test]
    fn a_service_viewed_on_no_http_port_falls_back_to_the_ingress_port() {
        let out = collect_for(
            r#"
            let web = app.service("web");
            web.ingress("site.example.com", 8443).tls(Terminate.Https, Output.Http1);
            app.deployment("app").tcp(5432, web.port(7000));
        "#,
        );
        let [(_, upstream)] = out.pairs.as_slice() else {
            panic!("one HTTPS ingress, got {}", out.pairs.len());
        };
        assert_eq!(upstream.service_port, 8443);
    }

    // spec: ING#declaration
    #[test]
    fn a_tls_dtls_or_unterminated_ingress_reaches_the_service_at_its_own_port() {
        let out = collect_for(
            r#"
            let web = app.service("web");
            web.http(80);
            web.ingress("db.example.com", 5432).tls(Terminate.Tls, Output.Tcp);
            web.ingress("voip.example.com", 5684).tls(Terminate.Dtls, Output.Udp);
            web.ingress("raw.example.com", 2222);
            app.deployment("app").tcp(9999, web.port(7000));
        "#,
        );
        let mut routes: Vec<(u16, String)> = out
            .l4_routes
            .iter()
            .map(|r| (r.port, r.upstreams.join(",")))
            .collect();
        routes.sort();
        for (port, upstream) in routes {
            assert!(
                upstream.ends_with(&format!("]:{port}")),
                "the ingress on {port} reaches {upstream}"
            );
        }
    }
}

#[cfg(test)]
mod route_emission_tests {
    use super::*;
    use crate::tests::run_test_script_app;

    fn running(app: &AppDef, name: &str, ip: &str) -> RunningPod {
        let resource = app
            .resources
            .iter()
            .find(|(id, _)| id.name.as_str() == name)
            .map(|(_, r)| r.clone())
            .unwrap();
        RunningPod {
            instance: ResourceInstance::new_singleton(
                AppName::new_unchecked("demo"),
                ResourceKind::Deployment,
                name,
            ),
            pod_prefix: "fd00:1:2:3::/64".parse().unwrap(),
            pod_ip: ip.parse().unwrap(),
            resource,
            observed_healthy: true,
        }
    }

    // spec: PRX#route-emission
    #[test]
    fn two_pods_binding_one_prefix_form_one_route_with_both_upstreams() {
        let app = run_test_script_app(
            r#"
            let svc = app.service("public").http(80);
            app.deployment("blue").http(3000, svc.route("/api"));
            app.deployment("green").http(4000, svc.route("/api"));
            app.deployment("idle").http(5000, svc.route("/idle"));
        "#,
        );
        let def = app.def.load_full();
        let pods = [
            running(&def, "blue", "fd00:1:2:3::2"),
            running(&def, "green", "fd00:1:2:4::2"),
        ];
        let service_def = service_proxy_def(&def, "public");

        let routes = collect_http_routes(&def, "public", &pods, &service_def);

        assert_eq!(routes.len(), 2);
        let api = routes.iter().find(|r| r.prefix == "/api").unwrap();
        let mut upstreams = api.upstreams.clone();
        upstreams.sort();
        assert_eq!(
            upstreams,
            vec!["[fd00:1:2:3::2]:3000", "[fd00:1:2:4::2]:4000"]
        );
        let idle = routes.iter().find(|r| r.prefix == "/idle").unwrap();
        assert!(
            idle.upstreams.is_empty(),
            "a prefix with no running pod is still emitted"
        );
    }
}
