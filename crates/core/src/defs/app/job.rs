use rhai::{EvalAltResult, TypeBuilder};

use crate::runtime::barrier::runtime::{action_def, is_in_action_closure};

use super::super::{
    job::Job,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl job.type]
    // l[impl app.resources.context.named]
    builder.with_fn(
        "job",
        |this: &mut App, name: &str| -> Result<Job, Box<EvalAltResult>> {
            super::super::validate_name(name)?;
            let rname = ResourceName::new(name.into());
            if is_in_action_closure() {
                let adef = action_def().ok_or_else(|| -> Box<EvalAltResult> {
                    "internal: action context but no action AppDef set".into()
                })?;
                let def = adef.load();
                let id = ResourceId {
                    kind: ResourceKind::Job,
                    name: rname,
                };
                match def.resources.get(&id) {
                    Some(Resource::Job(j)) => {
                        let mut frozen = j.clone();
                        frozen.frozen = true;
                        Ok(frozen)
                    }
                    Some(_) => Err(format!("'{}' is not a job", name).into()),
                    None => Err(format!("no static job named '{}'", name).into()),
                }
            } else {
                let id = ResourceId {
                    kind: ResourceKind::Job,
                    name: rname.clone(),
                };
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.resources.entry(id.clone()).or_insert_with(|| {
                        Resource::Job(Job {
                            name: rname.clone(),
                            def: Default::default(),
                            frozen: false,
                        })
                    });
                    d
                });
                let def = this.def.load();
                match def.resources.get(&id) {
                    Some(Resource::Job(j)) => Ok(j.clone()),
                    _ => unreachable!(),
                }
            }
        },
    );

    // l[impl app.resources.context.anonymous]
    builder.with_fn(
        "job",
        |_this: &mut App| -> Result<Job, Box<EvalAltResult>> {
            if !is_in_action_closure() {
                return Err("anonymous jobs can only be created inside action closures".into());
            }
            Ok(Job {
                name: ResourceName::new(String::new()),
                def: Default::default(),
                frozen: false,
            })
        },
    );
}
