use std::ops::{Range, RangeInclusive};

use rhai::{CustomType, EvalAltResult, TypeBuilder};

use crate::runtime::barrier::runtime::is_in_action_closure;

use super::{
    Freezable, Holder,
    container::parse_healthcheck,
    enums::{OnTerminate, OnUpdate, Priority},
    pod::PodDef,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
    take::take_int_in_range,
};

// spec: DEP#declaration
#[derive(Debug, Clone)]
pub struct DeploymentDef {
    pub pod: Holder<PodDef>,
    /// The lower and upper bounds of the instance count, both inclusive.
    pub scale: RangeInclusive<u16>,
    pub on_update: OnUpdate,
    pub on_terminate: OnTerminate,
    // spec: DEP#priority
    pub priority: Priority,
    // spec: BSL#descriptions
    pub description: Option<String>,
}

impl Default for DeploymentDef {
    fn default() -> Self {
        Self {
            pod: Holder::default(),
            scale: 1..=1,
            on_update: OnUpdate::default(),
            on_terminate: OnTerminate::default(),
            priority: Priority::default(),
            description: None,
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
    // spec: APP#static-resource-immutability
    fn is_frozen(&self) -> bool {
        // Anonymous deployments use an empty name as a sentinel (see app/deployment.rs).
        self.frozen || (!self.name.is_empty() && is_in_action_closure())
    }
}

impl super::app::StaticResource for Deployment {
    const KIND: ResourceKind = ResourceKind::Deployment;

    fn from_resource(resource: &Resource) -> Option<&Self> {
        match resource {
            Resource::Deployment(r) => Some(r),
            _ => None,
        }
    }

    fn freeze(&mut self) {
        self.frozen = true;
    }
}

impl CustomType for Deployment {
    fn build(mut builder: TypeBuilder<Self>) {
        // spec: DEP#declaration
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
            // spec: DEP#health-checks
            .with_fn(
                "healthcheck",
                |this: &mut Self, config: rhai::Map| -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let hc = parse_healthcheck(config)?;
                    this.def.lock().pod.lock().container.lock().healthcheck = Some(hc);
                    Ok(this.clone())
                },
            )
            // spec: DEP#scale
            .with_fn(
                "scale",
                |this: &mut Self, scale: i64| -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let s = validate_scale(scale)?;
                    check_lower_bound(s)?;
                    this.def.lock().scale = s..=s;
                    Ok(this.clone())
                },
            )
            // spec: DEP#scale
            // `lower..end` excludes `end`, as it does everywhere in Rhai.
            .with_fn(
                "scale",
                |this: &mut Self, scale: Range<i64>| -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let bounds = scale_bounds(
                        scale.start,
                        scale.end.saturating_sub(1),
                        &format!("{scale:?}"),
                    )?;
                    this.def.lock().scale = bounds;
                    Ok(this.clone())
                },
            )
            // spec: DEP#scale
            .with_fn(
                "scale",
                |this: &mut Self,
                 scale: RangeInclusive<i64>|
                 -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let bounds = scale_bounds(*scale.start(), *scale.end(), &format!("{scale:?}"))?;
                    this.def.lock().scale = bounds;
                    Ok(this.clone())
                },
            )
            // spec: DEP#update-and-termination-strategies
            .with_fn(
                "on_update",
                |this: &mut Self, strategy: OnUpdate| -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().on_update = strategy;
                    Ok(this.clone())
                },
            )
            // spec: DEP#update-and-termination-strategies
            .with_fn(
                "on_terminate",
                |this: &mut Self,
                 strategy: OnTerminate|
                 -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().on_terminate = strategy;
                    Ok(this.clone())
                },
            )
            // spec: DEP#priority
            .with_fn(
                "priority",
                |this: &mut Self, level: Priority| -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().priority = level;
                    Ok(this.clone())
                },
            )
            // spec: BSL#descriptions
            .with_fn(
                "description",
                |this: &mut Self, desc: &str| -> Result<Deployment, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().description = Some(desc.to_owned());
                    Ok(this.clone())
                },
            );
    }
}

const MAX_SCALE_LOWER_BOUND: u16 = 10;

fn validate_scale(n: i64) -> Result<u16, Box<EvalAltResult>> {
    take_int_in_range::<u16>("scale", n, 1..=i64::from(u16::MAX))
}

fn validate_scale_lower(n: i64) -> Result<u16, Box<EvalAltResult>> {
    take_int_in_range::<u16>("scale lower bound", n, 0..=i64::from(u16::MAX))
}

/// Check a scale range's bounds, `upper` inclusive, naming the range as the
/// script wrote it.
// spec: DEP#scale
fn scale_bounds(
    lower: i64,
    upper: i64,
    written: &str,
) -> Result<RangeInclusive<u16>, Box<EvalAltResult>> {
    let lower = validate_scale_lower(lower)?;
    check_lower_bound(lower)?;
    if upper < i64::from(lower) {
        return Err(format!(
            "scale range {written} holds no instance count: its lower bound must not exceed its \
             upper bound"
        )
        .into());
    }
    let upper = take_int_in_range::<u16>("scale upper bound", upper, 1..=i64::from(u16::MAX))?;
    Ok(lower..=upper)
}

fn check_lower_bound(n: u16) -> Result<(), Box<EvalAltResult>> {
    if n > MAX_SCALE_LOWER_BOUND {
        return Err(
            format!("scale lower bound {n} exceeds maximum of {MAX_SCALE_LOWER_BOUND}").into(),
        );
    }
    Ok(())
}
