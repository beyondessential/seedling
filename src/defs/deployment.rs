use std::ops::Range;

use rhai::{CustomType, TypeBuilder};

use super::{
    Holder,
    enums::{OnTerminate, OnUpdate},
    pod::PodDef,
    resource::{ResourceId, ResourceKind, ResourceName},
};

// l[impl deployment.type]
#[derive(Debug, Clone)]
pub struct DeploymentDef {
    pub pod: Holder<PodDef>,
    pub scale: Range<u16>,
    pub on_update: OnUpdate,
    pub on_terminate: OnTerminate,
}

impl Default for DeploymentDef {
    fn default() -> Self {
        Self {
            pod: Holder::default(),
            scale: 1..1,
            on_update: OnUpdate::default(),
            on_terminate: OnTerminate::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Deployment {
    pub name: ResourceName,
    pub def: Holder<DeploymentDef>,
}

impl CustomType for Deployment {
    fn build(mut builder: TypeBuilder<Self>) {
        // l[impl deployment.pod]
        PodDef::mixin(
            &mut builder,
            move |this| this.def.lock().pod.clone(),
            |this| ResourceId {
                kind: ResourceKind::Deployment,
                name: this.name.clone(),
            },
        );
        builder
            .with_name("Deployment")
            // l[impl deployment.scale]
            .with_fn("scale", |this: &mut Self, scale: i64| {
                let s = clamp_scale(scale);
                this.def.lock().scale = s..s;
                this.clone()
            })
            .with_fn("scale", |this: &mut Self, scale: Range<i64>| {
                let min = clamp_scale(scale.start);
                let max = clamp_scale(scale.end);
                this.def.lock().scale = min..max;
                this.clone()
            })
            // l[impl deployment.on-update]
            .with_fn("on_update", |this: &mut Self, strategy: OnUpdate| {
                this.def.lock().on_update = strategy;
                this.clone()
            })
            // l[impl deployment.on-terminate]
            .with_fn("on_terminate", |this: &mut Self, strategy: OnTerminate| {
                this.def.lock().on_terminate = strategy;
                this.clone()
            });
    }
}

fn clamp_scale(n: i64) -> u16 {
    if n < 0 {
        0
    } else if n > u16::MAX as i64 {
        u16::MAX
    } else {
        n as u16
    }
}
