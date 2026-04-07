use std::net::Ipv6Addr;

use ipnet::Ipv6Net;
use tracing::error;

use crate::{
    defs::{
        app::AppDef,
        pod::PodDef,
        resource::{Resource, ResourceKind},
    },
    runtime::InstanceRegistry,
    system::{System, translate::proxy::instance_ipv6, types::ServiceRoute},
};

use super::RunningPod;

// r[autonomous.network]
// r[fault.non-blocking]
pub(super) async fn apply(
    driver: &System,
    snapshot: &AppDef,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    running_pods: &[RunningPod],
    app_name: &str,
) {
    let mut routes: Vec<ServiceRoute> = Vec::new();

    for (id, resource) in &snapshot.resources {
        let svc_name = match resource {
            Resource::Service(s) => s.name.as_str(),
            Resource::HttpService(h) => h.service.name.as_str(),
            // ExternalService is managed outside seedling; skip.
            Resource::ExternalService(_) => continue,
            _ => continue,
        };

        // Derive the stable /128 service IPv6 from the node prefix and the
        // service's persisted instance ID. Always uses ResourceKind::Service
        // regardless of whether the resource entry is Service or HttpService.
        let _ = id; // name used directly; kind normalised below
        let svc_instance =
            registry.get_or_create_singleton(app_name, ResourceKind::Service, Some(svc_name));
        let service_ip = instance_ipv6(node_prefix, &svc_instance);

        let backends: Vec<Ipv6Addr> = running_pods
            .iter()
            .filter(|pod| pod_backs_service(pod, svc_name))
            .map(|pod| pod.pod_ip)
            .collect();

        if !backends.is_empty() {
            routes.push(ServiceRoute {
                service_ip,
                backends,
            });
        }
    }

    if let Err(e) = driver.data_plane.apply_routes(&routes).await {
        error!(error = %e, "routes: apply_routes failed");
    }
}

/// Returns `true` if the pod's definition contains any TCP, UDP, or HTTP
/// binding that references `service_name`.
fn pod_backs_service(pod: &RunningPod, service_name: &str) -> bool {
    match &pod.resource {
        Resource::Deployment(dep) => {
            let def = dep.def.lock();
            let pod_def = def.pod.lock();
            pod_def_backs_service(&pod_def, service_name)
        }
        Resource::Job(job) => {
            let def = job.def.lock();
            let pod_def = def.pod.lock();
            pod_def_backs_service(&pod_def, service_name)
        }
        _ => false,
    }
}

fn pod_def_backs_service(pod: &PodDef, service_name: &str) -> bool {
    pod.tcp_bindings
        .iter()
        .any(|b| b.service_port.service.name.as_str() == service_name)
        || pod
            .udp_bindings
            .iter()
            .any(|b| b.service_port.service.name.as_str() == service_name)
        || pod
            .http_bindings
            .iter()
            .any(|b| b.route.http.service.name.as_str() == service_name)
}
