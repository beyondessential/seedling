use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use ipnet::Ipv6Net;
use tracing::error;

use crate::{
    defs::{
        app::AppDef,
        resource::{Resource, ResourceKind},
    },
    runtime::InstanceRegistry,
    system::{
        System,
        translate::proxy::instance_ipv6,
        types::{DataPlaneRules, ForwardProto, IngressRule, MountRule},
    },
};

use super::RunningPod;

// r[autonomous.ingress]
// r[autonomous.network]
// r[fault.non-blocking]
pub(super) async fn apply(
    driver: &System,
    snapshot: &AppDef,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    running_pods: &[RunningPod],
    app_name: &str,
    caddy_ip: Ipv6Addr,
) {
    let ingress = build_ingress_rules(snapshot, caddy_ip);
    let mounts = build_mount_rules(node_prefix, registry, running_pods, app_name);

    let rules = DataPlaneRules { ingress, mounts };

    if let Err(e) = driver.data_plane.apply_rules(&rules).await {
        error!(error = %e, "phase 5: apply_rules failed");
    }
}

fn build_ingress_rules(snapshot: &AppDef, caddy_ip: Ipv6Addr) -> Vec<IngressRule> {
    let mut rules = Vec::new();

    for resource in snapshot.resources.values() {
        let ingress = match resource {
            Resource::Ingress(i) => i,
            _ => continue,
        };

        let def = ingress.def.lock();

        // Caddy listens internally on the same port number as the external
        // ingress port — there is no port remapping between the host and
        // Caddy's container.
        let caddy_addr = SocketAddr::new(IpAddr::V6(caddy_ip), def.port);

        let proto = if def.dtls || def.quic {
            ForwardProto::Both
        } else {
            ForwardProto::Tcp
        };

        rules.push(IngressRule {
            external_port: def.port,
            proto,
            caddy_addr,
        });

        // If the ingress has an HTTP→HTTPS redirect configured, add a second
        // rule for the redirect source port (always TCP only).
        if let Some(redirect) = &def.redirect {
            rules.push(IngressRule {
                external_port: redirect.port,
                proto: ForwardProto::Tcp,
                caddy_addr: SocketAddr::new(IpAddr::V6(caddy_ip), redirect.port),
            });
        }
    }

    rules
}

fn build_mount_rules(
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    running_pods: &[RunningPod],
    app_name: &str,
) -> Vec<MountRule> {
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

        let mount_addr = pod_mount_addr(&pod.pod_prefix);

        for sp in &service_mounts {
            let svc_name = sp.service.name.as_str();
            let svc_instance =
                registry.get_or_create_singleton(app_name, ResourceKind::Service, Some(svc_name));
            let service_ip = instance_ipv6(node_prefix, &svc_instance);

            rules.push(MountRule {
                pod_prefix: pod.pod_prefix,
                mount_addr,
                mount_port: sp.port,
                service_ip,
                // Port translation is not yet supported; mount_port and
                // service_port are always the same value.
                service_port: sp.port,
                proto: ForwardProto::Tcp,
            });
        }
    }

    rules
}

/// Returns `pod_prefix::2` — the bridge-side mount endpoint address used as
/// the DNAT6 destination match for service-mount rules.
fn pod_mount_addr(pod_prefix: &Ipv6Net) -> Ipv6Addr {
    let mut bytes = pod_prefix.network().octets();
    bytes[8..].fill(0);
    bytes[15] = 2;
    Ipv6Addr::from(bytes)
}
