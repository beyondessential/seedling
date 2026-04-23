use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

use ipnet::Ipv6Net;
use seedling_protocol::names::{AppName, ExternalServiceName, ServiceRef};

use super::RunningPod;
use crate::{
    defs::{
        app::AppDef,
        resource::{Resource, ResourceKind},
    },
    runtime::{
        InstanceRegistry, external_service_mappings::ExternalServiceSnapshot,
        registry::RegistryError, site_services::SiteServiceProtocol,
    },
    system::{
        translate::proxy::instance_ipv6,
        types::{ForwardProto, IngressRule, MountRule, ServiceDnatRule},
    },
};

/// Per-app map from `(service_name, service_port, proto)` to `Vec<(pod_ip, pod_port)>`.
pub(super) type AppBackendMap = HashMap<(String, u16, ForwardProto), Vec<(Ipv6Addr, u16)>>;

/// Pre-compute the per-app backend map for every app in the tick so that
/// external-service bindings whose mapping target is a different app can look
/// up that app's backends.
pub(super) fn collect_backends_by_app(
    running_pods_by_app: &HashMap<AppName, Vec<RunningPod>>,
) -> HashMap<AppName, AppBackendMap> {
    running_pods_by_app
        .iter()
        .map(|(app, pods)| (app.clone(), collect_service_backends(pods)))
        .collect()
}

/// Collects backends from running pods' bindings.
/// Returns a map from `(service_name, service_port, proto)` to `Vec<(pod_ip, pod_port)>`.
fn collect_service_backends(
    running_pods: &[RunningPod],
) -> HashMap<(String, u16, ForwardProto), Vec<(Ipv6Addr, u16)>> {
    let mut backends: HashMap<(String, u16, ForwardProto), Vec<(Ipv6Addr, u16)>> = HashMap::new();

    for pod in running_pods {
        let (http, tcp, udp) = match &pod.resource {
            Resource::Deployment(dep) => {
                let def = dep.def.lock();
                let pod_def = def.pod.lock();
                (
                    pod_def.http_bindings.clone(),
                    pod_def.tcp_bindings.clone(),
                    pod_def.udp_bindings.clone(),
                )
            }
            Resource::Job(job) => {
                let def = job.def.lock();
                let pod_def = def.pod.lock();
                (
                    pod_def.http_bindings.clone(),
                    pod_def.tcp_bindings.clone(),
                    pod_def.udp_bindings.clone(),
                )
            }
            _ => continue,
        };

        for b in &http {
            let svc_name = b.route.http.service.name().as_str().to_owned();
            let svc_port = b.route.http.port.get();
            backends
                .entry((svc_name, svc_port, ForwardProto::Tcp))
                .or_default()
                .push((pod.pod_ip, b.pod_port.get()));
        }

        for b in &tcp {
            let svc_name = b.service_port.service.name().as_str().to_owned();
            let svc_port = b.service_port.port.get();
            backends
                .entry((svc_name, svc_port, ForwardProto::Tcp))
                .or_default()
                .push((pod.pod_ip, b.pod_port.get()));
        }

        for b in &udp {
            let svc_name = b.service_port.service.name().as_str().to_owned();
            let svc_port = b.service_port.port.get();
            backends
                .entry((svc_name, svc_port, ForwardProto::Udp))
                .or_default()
                .push((pod.pod_ip, b.pod_port.get()));
        }
    }

    backends
}

// r[autonomous.ingress]
pub(super) fn build_ingress_rules(
    snapshot: &AppDef,
    caddy_ip: Ipv6Addr,
    caddy_v4: Option<Ipv4Addr>,
) -> Vec<IngressRule> {
    let mut rules = Vec::new();

    for resource in snapshot.resources.values() {
        let ingress = match resource {
            Resource::Ingress(i) => i,
            _ => continue,
        };

        let def = ingress.def.lock();

        let caddy_v6 = SocketAddr::new(IpAddr::V6(caddy_ip), def.port.get());
        let caddy_v4_addr = caddy_v4.map(|ip| SocketAddr::new(IpAddr::V4(ip), def.port.get()));

        let proto = if def.dtls || def.http_terminate.is_some() {
            ForwardProto::Both
        } else {
            ForwardProto::Tcp
        };

        rules.push(IngressRule {
            external_port: def.port.get(),
            proto,
            caddy_v6,
            caddy_v4: caddy_v4_addr,
        });

        if let Some(redirect) = &def.redirect {
            rules.push(IngressRule {
                external_port: redirect.port.get(),
                proto: ForwardProto::Tcp,
                caddy_v6: SocketAddr::new(IpAddr::V6(caddy_ip), redirect.port.get()),
                caddy_v4: caddy_v4.map(|ip| SocketAddr::new(IpAddr::V4(ip), redirect.port.get())),
            });
        }
    }

    rules
}

