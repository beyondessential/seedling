use std::collections::HashSet;

use super::registry::ForwardProto;
use crate::defs::{
    app::AppDef,
    pod::PodDef,
    resource::Resource,
    service::{BoundService, ServicePort},
};

/// The service-side ports an app's pods serve, as `(service, port, proto)`.
pub type DeclaredPorts = HashSet<(String, u16, ForwardProto)>;

// spec: FWD#requesting-a-forward
// spec: FWD#forward-lifetime
/// Collect the service-side ports an app's pods attach to: the only ports a
/// service's address routes anywhere.
///
/// Collected in one pass so a caller checking many forwards walks the
/// definition once, rather than once per forward while holding a lock the
/// datagram path needs.
pub fn declared_ports(def: &AppDef) -> DeclaredPorts {
    let mut declared = DeclaredPorts::new();
    for resource in def.resources.values() {
        match resource {
            Resource::Deployment(dep) => {
                collect_pod_ports(&dep.def.lock().pod.lock(), &mut declared)
            }
            Resource::Job(job) => collect_pod_ports(&job.def.lock().pod.lock(), &mut declared),
            _ => {}
        }
    }
    declared
}

/// Whether a pod in `def` attaches to `service` at service-side `port` for
/// `proto`.
pub fn declares_port(def: &AppDef, service: &str, port: u16, proto: &ForwardProto) -> bool {
    declared_ports(def).contains(&(service.to_owned(), port, proto.clone()))
}

fn collect_pod_ports(pod: &PodDef, declared: &mut DeclaredPorts) {
    let mut declare = |bound: &BoundService, port: u16, proto: ForwardProto| {
        if let BoundService::App(service) = bound {
            declared.insert((service.name.as_str().to_owned(), port, proto));
        }
    };
    let mut declare_bindings = |bindings: &[crate::defs::pod::TcpUdpBinding],
                                proto: ForwardProto| {
        for binding in bindings {
            let ServicePort { service, port } = &binding.service_port;
            declare(service, port.get(), proto.clone());
        }
    };
    declare_bindings(&pod.tcp_bindings, ForwardProto::Tcp);
    declare_bindings(&pod.udp_bindings, ForwardProto::Udp);
    for binding in &pod.http_bindings {
        declare(
            &binding.route.http.service,
            binding.route.http.port.get(),
            ForwardProto::Tcp,
        );
    }
}

#[cfg(test)]
mod tests {
    use seedling_protocol::names::AppName;

    use super::*;

    fn appdef_from(script: &str) -> std::sync::Arc<AppDef> {
        let name = AppName::new("testapp").unwrap();
        let (app, err) = crate::runtime::apps::evaluate_script(
            &name,
            script,
            &std::collections::BTreeMap::new(),
            &crate::ScriptLimits::default(),
        );
        assert!(err.is_none(), "script error: {err:?}");
        app.def.load_full()
    }

    // spec: FWD#requesting-a-forward
    #[test]
    fn ports_are_declared_per_proto_by_pod_bindings() {
        let def = appdef_from(
            r#"
            let db = app.service("database");
            let dns = app.service("dns");
            let web = app.service("web");
            app.deployment("postgres").image("docker.io/library/postgres:16").tcp(5432, db.port(5433));
            app.job("resolver").image("docker.io/library/unbound:1").udp(53, dns);
            app.deployment("site").image("docker.io/library/nginx:1").http(8080, web.http(80));
            app.deployment("client").image("docker.io/library/psql:1").mount(db.port(9999));
        "#,
        );

        assert!(declares_port(&def, "database", 5433, &ForwardProto::Tcp));
        assert!(
            !declares_port(&def, "database", 5432, &ForwardProto::Tcp),
            "the pod-side port is not the service-side one"
        );
        assert!(!declares_port(&def, "database", 5433, &ForwardProto::Udp));
        assert!(
            !declares_port(&def, "database", 9999, &ForwardProto::Tcp),
            "a mount reaches a service port; it does not serve one"
        );
        assert!(declares_port(&def, "dns", 53, &ForwardProto::Udp));
        assert!(!declares_port(&def, "dns", 53, &ForwardProto::Tcp));
        assert!(declares_port(&def, "web", 80, &ForwardProto::Tcp));
        assert!(!declares_port(&def, "web", 8080, &ForwardProto::Tcp));
        assert!(!declares_port(&def, "nope", 80, &ForwardProto::Tcp));
    }
}
