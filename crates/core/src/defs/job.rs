use rhai::{CustomType, Dynamic, EvalAltResult, TypeBuilder};

use crate::runtime::barrier::runtime::is_in_action_closure;

use super::{
    Freezable, Holder,
    duration::Duration,
    pod::PodDef,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
    take::take_whole_secs,
};
use crate::runtime::identity::InstanceId;

// spec: JOB#instances
#[derive(Debug, Default, Clone)]
pub struct JobDef {
    pub pod: Holder<PodDef>,
    pub deadline: Option<u64>,
    // spec: BSL#descriptions
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub name: ResourceName,
    pub def: Holder<JobDef>,
    pub frozen: bool,
    /// The identity an anonymous Job holds for its lifetime; `None` for a named one.
    // spec: IDN#anonymous-instances
    pub anon_id: Option<InstanceId>,
}

impl super::Freezable for Job {
    // spec: APP#static-resource-immutability
    fn is_frozen(&self) -> bool {
        // An anonymous job stays mutable inside the action that creates it.
        self.frozen || (self.anon_id.is_none() && is_in_action_closure())
    }
}

impl super::app::StaticResource for Job {
    const KIND: ResourceKind = ResourceKind::Job;

    fn from_resource(resource: &Resource) -> Option<&Self> {
        match resource {
            Resource::Job(r) => Some(r),
            _ => None,
        }
    }

    fn freeze(&mut self) {
        self.frozen = true;
    }
}

impl CustomType for Job {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Job");
        // spec: JOB#declaration
        PodDef::mixin(
            &mut builder,
            move |this| this.def.lock().pod.clone(),
            |this| ResourceId {
                kind: ResourceKind::Job,
                name: this.name.clone(),
            },
        );
        // spec: JOB#deadline
        builder.with_fn(
            "deadline",
            |this: &mut Self, deadline: Dynamic| -> Result<Job, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let secs = take_whole_secs("deadline", deadline, 1..=Duration::MAX_SECS)?;
                this.def.lock().deadline = Some(secs);
                Ok(this.clone())
            },
        );
        // spec: BSL#descriptions
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
