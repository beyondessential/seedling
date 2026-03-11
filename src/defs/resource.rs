use std::sync::Arc;

use rhai::{Dynamic, Map};

use super::{
    deployment::Deployment,
    ingress::Ingress,
    job::Job,
    service::{ExternalService, HttpService, Service},
    volume::{ExternalVolume, Volume},
};

pub type ResourceName = Arc<String>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId {
    pub kind: ResourceKind,
    pub name: ResourceName,
}

// l[impl const.resource-type.enum]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceKind {
    Parameter,
    Service,
    HttpService,
    ExternalService,
    Ingress,
    Deployment,
    Job,
    Volume,
    ExternalVolume,
    Action,
}

impl ResourceKind {
    pub fn rhai_constant() -> Map {
        let mut map = Map::new();
        map.insert("Parameter".into(), Dynamic::from(Self::Parameter));
        map.insert("Service".into(), Dynamic::from(Self::Service));
        map.insert("HttpService".into(), Dynamic::from(Self::HttpService));
        map.insert(
            "ExternalService".into(),
            Dynamic::from(Self::ExternalService),
        );
        map.insert("Ingress".into(), Dynamic::from(Self::Ingress));
        map.insert("Deployment".into(), Dynamic::from(Self::Deployment));
        map.insert("Job".into(), Dynamic::from(Self::Job));
        map.insert("Volume".into(), Dynamic::from(Self::Volume));
        map.insert("ExternalVolume".into(), Dynamic::from(Self::ExternalVolume));
        map.insert("Action".into(), Dynamic::from(Self::Action));
        map
    }
}

#[derive(Debug, Clone)]
pub enum Resource {
    Service(Service),
    HttpService(HttpService),
    ExternalService(ExternalService),
    Ingress(Ingress),
    Deployment(Deployment),
    Job(Job),
    Volume(Volume),
    ExternalVolume(ExternalVolume),
}

impl Resource {
    pub fn kind(&self) -> ResourceKind {
        match self {
            Self::Service(_) => ResourceKind::Service,
            Self::HttpService(_) => ResourceKind::HttpService,
            Self::ExternalService(_) => ResourceKind::ExternalService,
            Self::Ingress(_) => ResourceKind::Ingress,
            Self::Deployment(_) => ResourceKind::Deployment,
            Self::Job(_) => ResourceKind::Job,
            Self::Volume(_) => ResourceKind::Volume,
            Self::ExternalVolume(_) => ResourceKind::ExternalVolume,
        }
    }
}
