use std::ops::Range;

use rhai::{CustomType, TypeBuilder};

use super::{
    Holder,
    app::App,
    resource::{ResourceId, ResourceKind, ResourceName},
    service::{PartialRoute, Service, ServicePort, ServiceProtocol},
};

#[derive(Debug, Clone)]
pub struct DeploymentDef {
    image: Option<String>,
    scale: Range<u8>,
}

impl Default for DeploymentDef {
    fn default() -> Self {
        Self {
            image: None,
            scale: 1..255,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Deployment {
    pub app: App,
    pub name: ResourceName,
    pub def: Holder<DeploymentDef>,
}

impl CustomType for Deployment {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Deployment")
            .with_fn("image", |this: &mut Self, image: &str| {
                this.def.lock().unwrap().image = Some(image.into());
                this.clone()
            })
            .with_fn("scale", |this: &mut Self, scale: i64| {
                let s = if scale < 0 {
                    0
                } else if scale > u8::MAX as _ {
                    u8::MAX
                } else {
                    scale as u8
                };
                this.def.lock().unwrap().scale = s..s;
                this.clone()
            })
            .with_fn("http", |this: &mut Self, port: i64, route: PartialRoute| {
                let port = port as u16; // TODO: error on large ports

                route.add_resource(
                    port,
                    ResourceId {
                        kind: ResourceKind::Deployment,
                        name: this.name.clone(),
                    },
                );
                this.clone()
            })
            .with_fn("tcp", |this: &mut Self, port: i64, svc: ServicePort| {
                let port = port as u16; // TODO: error on large ports

                svc.add_resource(
                    ServiceProtocol::Tcp,
                    port,
                    ResourceId {
                        kind: ResourceKind::Deployment,
                        name: this.name.clone(),
                    },
                );
                this.clone()
            })
            .with_fn("tcp", |this: &mut Self, port: i64, svc: Service| {
                let port = port as u16; // TODO: error on large ports

                svc.make_port(port).add_resource(
                    ServiceProtocol::Tcp,
                    port,
                    ResourceId {
                        kind: ResourceKind::Deployment,
                        name: this.name.clone(),
                    },
                );
                this.clone()
            })
            .with_fn("udp", |this: &mut Self, port: i64, svc: ServicePort| {
                let port = port as u16; // TODO: error on large ports

                svc.add_resource(
                    ServiceProtocol::Udp,
                    port,
                    ResourceId {
                        kind: ResourceKind::Deployment,
                        name: this.name.clone(),
                    },
                );
                this.clone()
            })
            .with_fn("udp", |this: &mut Self, port: i64, svc: Service| {
                let port = port as u16; // TODO: error on large ports

                svc.make_port(port).add_resource(
                    ServiceProtocol::Udp,
                    port,
                    ResourceId {
                        kind: ResourceKind::Deployment,
                        name: this.name.clone(),
                    },
                );
                this.clone()
            });
    }
}
