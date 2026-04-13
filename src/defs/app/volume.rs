use rhai::{Dynamic, EvalAltResult, TypeBuilder};

use crate::runtime::barrier::runtime::{action_def, is_in_action_closure, next_anon_vol_id};

use super::super::{
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
    volume::{ExternalVolume, Volume},
};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl volume.type]
    // l[impl app.resources.context.named]
    builder.with_fn(
        "volume",
        |this: &mut App, name: &str| -> Result<Volume, Box<EvalAltResult>> {
            super::super::validate_name(name)?;
            let rname = ResourceName::new(name.into());
            if is_in_action_closure() {
                let adef = action_def().ok_or_else(|| -> Box<EvalAltResult> {
                    "internal: action context but no action AppDef set".into()
                })?;
                let def = adef.lock();
                let id = ResourceId {
                    kind: ResourceKind::Volume,
                    name: rname,
                };
                match def.resources.get(&id) {
                    Some(Resource::Volume(v)) => {
                        let mut frozen = v.clone();
                        frozen.frozen = true;
                        Ok(frozen)
                    }
                    Some(_) => Err(format!("'{}' is not a volume", name).into()),
                    None => Err(format!("no static volume named '{}'", name).into()),
                }
            } else {
                let mut def = this.def.lock();
                let id = ResourceId {
                    kind: ResourceKind::Volume,
                    name: rname.clone(),
                };
                let resource = def
                    .resources
                    .entry(id)
                    .or_insert_with(|| Resource::Volume(Volume::new(Some(rname))));
                match resource {
                    Resource::Volume(v) => Ok(v.clone()),
                    _ => unreachable!(),
                }
            }
        },
    );

    // l[impl volume.type]
    // l[impl app.resources.context.anonymous]
    builder.with_fn(
        "volume",
        |_this: &mut App| -> Result<Volume, Box<EvalAltResult>> {
            if !is_in_action_closure() {
                return Err("anonymous volumes can only be created inside action closures".into());
            }
            let anon_id = next_anon_vol_id()
                .unwrap_or_else(|| format!("seedling-anon-fallback-vol-{}", uuid::Uuid::new_v4()));
            Ok(Volume::new_anonymous(anon_id))
        },
    );

    // l[impl volume.external]
    builder.with_fn(
        "external_volume",
        |this: &mut App, name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            super::super::validate_name(name)?;
            let rname = ResourceName::new(name.into());
            let mut def = this.def.lock();
            let id = ResourceId {
                kind: ResourceKind::ExternalVolume,
                name: rname.clone(),
            };
            let resource = def
                .resources
                .entry(id)
                .or_insert_with(|| Resource::ExternalVolume(ExternalVolume { name: rname }));
            match resource {
                Resource::ExternalVolume(v) => Ok(Dynamic::from(v.clone())),
                _ => unreachable!(),
            }
        },
    );
}
