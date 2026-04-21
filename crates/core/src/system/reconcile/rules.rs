use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

use ipnet::Ipv6Net;
use seedling_protocol::names::AppName;

use super::RunningPod;
use crate::{
    defs::{
        app::AppDef,
        resource::{Resource, ResourceKind},
    },
    runtime::{InstanceRegistry, registry::RegistryError},
    system::{
        translate::proxy::instance_ipv6,
        types::{ForwardProto, IngressRule, MountRule, ServiceDnatRule},
    },
};

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
            let svc_name = b.route.http.service.name.as_str().to_owned();
            let svc_port = b.route.http.port.get();
            backends
                .entry((svc_name, svc_port, ForwardProto::Tcp))
                .or_default()
                .push((pod.pod_ip, b.pod_port.get()));
        }

        for b in &tcp {
            let svc_name = b.service_port.service.name.as_str().to_owned();
            let svc_port = b.service_port.port.get();
            backends
                .entry((svc_name, svc_port, ForwardProto::Tcp))
                .or_default()
                .push((pod.pod_ip, b.pod_port.get()));
        }

        for b in &udp {
            let svc_name = b.service_port.service.name.as_str().to_owned();
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

    Ok(rules)
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
            let svc_name = sp.service.name.as_str();
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
