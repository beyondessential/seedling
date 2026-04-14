use std::net::Ipv6Addr;

use ipnet::Ipv6Net;

use crate::{
    defs::{
        app::AppDef,
        pod::PodDef,
        resource::{Resource, ResourceKind},
    },
    runtime::{
        InstanceRegistry, desired::DesiredState, identity::ResourceInstance,
        lifecycle::LifecycleState, registry::RegistryError,
    },
    system::{translate::proxy::instance_ipv6, types::ServiceRoute},
};

use super::RunningPod;

// r[autonomous.network]
// r[fault.non-blocking]
#[expect(
    clippy::type_complexity,
    reason = "flattening the tuple would hurt readability"
)]
pub(super) fn build(
    desired: &DesiredState,
    snapshot: &AppDef,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    running_pods: &[RunningPod],
    app_name: &str,
) -> Result<
    (
        Vec<ServiceRoute>,
        Vec<(ResourceInstance, &'static str, serde_json::Value)>,
    ),
    RegistryError,
> {
    let mut observations: Vec<(ResourceInstance, &'static str, serde_json::Value)> = Vec::new();
    let mut routes: Vec<ServiceRoute> = Vec::new();

    for (id, resource) in &snapshot.resources {
        let svc_name = match resource {
            Resource::Service(s) => s.name.as_str(),
            Resource::HttpService(h) => h.service.name.as_str(),
            // TODO: ExternalService is external to this BSL app but still within
            // seedling. When cross-app service routing is implemented, this must
            // resolve to the source service's IPv6 address in the other app
            // (instance_ipv6(node_prefix, &source_service_instance)) and install
            // a route for it. For now, no route is installed.
            Resource::ExternalService(_) => continue,
            _ => continue,
        };

        // Derive the stable /128 service IPv6 from the node prefix and the
        // service's persisted instance ID. Always uses ResourceKind::Service
        // regardless of whether the resource entry is Service or HttpService.
        let _ = id; // name used directly; kind normalised below
        let svc_instance =
            registry.get_or_create_singleton(app_name, ResourceKind::Service, Some(svc_name))?;
        let service_ip = instance_ipv6(node_prefix, &svc_instance);

        let backends: Vec<Ipv6Addr> = running_pods
            .iter()
            .filter(|pod| pod_backs_service(pod, svc_name))
            .map(|pod| pod.pod_ip)
            .collect();

        // Service exists → its IP is allocated (oracle "network_created").
        observations.push((
            svc_instance.clone(),
            "network_created",
            serde_json::json!({}),
        ));
        // Always install a route, even with no backends — the data plane
        // converts an empty backends list to a blackhole /128 so that
        // connections fail fast (RST) rather than timing out.
        routes.push(ServiceRoute {
            service_ip,
            backends: backends.clone(),
        });
        if !backends.is_empty() {
            observations.push((
                svc_instance.clone(),
                "backend_healthy",
                serde_json::json!({}),
            ));
        }
    }

    // Emit termination observations for services desired at Unscheduled.
    for dr in &desired.resources {
        match &dr.definition {
            Resource::Service(s) => {
                if dr.desired != LifecycleState::Unscheduled {
                    continue;
                }
                let svc_name = s.name.as_str();
                let svc_instance = registry.get_or_create_singleton(
                    app_name,
                    ResourceKind::Service,
                    Some(svc_name),
                )?;
                observations.push((svc_instance.clone(), "stop_sent", serde_json::json!({})));
                observations.push((
                    svc_instance.clone(),
                    "network_removed",
                    serde_json::json!({}),
                ));
                observations.push((
                    svc_instance.clone(),
                    "network_cleaned_up",
                    serde_json::json!({}),
                ));
            }
            Resource::HttpService(h) => {
                if dr.desired != LifecycleState::Unscheduled {
                    continue;
                }
                let svc_name = h.service.name.as_str();
                let svc_instance = registry.get_or_create_singleton(
                    app_name,
                    ResourceKind::Service,
                    Some(svc_name),
                )?;
                observations.push((svc_instance.clone(), "stop_sent", serde_json::json!({})));
                observations.push((
                    svc_instance.clone(),
                    "network_removed",
                    serde_json::json!({}),
                ));
                observations.push((
                    svc_instance.clone(),
                    "network_cleaned_up",
                    serde_json::json!({}),
                ));
            }
            _ => {}
        }
    }

    Ok((routes, observations))
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
