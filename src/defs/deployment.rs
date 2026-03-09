use std::ops::Range;

use rhai::{CustomType, Dynamic, Map, TypeBuilder};

use super::{
    Holder,
    pod::PodDef,
    resource::{ResourceId, ResourceKind, ResourceName},
};

#[derive(Debug, Clone)]
pub struct DeploymentDef {
    pod: Holder<PodDef>,
    scale: Range<u8>,
    strategy: DeploymentStrategy,
}

impl Default for DeploymentDef {
    fn default() -> Self {
        Self {
            pod: Holder::default(),
            scale: 1..255,
            strategy: DeploymentStrategy::default(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub enum DeploymentStrategy {
    #[default]
    Rolling,
    Replace,
}

impl DeploymentStrategy {
    pub(super) fn rhai_constant() -> Map {
        let mut map = Map::new();
        map.insert("Rolling".into(), Dynamic::from(Self::Rolling));
        map.insert("Replace".into(), Dynamic::from(Self::Replace));
        map
    }
}

#[derive(Debug, Clone)]
pub struct Deployment {
    pub name: ResourceName,
    pub def: Holder<DeploymentDef>,
}

impl CustomType for Deployment {
    fn build(mut builder: TypeBuilder<Self>) {
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
            .with_fn(
                "strategy",
                |this: &mut Self, strategy: DeploymentStrategy| {
                    this.def.lock().strategy = strategy;
                    this.clone()
                },
            );
    }
}

fn clamp_scale(n: i64) -> u8 {
    if n < 0 {
        0
    } else if n > u8::MAX as _ {
        u8::MAX
    } else {
        n as u8
    }
}
