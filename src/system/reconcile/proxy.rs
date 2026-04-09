use ipnet::Ipv6Net;

use crate::{
    defs::{
        app::AppDef,
        ingress::IngressDef,
        pod::PodDef,
        resource::{Resource, ResourceKind},
    },
    runtime::{
        InstanceRegistry, desired::DesiredState, identity::ResourceInstance,
        lifecycle::LifecycleState,
    },
    system::translate::proxy::{ServiceUpstream, instance_ipv6},
};

pub(super) struct ProxyBuildOutput {
    pub pairs: Vec<(IngressDef, ServiceUpstream)>,
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
    app_name: &str,
) -> ProxyBuildOutput {
    let mut observations: Vec<(ResourceInstance, &'static str, serde_json::Value)> = Vec::new();
    let mut ready_observations: Vec<(ResourceInstance, &'static str, serde_json::Value)> =
        Vec::new();
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
        let service_ip = instance_ipv6(node_prefix, &svc_instance);

        // Find the upstream port by scanning all pod definitions for a binding
        // that references this service. Fall back to the ingress's declared port
        // if no binding is found.
        let upstream_port = find_upstream_port(snapshot, svc_name, def.port);

        let ingress_instance = registry.get_or_create_singleton(
            app_name,
            ResourceKind::Ingress,
            Some(ingress.service.name.as_str()),
        );
        observations.push((
            ingress_instance.clone(),
            "ingress_configured",
            serde_json::json!({}),
        ));

        ready_observations.push((ingress_instance, "ingress_ready", serde_json::json!({})));

        pairs.push((
            def,
            ServiceUpstream {
                service_ip,
                service_port: upstream_port,
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
            Some(ingress.service.name.as_str()),
        );
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

    ProxyBuildOutput {
        pairs,
        observations,
        ready_observations,
    }
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
        if b.service_port.service.name.as_str() == service_name {
            return Some(b.service_port.port);
        }
    }

    for b in &pod.http_bindings {
        if b.route.http.service.name.as_str() == service_name {
            return Some(b.route.http.port);
        }
    }

    None
}
