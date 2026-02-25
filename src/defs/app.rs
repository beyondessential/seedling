use std::collections::{BTreeMap, BTreeSet};

use rhai::{CustomType, TypeBuilder};

use crate::defs::resource::ResourceName;

use super::{
    Holder,
    action::ActionDef,
    deployment::Deployment,
    install::InstallDef,
    resource::{Resource, ResourceId, ResourceKind},
    service::Service,
};

#[derive(Debug, Default, Clone)]
pub struct App(pub(super) Holder<AppDef>);

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
                let mut this = this.0.lock().unwrap();
                this.params.insert(name.into());
                "<placeholder>"
            })
            .with_fn("service", |this: &mut Self, name: &str| {
                let name = ResourceName::new(name.into());
                let app = this.clone();
                let mut this = this.0.lock().unwrap();
                let Resource::Service(service) = this
                    .resources
                    .entry(ResourceId {
                        kind: ResourceKind::Service,
                        name: name.clone(),
                    })
                    .or_insert_with(|| {
                        Resource::Service(Service {
                            app,
                            name,
                            def: Default::default(),
                        })
                    })
                else {
                    unreachable!()
                };

                service.clone()
            })
            .with_fn("deployment", |this: &mut Self, name: &str| {
                let name = ResourceName::new(name.into());
                let app = this.clone();
                let mut this = this.0.lock().unwrap();
                let Resource::Deployment(deployment) = this
                    .resources
                    .entry(ResourceId {
                        kind: ResourceKind::Deployment,
                        name: name.clone(),
                    })
                    .or_insert_with(|| {
                        Resource::Deployment(Deployment {
                            app,
                            name,
                            def: Default::default(),
                        })
                    })
                else {
                    unreachable!()
                };

                deployment.clone()
            });
    }
}
