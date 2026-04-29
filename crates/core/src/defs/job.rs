use rhai::{CustomType, EvalAltResult, TypeBuilder};

use super::{
    Freezable, Holder,
    pod::PodDef,
    resource::{ResourceId, ResourceKind, ResourceName},
};

// l[impl job.type]
#[derive(Debug, Default, Clone)]
pub struct JobDef {
    pub pod: Holder<PodDef>,
    pub deadline: Option<u64>,
    // l[impl bsl.resource.description]
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub name: ResourceName,
    pub def: Holder<JobDef>,
    pub frozen: bool,
}

impl super::Freezable for Job {
    fn is_frozen(&self) -> bool {
        self.frozen
    }
}

impl CustomType for Job {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Job");
        // l[impl job.pod]
        PodDef::mixin(
            &mut builder,
            move |this| this.def.lock().pod.clone(),
            |this| ResourceId {
                kind: ResourceKind::Job,
                name: this.name.clone(),
            },
        );
        // l[impl job.deadline]
        builder.with_fn(
            "deadline",
            |this: &mut Self, seconds: i64| -> Result<Job, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                if seconds <= 0 {
                    return Err("deadline must be a positive number of seconds".into());
                }
                this.def.lock().deadline = Some(seconds as u64);
                Ok(this.clone())
            },
        );
        // l[impl bsl.resource.description]
        builder.with_fn(
            "description",
            |this: &mut Self, desc: &str| -> Result<Job, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                this.def.lock().description = Some(desc.to_owned());
                Ok(this.clone())
            },
        );
    }
}
