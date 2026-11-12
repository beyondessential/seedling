use rhai::{EvalAltResult, TypeBuilder};

use crate::runtime::barrier::runtime::is_in_action_closure;

use super::super::{
    deployment::Deployment,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // spec: DEP#declaration
    // spec: APP#named-resources
    builder.with_fn(
        "deployment",
        |this: &mut App, name: &str| -> Result<Deployment, Box<EvalAltResult>> {
            super::super::validate_name(name)?;
            super::ensure_not_validating()?;
            let rname = ResourceName::new(name.into());
            if is_in_action_closure() {
                super::static_ref::<Deployment>(this, name)
            } else {
                let id = ResourceId {
                    kind: ResourceKind::Deployment,
                    name: rname.clone(),
                };
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.resources.entry(id.clone()).or_insert_with(|| {
                        Resource::Deployment(Deployment {
                            name: rname.clone(),
                            def: Default::default(),
                            frozen: false,
                        })
                    });
                    d
                });
                let def = this.def.load();
                match def.resources.get(&id) {
                    Some(Resource::Deployment(d)) => Ok(d.clone()),
                    _ => unreachable!(),
                }
            }
        },
    );

    // spec: APP#anonymous-resources
    builder.with_fn(
        "deployment",
        |_this: &mut App| -> Result<Deployment, Box<EvalAltResult>> {
            if !is_in_action_closure() {
                return Err(
                    "anonymous deployments can only be created inside action closures".into(),
                );
            }
            Ok(Deployment {
                name: ResourceName::new(String::new()),
                def: Default::default(),
                frozen: false,
            })
        },
    );
}
