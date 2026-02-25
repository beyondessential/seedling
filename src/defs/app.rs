use std::collections::{BTreeMap, BTreeSet};

use rhai::{CustomType, TypeBuilder};

use super::{
    Holder,
    action::ActionDef,
    deployment::Deployment,
    ingress::Ingress,
    install::InstallDef,
    resource::{Resource, ResourceId, ResourceKind},
    service::Service,
};

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
            })
            .with_fn("ingress", |this: &mut Self, name: &str| {
                this.0.lock().unwrap().add_ingress(name)
            })
            .with_fn("deployment", |this: &mut Self, name: &str| {
                this.0.lock().unwrap().add_deployment(name)
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

    fn add_ingress(&mut self, name: &str) -> Ingress {
        let Resource::Ingress(ingress) = self
            .resources
            .entry(ResourceId {
                kind: ResourceKind::Ingress,
                name: name.into(),
            })
            .or_insert_with(|| Resource::Ingress(Ingress::default()))
        else {
            unreachable!()
        };

        ingress.clone()
    }

    fn add_deployment(&mut self, name: &str) -> Deployment {
        let Resource::Deployment(deployment) = self
            .resources
            .entry(ResourceId {
                kind: ResourceKind::Deployment,
                name: name.into(),
            })
            .or_insert_with(|| Resource::Deployment(Deployment::default()))
        else {
            unreachable!()
        };

        deployment.clone()
    }
}
