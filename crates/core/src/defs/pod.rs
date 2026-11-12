use rhai::{EvalAltResult, TypeBuilder};

use super::{
    Freezable, Holder, Port,
    container::ContainerDef,
    resource::ResourceId,
    service::{BoundService, HttpService, HttpServiceRoute, Service, ServicePort},
};

/// Refuse to bind a pod to a service the operator maps to its endpoint.
// spec: POD#port-bindings
fn refuse_external(service: &BoundService) -> Result<(), Box<EvalAltResult>> {
    if service.is_external() {
        return Err(format!(
            "cannot bind a pod to external service '{}': an external service's only target is \
             the endpoint an operator maps to it",
            service.name()
        )
        .into());
    }
    Ok(())
}

// spec: POD#pod-interface
#[derive(Debug, Default, Clone)]
pub struct PodDef {
    pub container: Holder<ContainerDef>,
    pub service_mounts: Vec<ServicePort>,
    pub http_bindings: Vec<HttpBinding>,
    pub tcp_bindings: Vec<TcpUdpBinding>,
    pub udp_bindings: Vec<TcpUdpBinding>,
}

#[derive(Debug, Clone)]
pub struct HttpBinding {
    pub pod_port: Port,
    pub route: HttpServiceRoute,
}

#[derive(Debug, Clone)]
pub struct TcpUdpBinding {
    pub pod_port: Port,
    pub service_port: ServicePort,
}

impl PodDef {
    pub(super) fn mixin<T: Clone + Freezable + 'static>(
        builder: &mut TypeBuilder<T>,
        ext: impl Fn(&mut T) -> Holder<Self> + Copy + 'static,
        resource: impl Fn(&mut T) -> ResourceId + Copy + 'static,
    ) {
        ContainerDef::mixin(
            builder,
            move |this| ext(this).lock().container.clone(),
            resource,
        );

        // spec: POD#service-mounts
        builder.with_fn(
            "mount",
            move |this: &mut T, svc: ServicePort| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                ext(this).lock().service_mounts.push(svc);
                Ok(this.clone())
            },
        );

        // spec: POD#port-bindings — route form
        builder.with_fn(
            "http",
            move |this: &mut T,
                  port: i64,
                  mut route: HttpServiceRoute|
                  -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let port = Port::new(port)?;
                // spec: POD#port-bindings
                refuse_external(&route.http.service)?;
                // spec: HTTP#route-redirects
                // Refuses a prefix already declared as a redirect, and records
                // the binding so one declared afterwards is refused too.
                route.record_binding()?;
                ext(this).lock().http_bindings.push(HttpBinding {
                    pod_port: port,
                    route,
                });
                Ok(this.clone())
            },
        );

        // spec: POD#port-bindings — HttpService form (defaults to route("/"))
        builder.with_fn(
            "http",
            move |this: &mut T, port: i64, service: HttpService| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let port = Port::new(port)?;
                // spec: POD#port-bindings
                refuse_external(&service.service)?;
                // Binding a bare service is binding it at "/", so the service
                // must learn about that route the same as an explicit one.
                let mut route = service.root_route();
                // spec: HTTP#route-redirects
                route.record_binding()?;
                ext(this).lock().http_bindings.push(HttpBinding {
                    pod_port: port,
                    route,
                });
                Ok(this.clone())
            },
        );

        // spec: POD#port-bindings — ServicePort form
        builder.with_fn(
            "tcp",
            move |this: &mut T, port: i64, svc: ServicePort| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let port = Port::new(port)?;
                // spec: POD#port-bindings
                refuse_external(&svc.service)?;
                ext(this).lock().tcp_bindings.push(TcpUdpBinding {
                    pod_port: port,
                    service_port: svc,
                });
                Ok(this.clone())
            },
        );

        // spec: POD#port-bindings — Service form (defaults to svc.port(port))
        builder.with_fn(
            "tcp",
            move |this: &mut T, port: i64, svc: Service| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let port = Port::new(port)?;
                let service_port = ServicePort {
                    service: svc.into(),
                    port,
                };
                ext(this).lock().tcp_bindings.push(TcpUdpBinding {
                    pod_port: port,
                    service_port,
                });
                Ok(this.clone())
            },
        );

        // spec: POD#port-bindings — ServicePort form
        builder.with_fn(
            "udp",
            move |this: &mut T, port: i64, svc: ServicePort| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let port = Port::new(port)?;
                // spec: POD#port-bindings
                refuse_external(&svc.service)?;
                ext(this).lock().udp_bindings.push(TcpUdpBinding {
                    pod_port: port,
                    service_port: svc,
                });
                Ok(this.clone())
            },
        );

        // spec: POD#port-bindings — Service form (defaults to svc.port(port))
        builder.with_fn(
            "udp",
            move |this: &mut T, port: i64, svc: Service| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let port = Port::new(port)?;
                let service_port = ServicePort {
                    service: svc.into(),
                    port,
                };
                ext(this).lock().udp_bindings.push(TcpUdpBinding {
                    pod_port: port,
                    service_port,
                });
                Ok(this.clone())
            },
        );
    }
}
