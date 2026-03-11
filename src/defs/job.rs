use rhai::{CustomType, EvalAltResult, TypeBuilder};

use super::{
    Holder,
    pod::PodDef,
    resource::{ResourceId, ResourceKind, ResourceName},
};

// l[impl job.type]
#[derive(Debug, Default, Clone)]
pub struct JobDef {
    pub pod: Holder<PodDef>,
    pub deadline: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub name: ResourceName,
    pub def: Holder<JobDef>,
}

// l[impl job.pod]
// l[impl job.deadline]
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
        builder.with_fn(
            "deadline",
            |this: &mut Self, seconds: i64| -> Result<Job, Box<EvalAltResult>> {
                if seconds <= 0 {
                    return Err("deadline must be a positive number of seconds".into());
                }
                this.def.lock().deadline = Some(seconds as u64);
                Ok(this.clone())
            },
        );
    }
}
