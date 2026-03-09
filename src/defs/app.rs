use std::collections::{BTreeMap, BTreeSet};

use rhai::{CustomType, TypeBuilder};

use crate::defs::resource::ResourceName;

use super::{
    Holder,
    action::ActionDef,
    deployment::Deployment,
    install::InstallDef,
    job::Job,
    resource::{Resource, ResourceId, ResourceKind},
    service::Service,
    volume::Volume,
};

#[derive(Debug, Default, Clone)]
pub struct App(pub(super) Holder<AppDef>);

#[derive(Debug, Default, Clone)]
pub struct AppDef {
    pub params: BTreeSet<String>,
    pub external_volumes: BTreeSet<String>,
    pub resources: BTreeMap<ResourceId, Resource>,
    #[expect(dead_code, reason = "not yet used")]
    pub actions: BTreeMap<String, Holder<ActionDef>>,
    #[expect(dead_code, reason = "not yet used")]
    pub install: Option<Holder<InstallDef>>,
}

impl CustomType for App {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("App")
            .with_fn("param", |this: &mut Self, name: &str| {
                let mut this = this.0.lock();
                this.params.insert(name.into());
                "<placeholder>"
            })
            .with_fn("external_volume", |this: &mut Self, name: &str| {
                let app = this.clone();
                let mut this = this.0.lock();
                this.external_volumes.insert(name.into());
                let name = ResourceName::new(name.into());
                Volume {
                    app,
                    name,
                    def: Default::default(),
                }
            })
            .with_fn("service", |this: &mut Self, name: &str| {
                let name = ResourceName::new(name.into());
                let app = this.clone();
                let mut this = this.0.lock();
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
                let mut this = this.0.lock();
                let Resource::Deployment(deployment) = this
                    .resources
                    .entry(ResourceId {
                        kind: ResourceKind::Deployment,
                        name: name.clone(),
                    })
                    .or_insert_with(|| {
                        Resource::Deployment(Deployment {
                            name,
                            def: Default::default(),
                        })
                    })
                else {
                    unreachable!()
                };

                deployment.clone()
            })
            .with_fn("job", |this: &mut Self, name: &str| {
                let name = ResourceName::new(name.into());
                let mut this = this.0.lock();
                let Resource::Job(job) = this
                    .resources
                    .entry(ResourceId {
                        kind: ResourceKind::Job,
                        name: name.clone(),
                    })
                    .or_insert_with(|| {
                        Resource::Job(Job {
                            name,
                            def: Default::default(),
                        })
                    })
                else {
                    unreachable!()
                };

                job.clone()
            });
    }
}
