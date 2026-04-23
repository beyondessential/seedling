use std::sync::Arc;

use rhai::{Dynamic, Map};
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    Parameter,
    Service,
    HttpService,
    Ingress,
    Deployment,
    Job,
    Volume,
    ExternalVolume,
    ExternalService,
    Action,
}

impl ResourceKind {
    pub fn rhai_constant() -> Map {
        let mut map = Map::new();
        map.insert("Parameter".into(), Dynamic::from(Self::Parameter));
        map.insert("Service".into(), Dynamic::from(Self::Service));
        map.insert("HttpService".into(), Dynamic::from(Self::HttpService));
        map.insert("Ingress".into(), Dynamic::from(Self::Ingress));
        map.insert("Deployment".into(), Dynamic::from(Self::Deployment));
        map.insert("Job".into(), Dynamic::from(Self::Job));
        map.insert("Volume".into(), Dynamic::from(Self::Volume));
        map.insert("ExternalVolume".into(), Dynamic::from(Self::ExternalVolume));
        map.insert(
            "ExternalService".into(),
            Dynamic::from(Self::ExternalService),
        );
        map.insert("Action".into(), Dynamic::from(Self::Action));
        map
    }
}

// l[impl bsl.resource]
#[derive(Debug, Clone)]
pub enum Resource {
    Service(Service),
    HttpService(HttpService),
    Ingress(Ingress),
    Deployment(Deployment),
    Job(Job),
    Volume(Volume),
    ExternalVolume(ExternalVolume),
    ExternalService(ExternalService),
}

impl Resource {
    pub fn kind(&self) -> ResourceKind {
        match self {
            Self::Service(_) => ResourceKind::Service,
            Self::HttpService(_) => ResourceKind::HttpService,
            Self::Ingress(_) => ResourceKind::Ingress,
            Self::Deployment(_) => ResourceKind::Deployment,
            Self::Job(_) => ResourceKind::Job,
            Self::Volume(_) => ResourceKind::Volume,
            Self::ExternalVolume(_) => ResourceKind::ExternalVolume,
            Self::ExternalService(_) => ResourceKind::ExternalService,
        }
    }

    pub fn name(&self) -> &ResourceName {
        match self {
            Self::Service(s) => &s.name,
            Self::HttpService(h) => &h.service.name,
            Self::Ingress(i) => &i.name,
            Self::Deployment(d) => &d.name,
            Self::Job(j) => &j.name,
            Self::Volume(v) => v
                .name
                .as_ref()
                .expect("volumes stored in AppDef always have a name"),
            Self::ExternalVolume(v) => &v.name,
            Self::ExternalService(s) => &s.name,
        }
    }

    pub fn to_dynamic(&self) -> Dynamic {
        match self {
            Self::Service(s) => Dynamic::from(s.clone()),
            Self::HttpService(h) => Dynamic::from(h.clone()),
            Self::Ingress(i) => Dynamic::from(i.clone()),
            Self::Deployment(d) => Dynamic::from(d.clone()),
            Self::Job(j) => Dynamic::from(j.clone()),
            Self::Volume(v) => Dynamic::from(v.clone()),
            Self::ExternalVolume(v) => Dynamic::from(v.clone()),
            Self::ExternalService(s) => Dynamic::from(s.clone()),
        }
    }
}