// r[impl infra.dataplane.service-dnat]
pub(super) fn build_service_dnat_rules(
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    running_pods: &[RunningPod],
    app_name: &AppName,
    snapshot: &ExternalServiceSnapshot,
    backends_by_app: &HashMap<AppName, AppBackendMap>,
) -> Result<Vec<ServiceDnatRule>, RegistryError> {
    let backends = collect_service_backends(running_pods);
    let mut rules = Vec::new();

    for ((svc_name, svc_port, proto), backend_list) in &backends {
        let svc_instance =
            registry.get_or_create_singleton(app_name, ResourceKind::Service, Some(svc_name))?;
        let service_ip = instance_ipv6(node_prefix, &svc_instance);

        rules.push(ServiceDnatRule {
            service_ip,
            service_port: *svc_port,
            backends: backend_list.clone(),
            proto: *proto,
        });
    }

    // r[impl service.external.mapping.events] external-service bindings are
    // resolved through the operator-configured mapping table: the slot gets
    // its own stable service IP, backends come from either the target app's
    // service or from the site service's endpoints.
    for (ext_name, svc_port, proto) in collect_external_bindings(running_pods) {
        let Some(target) = snapshot.mappings.get(&(app_name.clone(), ext_name.clone())) else {
            continue;
        };
        let resolved_backends =
            resolve_external_backends(target, svc_port, proto, snapshot, backends_by_app);
        let svc_instance = registry.get_or_create_singleton(
            app_name,
            ResourceKind::ExternalService,
            Some(ext_name.as_str()),
        )?;
        let service_ip = instance_ipv6(node_prefix, &svc_instance);
        rules.push(ServiceDnatRule {
            service_ip,
            service_port: svc_port,
            backends: resolved_backends,
            proto,
        });
    }

    Ok(rules)
}

/// Walk a running app's pods and collect its external-service bindings as
/// `(slot name, service port, transport protocol)` triples, de-duplicated so
/// we emit at most one DNAT rule per slot+port+proto.
fn collect_external_bindings(
    running_pods: &[RunningPod],
) -> std::collections::HashSet<(ExternalServiceName, u16, ForwardProto)> {
    let mut out = std::collections::HashSet::new();

    for pod in running_pods {
        let (http, tcp, udp) = match &pod.resource {
            Resource::Deployment(dep) => {
                let def = dep.def.lock();
                let pod_def = def.pod.lock();
                (
                    pod_def.http_bindings.clone(),
                    pod_def.tcp_bindings.clone(),
                    pod_def.udp_bindings.clone(),
                )
            }
            Resource::Job(job) => {
                let def = job.def.lock();
                let pod_def = def.pod.lock();
                (
                    pod_def.http_bindings.clone(),
                    pod_def.tcp_bindings.clone(),
                    pod_def.udp_bindings.clone(),
                )
            }
            _ => continue,
        };

        for b in &http {
            if b.route.http.service.is_external() {
                out.insert((
                    ExternalServiceName::new_unchecked(
                        b.route.http.service.name().as_str().to_owned(),
                    ),
                    b.route.http.port.get(),
                    ForwardProto::Tcp,
                ));
            }
        }
        for b in &tcp {
            if b.service_port.service.is_external() {
                out.insert((
                    ExternalServiceName::new_unchecked(
                        b.service_port.service.name().as_str().to_owned(),
                    ),
                    b.service_port.port.get(),
                    ForwardProto::Tcp,
                ));
            }
        }
        for b in &udp {
            if b.service_port.service.is_external() {
                out.insert((
                    ExternalServiceName::new_unchecked(
                        b.service_port.service.name().as_str().to_owned(),
                    ),
                    b.service_port.port.get(),
                    ForwardProto::Udp,
                ));
            }
        }
    }

    out
}

