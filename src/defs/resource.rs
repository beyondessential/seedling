use std::sync::Arc;

use super::{deployment::Deployment, ingress::Ingress, service::Service};

pub type ResourceName = Arc<String>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId {
    pub kind: ResourceKind,
    pub name: ResourceName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceKind {
    Service,
    Ingress,
    Deployment,
    Job,
    CronJob,
    Volume,
}

#[derive(Debug, Clone)]
pub enum Resource {
    Service(Service),
    Ingress(Ingress),
    Deployment(Deployment),
    // Job(Job),
    // CronJob(CronJob),
    // Volume(Volume),
}
