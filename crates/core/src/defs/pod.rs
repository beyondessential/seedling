use rhai::{EvalAltResult, TypeBuilder};

use super::{
    Freezable, Holder, Port,
    container::ContainerDef,
    resource::ResourceId,
    service::{HttpService, HttpServiceRoute, Service, ServicePort},
};

// l[impl pod.interface]
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

        // l[impl pod.mount-serviceport]
        builder.with_fn(
            "mount",
            move |this: &mut T, svc: ServicePort| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                ext(this).lock().service_mounts.push(svc);
                Ok(this.clone())
            },
        );

        // l[impl pod.http] — route form
        builder.with_fn(
            "http",
            move |this: &mut T,
                  port: i64,
                  route: HttpServiceRoute|
                  -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let port = Port::new(port)?;
                ext(this).lock().http_bindings.push(HttpBinding {
                    pod_port: port,
                    route,
                });
                Ok(this.clone())
            },
        );

        // l[impl pod.http] — HttpService form (defaults to route("/"))
        builder.with_fn(
            "http",
            move |this: &mut T, port: i64, service: HttpService| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let port = Port::new(port)?;
                let route = HttpServiceRoute {
                    http: service,
                    prefix: "/".into(),
                };
                ext(this).lock().http_bindings.push(HttpBinding {
                    pod_port: port,
                    route,
                });
                Ok(this.clone())
            },
        );

        // l[impl pod.tcp] — ServicePort form
        builder.with_fn(
            "tcp",
            move |this: &mut T, port: i64, svc: ServicePort| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let port = Port::new(port)?;
                ext(this).lock().tcp_bindings.push(TcpUdpBinding {
                    pod_port: port,
                    service_port: svc,
                });
                Ok(this.clone())
            },
        );

        // l[impl pod.tcp] — Service form (defaults to svc.port(port))
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

        // l[impl pod.udp] — ServicePort form
        builder.with_fn(
            "udp",
            move |this: &mut T, port: i64, svc: ServicePort| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let port = Port::new(port)?;
                ext(this).lock().udp_bindings.push(TcpUdpBinding {
                    pod_port: port,
                    service_port: svc,
                });
                Ok(this.clone())
            },
        );

        // l[impl pod.udp] — Service form (defaults to svc.port(port))
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
