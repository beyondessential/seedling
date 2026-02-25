use std::{collections::BTreeMap, ops::Range, path::PathBuf};

use rhai::{CustomType, TypeBuilder};

use crate::defs::service::HttpService;

use super::{
    Holder,
    app::App,
    resource::{ResourceId, ResourceKind, ResourceName},
    service::{PartialRoute, Service, ServicePort, ServiceProtocol},
    volume::Volume,
};

#[derive(Debug, Clone)]
pub struct DeploymentDef {
    image: Option<String>,
    command: Vec<String>,
    scale: Range<u8>,
    mounted_volumes: BTreeMap<PathBuf, Volume>,
    mounted_services: BTreeMap<u16, Service>,
}

impl Default for DeploymentDef {
    fn default() -> Self {
        Self {
            image: None,
            command: Vec::new(),
            scale: 1..255,
            mounted_volumes: BTreeMap::new(),
            mounted_services: BTreeMap::new(),
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
            .with_fn("command", |this: &mut Self, cmd: &str| {
                this.def.lock().unwrap().command = vec![cmd.into()];
                this.clone()
            })
            .with_fn("scale", |this: &mut Self, scale: i64| {
                let s = clamp_scale(scale);
                this.def.lock().unwrap().scale = s..s;
                this.clone()
            })
            .with_fn("scale", |this: &mut Self, scale: Range<i64>| {
                let min = clamp_scale(scale.start);
                let max = clamp_scale(scale.end);
                this.def.lock().unwrap().scale = min..max;
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
            .with_fn(
                "http",
                |this: &mut Self, port: i64, service: HttpService| {
                    let port = port as u16; // TODO: error on large ports

                    PartialRoute {
                        http: service,
                        prefix: "/".into(),
                    }
                    .add_resource(
                        port,
                        ResourceId {
                            kind: ResourceKind::Deployment,
                            name: this.name.clone(),
                        },
                    );
                    this.clone()
                },
            )
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
            })
            .with_fn("mount", |this: &mut Self, path: &str, volume: Volume| {
                this.def
                    .lock()
                    .unwrap()
                    .mounted_volumes
                    .insert(path.into(), volume);
                this.clone()
            })
            .with_fn(
                "mount",
                |this: &mut Self, ServicePort { service, port }: ServicePort| {
                    this.def
                        .lock()
                        .unwrap()
                        .mounted_services
                        .insert(port, service);
                    this.clone()
                },
            );
    }
}

fn clamp_scale(n: i64) -> u8 {
    if n < 0 {
        0
    } else if n > u8::MAX as _ {
        u8::MAX
    } else {
        n as u8
    }
}
