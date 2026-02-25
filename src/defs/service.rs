use std::collections::{BTreeMap, BTreeSet};

use rhai::{CustomType, TypeBuilder};

use crate::defs::ingress::{Ingress, IngressDef};

use super::{
    Holder,
    app::App,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
};

pub type ResourcePort = (ResourceId, u16);
pub type ServiceHttpRoute = (u16, String);

#[derive(Debug, Default, Clone)]
pub struct ServiceDef {
    pub port_map: BTreeMap<(ServiceProtocol, u16), BTreeSet<ResourcePort>>,
    pub http_routes: Option<BTreeMap<ServiceHttpRoute, BTreeSet<ResourcePort>>>,
}

#[derive(Debug, Clone)]
pub struct Service {
    pub app: App,
    pub name: ResourceName,
    pub def: Holder<ServiceDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServiceProtocol {
    Tcp,
    Udp,
}

impl Service {
    fn make_ingress(&mut self) -> Ingress {
        let mut app = self.app.0.lock();
        let Resource::Ingress(ingress) = app
            .resources
            .entry(ResourceId {
                kind: ResourceKind::Ingress,
                name: self.name.clone(),
            })
            .or_insert_with(|| Resource::Ingress(Ingress::new(IngressDef::new(self.clone()))))
        else {
            unreachable!()
        };

        ingress.clone()
    }

    pub(super) fn make_port(self, port: u16) -> ServicePort {
        ServicePort {
            service: self,
            port,
        }
    }
}

impl CustomType for Service {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Service")
            .with_fn("http", |this: &mut Self| {
                this.def.lock().http_routes = Some(Default::default());
                HttpService {
                    service: this.clone(),
                    port: 80,
                }
            })
            .with_fn("port", |this: &mut Self, port: i64| {
                this.clone().make_port(port as _) // TODO: error on large ports
            })
            .with_fn("http", |this: &mut Self, port: i64| {
                let port = port as u16; // TODO: error on large ports

                this.def.lock().http_routes = Some(Default::default());
                HttpService {
                    service: this.clone(),
                    port,
                }
            })
            .with_fn("ingress", |this: &mut Self| this.make_ingress());
    }
}

#[derive(Debug, Clone)]
pub struct ServicePort {
    pub(super) service: Service,
    pub(super) port: u16,
}

impl CustomType for ServicePort {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("ServicePort");
    }
}

impl ServicePort {
    pub(super) fn add_resource(&self, protocol: ServiceProtocol, port: u16, resource: ResourceId) {
        let mut service = self.service.def.lock();
        service
            .port_map
            .entry((protocol, self.port))
            .or_insert(Default::default())
            .insert((resource, port));
    }
}

#[derive(Debug, Clone)]
pub struct HttpService {
    service: Service,
    port: u16,
}

impl CustomType for HttpService {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("HttpService")
            .with_fn("ingress", |this: &mut Self| this.service.make_ingress())
            .with_fn("route", |this: &mut Self, prefix: &str| PartialRoute {
                http: this.clone(),
                prefix: prefix.into(),
            })
            .with_fn("port", |this: &mut Self, port: i64| {
                this.service.clone().make_port(port as _) // TODO: error on large ports
            });
    }
}

#[derive(Debug, Clone)]
pub struct PartialRoute {
    pub(super) http: HttpService,
    pub(super) prefix: String,
}

impl CustomType for PartialRoute {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("PartialRoute");
    }
}

impl PartialRoute {
    pub(super) fn add_resource(&self, port: u16, resource: ResourceId) {
        let mut service = self.http.service.def.lock();
        let routes = service.http_routes.as_mut().unwrap();
        routes
            .entry((self.http.port, self.prefix.clone()))
            .or_insert(Default::default())
            .insert((resource.clone(), port));
        service
            .port_map
            .entry((ServiceProtocol::Tcp, self.http.port))
            .or_insert(Default::default())
            .insert((resource.clone(), port));
    }
}
