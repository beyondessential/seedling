use rhai::{Dynamic, EvalAltResult, TypeBuilder};

use crate::runtime::barrier::runtime::{
    action_def, get_operation_volume_binding, is_in_action_closure, is_in_probe, next_anon_vol_id,
};

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
                let def = adef.load();
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
                // l[impl app.resources.static]
                let id = ResourceId {
                    kind: ResourceKind::Volume,
                    name: rname.clone(),
                };
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.resources
                        .entry(id.clone())
                        .or_insert_with(|| Resource::Volume(Volume::new(Some(rname.clone()))));
                    d
                });
                let def = this.def.load();
                match def.resources.get(&id) {
                    Some(Resource::Volume(v)) => Ok(v.clone()),
                    _ => unreachable!(),
                }
            }
        },
    );

    // l[impl volume.type]
    // l[impl app.resources.context.anonymous]
    // l[impl app.resources.dynamic]
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
    // l[impl volume.external.dynamic]
    builder.with_fn(
        "external_volume",
        |this: &mut App, name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            external_volume_resolve(this, name)
        },
    );

    // r[impl image.discover]
    // Probe-robust fallback: backup handlers commonly call
    // `app.external_volume(param["output_volume"])`, and probing without
    // caller-supplied params makes that argument unit. Rather than bubble
    // a cryptic "function not found", when we're inside a probe pass we
    // synthesise a stub name so the closure can continue and `rt.start()`
    // can still extract container images further down. Outside probe
    // passes we preserve the original strict behaviour.
    builder.with_fn(
        "external_volume",
        |this: &mut App, name: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
            if is_in_probe() {
                let resolved = if let Some(s) = name.clone().try_cast::<rhai::ImmutableString>() {
                    let s = s.to_string();
                    if s.is_empty() {
                        "probe-ext-stub".to_string()
                    } else {
                        s
                    }
                } else {
                    "probe-ext-stub".to_string()
                };
                return external_volume_resolve(this, &resolved);
            }
            Err(format!(
                "external_volume: expected a string name, got {}",
                name.type_name()
            )
            .into())
        },
    );
}

fn external_volume_resolve(this: &mut App, name: &str) -> Result<Dynamic, Box<EvalAltResult>> {
    super::super::validate_name(name)?;
    let rname = ResourceName::new(name.into());
    let op_binding = get_operation_volume_binding(name);
    let id = ResourceId {
        kind: ResourceKind::ExternalVolume,
        name: rname.clone(),
    };
    this.def.rcu(|d| {
        let mut d = (**d).clone();
        d.resources.entry(id.clone()).or_insert_with(|| {
            Resource::ExternalVolume(ExternalVolume {
                name: rname.clone(),
                operation_binding: op_binding.clone(),
                description: None,
            })
        });
        d
    });
    let def = this.def.load();
    match def.resources.get(&id) {
        Some(Resource::ExternalVolume(v)) => Ok(Dynamic::from(v.clone())),
        _ => unreachable!(),
    }
}
