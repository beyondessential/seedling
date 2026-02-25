use std::collections::BTreeMap;

use rhai::{CustomType, TypeBuilder};

use super::{Holder, resource::ResourceId};

pub type ServicePort = (ServiceProtocol, u16);
pub type ResourcePort = (ResourceId, u16);
pub type ServiceHttpRoute = (u16, String);

#[derive(Debug, Default, Clone)]
pub struct ServiceDef {
    pub protocol: ServiceProtocol,
    pub port_map: BTreeMap<ServicePort, Vec<ResourcePort>>,
    pub http_routes: Option<BTreeMap<ServiceHttpRoute, Vec<ResourcePort>>>,
}

#[derive(Debug, Default, Clone)]
pub struct Service(Holder<ServiceDef>);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServiceProtocol {
    #[default]
    Tcp,
    Udp,
    Http,
}

impl CustomType for Service {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Service")
            .with_fn("http", |this: &mut Self| {
                this.0.lock().unwrap().make_http();
                this.clone()
            });
    }
}

impl ServiceDef {
    pub fn is_http(&self) -> bool {
        self.http_routes.is_some()
    }

    pub fn make_http(&mut self) {
        self.http_routes = Some(Default::default());
    }
}
