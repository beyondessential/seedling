use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct AppDef {
    pub params: Vec<String>,
    pub resources: BTreeMap<ResourceId, ResourceDef>,
    pub actions: BTreeMap<String, ActionDef>,
    pub install: Option<InstallDef>,
}

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
pub enum ResourceDef {
    Service(ServiceDef),
    // Ingress(IngressDef),
    // Deployment(DeploymentDef),
    // Job(JobDef),
    // CronJob(CronJobDef),
    // Volume(VolumeDef),
}

pub type ServicePort = (ServiceProtocol, u16);
pub type ResourcePort = (ResourceId, u16);
pub type ServiceHttpRoute = (u16, String);

#[derive(Debug, Clone)]
pub struct ServiceDef {
    pub protocol: ServiceProtocol,
    pub port_map: BTreeMap<ServicePort, Vec<ResourcePort>>,
    pub http_routes: Option<BTreeMap<ServiceHttpRoute, Vec<ResourcePort>>>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServiceProtocol {
    #[default]
    Tcp,
    Udp,
    Http,
}

#[derive(Debug, Clone)]
pub struct ActionDef {
    pub arguments: Vec<ActionArgumentDef>,
    pub rhai_closure: (),
    pub description: Option<String>,
}

impl ActionDef {
    pub fn is_shell(&self) -> bool {
        self.arguments
            .iter()
            .any(|arg| matches!(arg, ActionArgumentDef::ShellAttach))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ActionArgumentDef {
    Runtime,
    ShellAttach,
    OldAppDef,
    AppHistory,
    InstallRequirements,
}

#[derive(Debug, Clone)]
pub struct InstallDef {
    pub action: ActionDef,
    pub requirements: BTreeMap<String, InstallRequirementDef>,
}

#[derive(Debug, Clone)]
pub struct InstallRequirementDef {
    pub kind: InstallRequirementKind,
    pub required: bool,
    pub default_value: String,
    pub description: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub enum InstallRequirementKind {
    #[default]
    Text,
    Email,
    Password,
    WeakPassword,
}
