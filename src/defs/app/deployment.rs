use rhai::{EvalAltResult, TypeBuilder};

use crate::runtime::barrier::runtime::{action_def, is_in_action_closure};

use super::super::{
    deployment::Deployment,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl deployment.type]
    // l[impl app.resources.context.named]
    builder.with_fn(
        "deployment",
        |this: &mut App, name: &str| -> Result<Deployment, Box<EvalAltResult>> {
            super::super::validate_name(name)?;
            let rname = ResourceName::new(name.into());
            if is_in_action_closure() {
                let adef = action_def().ok_or_else(|| -> Box<EvalAltResult> {
                    "internal: action context but no action AppDef set".into()
                })?;
                let def = adef.lock();
                let id = ResourceId {
                    kind: ResourceKind::Deployment,
                    name: rname,
                };
                match def.resources.get(&id) {
                    Some(Resource::Deployment(d)) => {
                        let mut frozen = d.clone();
                        frozen.frozen = true;
                        Ok(frozen)
                    }
                    Some(_) => Err(format!("'{}' is not a deployment", name).into()),
                    None => Err(format!("no static deployment named '{}'", name).into()),
                }
            } else {
                let mut def = this.def.lock();
                let id = ResourceId {
                    kind: ResourceKind::Deployment,
                    name: rname.clone(),
                };
                let resource = def.resources.entry(id).or_insert_with(|| {
                    Resource::Deployment(Deployment {
                        name: rname,
                        def: Default::default(),
                        frozen: false,
                    })
                });
                match resource {
                    Resource::Deployment(d) => Ok(d.clone()),
                    _ => unreachable!(),
                }
            }
        },
    );

    // l[impl app.resources.context.anonymous]
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
