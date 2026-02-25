use super::{deployment::Deployment, ingress::Ingress, service::Service};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId {
    pub kind: ResourceKind,
    pub name: String,
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
