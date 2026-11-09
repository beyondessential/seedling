use super::registry::ForwardProto;
use crate::defs::{
    app::AppDef,
    pod::PodDef,
    resource::Resource,
    service::{BoundService, ServicePort},
};

// i[impl forward.request]
// i[impl forward.script-update]
/// Whether a pod in `def` attaches to `service` at service-side `port` for
/// `proto`: the only ports the service's address routes anywhere.
pub fn declares_port(def: &AppDef, service: &str, port: u16, proto: &ForwardProto) -> bool {
    def.resources.values().any(|resource| match resource {
        Resource::Deployment(dep) => {
            pod_declares_port(&dep.def.lock().pod.lock(), service, port, proto)
        }
        Resource::Job(job) => pod_declares_port(&job.def.lock().pod.lock(), service, port, proto),
        _ => false,
    })
}

fn pod_declares_port(pod: &PodDef, service: &str, port: u16, proto: &ForwardProto) -> bool {
    let is_target = |bound: &BoundService, bound_port: u16| {
        matches!(bound, BoundService::App(s) if s.name.as_str() == service) && bound_port == port
    };
    let in_bindings = |sp: &ServicePort| is_target(&sp.service, sp.port.get());
    match proto {
        ForwardProto::Tcp => {
            pod.tcp_bindings
                .iter()
                .any(|b| in_bindings(&b.service_port))
                || pod
                    .http_bindings
                    .iter()
                    .any(|b| is_target(&b.route.http.service, b.route.http.port.get()))
        }
        ForwardProto::Udp => pod
            .udp_bindings
            .iter()
            .any(|b| in_bindings(&b.service_port)),
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

    // i[verify forward.request]
    #[test]
    fn ports_are_declared_per_proto_by_pod_bindings() {
        let def = appdef_from(
            r#"
            let db = app.service("db");
            let dns = app.service("dns");
            let web = app.service("web");
            app.deployment("pg").image("docker.io/library/postgres:16").tcp(5432, db.port(5433));
            app.job("resolver").image("docker.io/library/unbound:1").udp(53, dns);
            app.deployment("site").image("docker.io/library/nginx:1").http(8080, web.http(80));
            app.deployment("client").image("docker.io/library/psql:1").mount(db.port(9999));
        "#,
        );

        assert!(declares_port(&def, "db", 5433, &ForwardProto::Tcp));
        assert!(
            !declares_port(&def, "db", 5432, &ForwardProto::Tcp),
            "the pod-side port is not the service-side one"
        );
        assert!(!declares_port(&def, "db", 5433, &ForwardProto::Udp));
        assert!(
            !declares_port(&def, "db", 9999, &ForwardProto::Tcp),
            "a mount reaches a service port; it does not serve one"
        );
        assert!(declares_port(&def, "dns", 53, &ForwardProto::Udp));
        assert!(!declares_port(&def, "dns", 53, &ForwardProto::Tcp));
        assert!(declares_port(&def, "web", 80, &ForwardProto::Tcp));
        assert!(!declares_port(&def, "web", 8080, &ForwardProto::Tcp));
        assert!(!declares_port(&def, "nope", 80, &ForwardProto::Tcp));
    }
}
