use rhai::{EvalAltResult, TypeBuilder};

use crate::runtime::barrier::runtime::{action_def, is_in_action_closure};

use super::super::{
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
    service::{ExternalService, Service},
};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl service.type]
    // l[impl app.resources.context.named]
    builder.with_fn(
        "service",
        |this: &mut App, name: &str| -> Result<Service, Box<EvalAltResult>> {
            super::super::validate_name(name)?;
            let rname = ResourceName::new(name.into());
            if is_in_action_closure() {
                let adef = action_def().ok_or_else(|| -> Box<EvalAltResult> {
                    "internal: action context but no action AppDef set".into()
                })?;
                let def = adef.load();
                let id = ResourceId {
                    kind: ResourceKind::Service,
                    name: rname,
                };
                match def.resources.get(&id) {
                    Some(Resource::Service(s)) => {
                        let mut frozen = s.clone();
                        frozen.frozen = true;
                        Ok(frozen)
                    }
                    Some(_) => Err(format!("'{}' is not a service", name).into()),
                    None => Err(format!("no static service named '{}'", name).into()),
                }
            } else {
                let weak = std::sync::Arc::downgrade(&this.def);
                let id = ResourceId {
                    kind: ResourceKind::Service,
                    name: rname.clone(),
                };
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.resources.entry(id.clone()).or_insert_with(|| {
                        Resource::Service(Service::new_with_app(rname.clone(), weak.clone()))
                    });
                    d
                });
                let def = this.def.load();
                match def.resources.get(&id) {
                    Some(Resource::Service(s)) => Ok(s.clone()),
                    _ => unreachable!(),
                }
            }
        },
    );

    // l[impl app.resources.context.anonymous]
    builder.with_fn(
        "service",
        |_this: &mut App| -> Result<Service, Box<EvalAltResult>> {
            if !is_in_action_closure() {
                return Err("anonymous services can only be created inside action closures".into());
            }
            Ok(Service::new(ResourceName::new(String::new())))
        },
    );

    // l[impl service.external]
    builder.with_fn(
        "external_service",
        |this: &mut App, name: &str| -> Result<ExternalService, Box<EvalAltResult>> {
            super::super::validate_name(name)?;
            let rname = ResourceName::new(name.into());
            let id = ResourceId {
                kind: ResourceKind::ExternalService,
                name: rname.clone(),
            };
            this.def.rcu(|d| {
                let mut d = (**d).clone();
                d.resources.entry(id.clone()).or_insert_with(|| {
                    Resource::ExternalService(ExternalService {
                        name: rname.clone(),
                    })
                });
                d
            });
            let def = this.def.load();
            match def.resources.get(&id) {
                Some(Resource::ExternalService(s)) => Ok(s.clone()),
                _ => unreachable!(),
            }
        },
    );
}
