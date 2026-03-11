use rhai::{CustomType, TypeBuilder};

use super::{
    Holder,
    pod::PodDef,
    resource::{ResourceId, ResourceKind, ResourceName},
};

// r[job.type]
#[derive(Debug, Clone)]
pub struct JobDef {
    pub pod: Holder<PodDef>,
    pub deadline: Option<u64>,
}

impl Default for JobDef {
    fn default() -> Self {
        Self {
            pod: Holder::default(),
            deadline: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub name: ResourceName,
    pub def: Holder<JobDef>,
}

// r[job.pod]
// r[job.deadline]
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
        builder.with_fn("deadline", |this: &mut Self, seconds: i64| {
            if seconds <= 0 {
                panic!("deadline must be a positive number of seconds");
            }
            this.def.lock().deadline = Some(seconds as u64);
            this.clone()
        });
    }
}
