use rhai::{CustomType, TypeBuilder};

use super::{
    Holder,
    pod::PodDef,
    resource::{ResourceId, ResourceKind, ResourceName},
};

#[derive(Debug, Clone)]
pub struct JobDef {
    pod: Holder<PodDef>,
}

impl Default for JobDef {
    fn default() -> Self {
        Self {
            pod: Holder::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub name: ResourceName,
    pub def: Holder<JobDef>,
}

impl CustomType for Job {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Job");
        PodDef::mixin(
            &mut builder,
            move |this| this.def.lock().pod.clone(),
            |this| ResourceId {
                kind: ResourceKind::Job,
                name: this.name.clone(),
            },
        );
    }
}
