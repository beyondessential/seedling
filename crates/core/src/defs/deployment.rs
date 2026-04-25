use std::ops::Range;

use rhai::{CustomType, EvalAltResult, TypeBuilder};

use super::{
    Freezable, Holder,
    container::ContainerDef,
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
    pub frozen: bool,
}

impl Freezable for Deployment {
    fn is_frozen(&self) -> bool {
        self.frozen
    }
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
        // l[impl container.healthcheck]
        ContainerDef::healthcheck_mixin(&mut builder, |this| {
            this.def.lock().pod.lock().container.clone()
        });
        builder
            .with_name("Deployment")
            // l[impl deployment.scale]
            .with_fn(
                "scale",
                |this: &mut Self, scale: i64| -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let s = validate_scale(scale)?;
                    // l[impl deployment.scale.max-lower-bound]
                    check_lower_bound(s)?;
                    this.def.lock().scale = s..s;
                    Ok(this.clone())
                },
            )
            .with_fn(
                "scale",
                |this: &mut Self, scale: Range<i64>| -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let min = validate_scale_lower(scale.start)?;
                    let max = validate_scale(scale.end)?;
                    // l[impl deployment.scale.max-lower-bound]
                    check_lower_bound(min)?;
                    if max == 0 {
                        return Err("scale upper bound must be non-zero".into());
                    }
                    this.def.lock().scale = min..max;
                    Ok(this.clone())
                },
            )
            // l[impl deployment.on-update]
            .with_fn(
                "on_update",
                |this: &mut Self, strategy: OnUpdate| -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().on_update = strategy;
                    Ok(this.clone())
                },
            )
            // l[impl deployment.on-terminate]
            .with_fn(
                "on_terminate",
                |this: &mut Self,
                 strategy: OnTerminate|
                 -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().on_terminate = strategy;
                    Ok(this.clone())
                },
            );
    }
}

const MAX_SCALE_LOWER_BOUND: u16 = 10;

fn validate_scale(n: i64) -> Result<u16, Box<EvalAltResult>> {
    if n <= 0 {
        return Err(format!("scale must be a positive non-zero integer, got {n}").into());
    }
    if n > u16::MAX as i64 {
        return Err(format!("scale {n} exceeds maximum of {}", u16::MAX).into());
    }
    Ok(n as u16)
}

fn validate_scale_lower(n: i64) -> Result<u16, Box<EvalAltResult>> {
    if n < 0 {
        return Err(format!("scale lower bound must not be negative, got {n}").into());
    }
    if n > u16::MAX as i64 {
        return Err(format!("scale {n} exceeds maximum of {}", u16::MAX).into());
    }
    Ok(n as u16)
}

fn check_lower_bound(n: u16) -> Result<(), Box<EvalAltResult>> {
    if n > MAX_SCALE_LOWER_BOUND {
        return Err(
            format!("scale lower bound {n} exceeds maximum of {MAX_SCALE_LOWER_BOUND}").into(),
        );
    }
    Ok(())
}
