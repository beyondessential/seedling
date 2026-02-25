use std::collections::BTreeMap;

use rhai::TypeBuilder;

use super::{
    Holder,
    container::ContainerDef,
    resource::ResourceId,
    service::{HttpService, PartialRoute, Service, ServicePort, ServiceProtocol},
};

#[derive(Debug, Default, Clone)]
pub struct PodDef {
    container: Holder<ContainerDef>,
    service_mounts: BTreeMap<u16, Service>,
}

impl PodDef {
    pub(super) fn mixin<T: Clone + 'static>(
        builder: &mut TypeBuilder<T>,
        ext: impl Fn(&mut T) -> Holder<Self> + Copy + 'static,
        resource: impl Fn(&mut T) -> ResourceId + Copy + 'static,
    ) {
        ContainerDef::mixin(
            builder,
            move |this| ext(this).lock().container.clone(),
            resource,
        );
        builder
            .with_fn(
                "http",
                move |this: &mut T, port: i64, route: PartialRoute| {
                    let port = port as u16; // TODO: error on large ports

                    route.add_resource(port, resource(this));
                    this.clone()
                },
            )
            .with_fn(
                "http",
                move |this: &mut T, port: i64, service: HttpService| {
                    let port = port as u16; // TODO: error on large ports

                    PartialRoute {
                        http: service,
                        prefix: "/".into(),
                    }
                    .add_resource(port, resource(this));
                    this.clone()
                },
            )
            .with_fn("tcp", move |this: &mut T, port: i64, svc: ServicePort| {
                let port = port as u16; // TODO: error on large ports

                svc.add_resource(ServiceProtocol::Tcp, port, resource(this));
                this.clone()
            })
            .with_fn("tcp", move |this: &mut T, port: i64, svc: Service| {
                let port = port as u16; // TODO: error on large ports

                svc.make_port(port)
                    .add_resource(ServiceProtocol::Tcp, port, resource(this));
                this.clone()
            })
            .with_fn("udp", move |this: &mut T, port: i64, svc: ServicePort| {
                let port = port as u16; // TODO: error on large ports

                svc.add_resource(ServiceProtocol::Udp, port, resource(this));
                this.clone()
            })
            .with_fn("udp", move |this: &mut T, port: i64, svc: Service| {
                let port = port as u16; // TODO: error on large ports

                svc.make_port(port)
                    .add_resource(ServiceProtocol::Udp, port, resource(this));
                this.clone()
            })
            .with_fn(
                "mount",
                move |this: &mut T, ServicePort { service, port }: ServicePort| {
                    ext(this).lock().service_mounts.insert(port, service);
                    this.clone()
                },
            );
    }
}