/// Turn a mapping target into the `(backend_ip, backend_port)` list that DNAT
/// should round-robin over. Empty when the target is offline or no endpoint
/// matches the requested `(service_port, protocol)`; the DNAT rule is still
/// installed (the empty backend list blackholes traffic, surfacing the
/// misconfiguration as a connection failure rather than a silent drop).
pub(super) fn resolve_external_backends(
    target: &ServiceRef,
    svc_port: u16,
    proto: ForwardProto,
    snapshot: &ExternalServiceSnapshot,
    backends_by_app: &HashMap<AppName, AppBackendMap>,
) -> Vec<(Ipv6Addr, u16)> {
    match target {
        ServiceRef::App {
            app: target_app,
            service: target_svc,
        } => backends_by_app
            .get(target_app)
            .and_then(|m| m.get(&(target_svc.as_str().to_owned(), svc_port, proto)))
            .cloned()
            .unwrap_or_default(),
        ServiceRef::Site { name: site_name } => snapshot
            .site_endpoints
            .get(site_name)
            .map(|eps| {
                eps.iter()
                    .filter(|e| e.service_port == svc_port && protocols_match(e.protocol, proto))
                    .filter_map(|e| {
                        e.remote_host
                            .parse::<Ipv6Addr>()
                            .ok()
                            .map(|ip| (ip, e.remote_port))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Site endpoints declare their own protocol at the application layer (tcp /
/// udp / http). At the nftables layer we only care whether it's TCP- or
/// UDP-based, so http endpoints match TCP bindings.
fn protocols_match(site_proto: SiteServiceProtocol, fwd_proto: ForwardProto) -> bool {
    matches!(
        (site_proto, fwd_proto),
        (
            SiteServiceProtocol::Tcp | SiteServiceProtocol::Http,
            ForwardProto::Tcp
        ) | (SiteServiceProtocol::Udp, ForwardProto::Udp)
            | (_, ForwardProto::Both)
    )
}

// r[impl infra.dataplane.mount-dnat]
pub(super) fn build_mount_rules(running_pods: &[RunningPod]) -> Vec<MountRule> {
    let backend_map = collect_service_backends(running_pods);
    let mut rules = Vec::new();

    for pod in running_pods {
        let service_mounts = match &pod.resource {
            Resource::Deployment(dep) => {
                let def = dep.def.lock();
                let pod_def = def.pod.lock();
                pod_def.service_mounts.clone()
            }
            Resource::Job(job) => {
                let def = job.def.lock();
                let pod_def = def.pod.lock();
                pod_def.service_mounts.clone()
            }
            _ => continue,
        };

        if service_mounts.is_empty() {
            continue;
        }

        let mount_addr = crate::system::translate::proxy::node_mount_addr(&pod.pod_prefix);

        for sp in &service_mounts {
            let svc_name = sp.service.name().as_str();
            // Emit a mount rule for each protocol that has backends for this
            // (service, port) pair.  Most mounts are TCP but UDP is valid too.
            for proto in [ForwardProto::Tcp, ForwardProto::Udp] {
                let key = (svc_name.to_owned(), sp.port.get(), proto);
                let svc_backends = backend_map.get(&key).cloned().unwrap_or_default();
                if svc_backends.is_empty() {
                    continue;
                }
                rules.push(MountRule {
                    pod_prefix: pod.pod_prefix,
                    mount_addr,
                    mount_port: sp.port.get(),
                    backends: svc_backends,
                    proto,
                });
            }
        }
    }

    rules
}

#[cfg(test)]
mod tests {
    use seedling_protocol::names::{AppName, AppServiceName, SiteServiceName};

    use super::*;
    use crate::runtime::site_services::SiteServiceEndpoint;

    fn ipv6(s: &str) -> Ipv6Addr {
        s.parse().unwrap()
    }

    fn snapshot_with_site(
        site_name: &str,
        endpoints: Vec<SiteServiceEndpoint>,
    ) -> ExternalServiceSnapshot {
        let mut s = ExternalServiceSnapshot::default();
        s.site_endpoints
            .insert(SiteServiceName::new(site_name).unwrap(), endpoints);
        s
    }

    #[test]
    fn resolve_site_target_picks_endpoints_matching_port_and_protocol() {
        // Site has three endpoints across two ports; the binding is to
        // service_port 3000 / TCP, so only the two matching endpoints
        // should turn into DNAT backends.
        let site = SiteServiceName::new("postgres-prod").unwrap();
        let endpoints = vec![
            SiteServiceEndpoint {
                service_port: 3000,
                protocol: SiteServiceProtocol::Tcp,
                remote_host: "fd5e::1".into(),
                remote_port: 8080,
            },
            SiteServiceEndpoint {
                service_port: 3000,
                protocol: SiteServiceProtocol::Tcp,
                remote_host: "fd5e::2".into(),
                remote_port: 8080,
            },
            SiteServiceEndpoint {
                service_port: 4000,
                protocol: SiteServiceProtocol::Tcp,
                remote_host: "fd5e::1".into(),
                remote_port: 4000,
            },
        ];
        let snap = snapshot_with_site(site.as_str(), endpoints);

        let backends = resolve_external_backends(
            &ServiceRef::Site { name: site },
            3000,
            ForwardProto::Tcp,
            &snap,
            &HashMap::new(),
        );
        let mut sorted = backends;
        sorted.sort();
        assert_eq!(
            sorted,
            vec![(ipv6("fd5e::1"), 8080), (ipv6("fd5e::2"), 8080)]
        );
    }

    #[test]
    fn resolve_site_target_drops_non_ipv6_remote_hosts() {
        // Defence in depth: if a DNS name or IPv4 literal somehow lands in
        // the DB, the resolver silently skips it (the DNAT rule then has
        // zero backends and blackholes, surfacing the misconfiguration).
        let site = SiteServiceName::new("legacy-mix").unwrap();
        let endpoints = vec![
            SiteServiceEndpoint {
                service_port: 80,
                protocol: SiteServiceProtocol::Http,
                remote_host: "example.com".into(),
                remote_port: 80,
            },
            SiteServiceEndpoint {
                service_port: 80,
                protocol: SiteServiceProtocol::Http,
                remote_host: "fd5e::42".into(),
                remote_port: 80,
            },
        ];
        let snap = snapshot_with_site(site.as_str(), endpoints);

        let backends = resolve_external_backends(
            &ServiceRef::Site { name: site },
            80,
            ForwardProto::Tcp,
            &snap,
            &HashMap::new(),
        );
        assert_eq!(backends, vec![(ipv6("fd5e::42"), 80)]);
    }

    #[test]
    fn resolve_app_target_looks_up_backend_map() {
        // App target resolution: the external slot points at another app's
        // native service; the resolver reaches into the pre-computed
        // per-app backend map.
        let mut backends = HashMap::new();
        let mut inner = AppBackendMap::new();
        inner.insert(
            ("api".into(), 8080, ForwardProto::Tcp),
            vec![(ipv6("fd5e::aa"), 9000), (ipv6("fd5e::bb"), 9000)],
        );
        backends.insert(AppName::new("other-app").unwrap(), inner);

        let snap = ExternalServiceSnapshot::default();
        let resolved = resolve_external_backends(
            &ServiceRef::App {
                app: AppName::new("other-app").unwrap(),
                service: AppServiceName::new("api").unwrap(),
            },
            8080,
            ForwardProto::Tcp,
            &snap,
            &backends,
        );
        assert_eq!(
            resolved,
            vec![(ipv6("fd5e::aa"), 9000), (ipv6("fd5e::bb"), 9000)]
        );
    }

    #[test]
    fn resolve_returns_empty_when_target_missing() {
        let snap = ExternalServiceSnapshot::default();
        let empty = resolve_external_backends(
            &ServiceRef::Site {
                name: SiteServiceName::new("ghost").unwrap(),
            },
            1234,
            ForwardProto::Tcp,
            &snap,
            &HashMap::new(),
        );
        assert!(empty.is_empty());
    }

    #[test]
    fn protocol_matching() {
        assert!(protocols_match(SiteServiceProtocol::Tcp, ForwardProto::Tcp));
        assert!(protocols_match(
            SiteServiceProtocol::Http,
            ForwardProto::Tcp
        ));
        assert!(protocols_match(SiteServiceProtocol::Udp, ForwardProto::Udp));
        assert!(!protocols_match(
            SiteServiceProtocol::Tcp,
            ForwardProto::Udp
        ));
        assert!(!protocols_match(
            SiteServiceProtocol::Udp,
            ForwardProto::Tcp
        ));
        assert!(protocols_match(
            SiteServiceProtocol::Tcp,
            ForwardProto::Both
        ));
    }
}
