use std::net::{Ipv6Addr, SocketAddr};

use ipnet::Ipv6Net;
use tracing::error;

use crate::{
    defs::{
        app::AppDef,
        ingress::IngressDef,
        pod::PodDef,
        resource::{Resource, ResourceKind},
    },
    runtime::InstanceRegistry,
    system::{
        System,
        translate::proxy::{ServiceUpstream, build_proxy_config, instance_ipv6},
    },
};

// r[autonomous.ingress]
// r[fault.non-blocking]
pub(super) async fn apply(
    driver: &System,
    snapshot: &AppDef,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    app_name: &str,
    caddy_addr: SocketAddr,
) {
    let mut pairs: Vec<(IngressDef, ServiceUpstream)> = Vec::new();

    for resource in snapshot.resources.values() {
        let ingress = match resource {
            Resource::Ingress(i) => i,
            _ => continue,
        };

        let def = ingress.def.lock().clone();
        let svc_name = ingress.service.name.as_str();

        let svc_instance =
            registry.get_or_create_singleton(app_name, ResourceKind::Service, Some(svc_name));
        let service_ip: Ipv6Addr = instance_ipv6(node_prefix, &svc_instance);

        // Find the upstream port by scanning all pod definitions for a binding
        // that references this service. Fall back to the ingress's declared port
        // if no binding is found.
        let upstream_port = find_upstream_port(snapshot, svc_name, def.port);

        pairs.push((
            def,
            ServiceUpstream {
                service_ip,
                service_port: upstream_port,
            },
        ));
    }

    if pairs.is_empty() {
        return;
    }

    let config = build_proxy_config(&pairs, caddy_addr);

    if let Err(e) = driver.proxy.apply_config(&config).await {
        error!(error = %e, "phase 6: apply_config failed");
    }
}

/// Scan all Deployment and Job pod definitions for the first TCP or HTTP
/// binding that references `service_name`, and return the pod port.
///
/// Falls back to `fallback_port` (the ingress's declared port) when no
/// binding is found.
///
/// # Port translation
/// TODO: port translation between pod_port and service_port is not yet
/// supported. Until it is, `pod_port` and `service_port.port` are asserted
/// to be equal so that misconfigurations are caught immediately rather than
/// silently routing traffic to the wrong port.
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
        if b.service_port.service.name.as_str() == service_name {
            // TODO: port translation is not yet supported; the pod port and
            // the service port must be identical until translation is
            // implemented.
            debug_assert_eq!(
                b.pod_port, b.service_port.port,
                "port translation not supported: pod_port {} != service_port {}",
                b.pod_port, b.service_port.port,
            );
            return Some(b.pod_port);
        }
    }

    for b in &pod.http_bindings {
        if b.route.http.service.name.as_str() == service_name {
            // TODO: port translation is not yet supported.
            debug_assert_eq!(
                b.pod_port, b.route.http.port,
                "port translation not supported: pod_port {} != http_service port {}",
                b.pod_port, b.route.http.port,
            );
            return Some(b.pod_port);
        }
    }

    None
}
