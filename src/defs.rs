use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use rhai::{CustomType, TypeBuilder};

type Holder<T> = Arc<Mutex<T>>;

#[derive(Debug, Default, Clone)]
pub struct App(Holder<AppDef>);

#[derive(Debug, Default, Clone)]
pub struct AppDef {
    pub params: BTreeSet<String>,
    pub resources: BTreeMap<ResourceId, Resource>,
    pub actions: BTreeMap<String, Holder<ActionDef>>,
    pub install: Option<Holder<InstallDef>>,
}

impl CustomType for App {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("App")
            .with_fn("param", |this: &mut Self, name: &str| {
                this.0.lock().unwrap().add_param(name)
            })
            .with_fn("service", |this: &mut Self, name: &str| {
                this.0.lock().unwrap().add_service(name)
            });
    }
}

impl AppDef {
    fn add_param(&mut self, name: &str) -> &'static str {
        self.params.insert(name.into());
        "<placeholder>"
    }

    fn add_service(&mut self, name: &str) -> Service {
        let Resource::Service(service) = self
            .resources
            .entry(ResourceId {
                kind: ResourceKind::Service,
                name: name.into(),
            })
            .or_insert_with(|| Resource::Service(Service::default()))
        else {
            unreachable!()
        };

        service.clone()
    }
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
pub enum Resource {
    Service(Service),
    Ingress(Ingress),
    // Ingress(IngressDef),
    // Deployment(DeploymentDef),
    // Job(JobDef),
    // CronJob(CronJobDef),
    // Volume(VolumeDef),
}

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
            });
    }
}

impl ServiceDef {
    pub fn is_http(&self) -> bool {
        self.http_routes.is_some()
    }

    fn make_http(&mut self) {
        self.http_routes = Some(Default::default());
    }
}

#[derive(Debug, Default, Clone)]
pub struct IngressDef {}

#[derive(Debug, Default, Clone)]
pub struct Ingress(Holder<IngressDef>);

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
