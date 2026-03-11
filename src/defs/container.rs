use std::{collections::BTreeMap, path::PathBuf};

use rhai::{Array, TypeBuilder};

use super::{
    Holder,
    enums::OnExit,
    resource::ResourceId,
    volume::{ExternalVolume, Volume},
};

// r[container.interface]
#[derive(Debug, Default, Clone)]
pub struct ContainerDef {
    pub image: Option<String>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub env: Vec<(String, String)>,
    pub volume_mounts: BTreeMap<PathBuf, VolumeMount>,
    pub on_exit: OnExit,
}

#[derive(Debug, Clone)]
pub enum VolumeMount {
    Volume(Volume),
    ExternalVolume(ExternalVolume),
}

impl ContainerDef {
    pub(super) fn mixin<T: Clone + 'static>(
        builder: &mut TypeBuilder<T>,
        ext: impl Fn(&mut T) -> Holder<Self> + Copy + 'static,
        _resource: impl Fn(&mut T) -> ResourceId + Copy + 'static,
    ) {
        // r[container.image]
        builder.with_fn("image", move |this: &mut T, image: &str| {
            ext(this).lock().image = Some(image.into());
            this.clone()
        });

        // r[container.command]
        builder
            .with_fn("command", move |this: &mut T, cmd: &str| {
                ext(this).lock().command = Some(vec![cmd.into()]);
                this.clone()
            })
            .with_fn("command", move |this: &mut T, entrypoint: Array| {
                ext(this).lock().command = Some(
                    entrypoint
                        .into_iter()
                        .map(|v| v.into_string().unwrap_or_default())
                        .collect(),
                );
                this.clone()
            });

        // r[container.arg]
        builder
            .with_fn("arg", move |this: &mut T, arg: &str| {
                ext(this)
                    .lock()
                    .args
                    .get_or_insert_default()
                    .push(arg.into());
                this.clone()
            })
            .with_fn("arg", move |this: &mut T, args: Array| {
                let holder = ext(this);
                let mut def = holder.lock();
                let list = def.args.get_or_insert_default();
                for v in args {
                    list.push(v.into_string().unwrap_or_default());
                }
                this.clone()
            });

        // r[container.env]
        builder
            .with_fn("env", move |this: &mut T, name: &str, value: &str| {
                let holder = ext(this);
                let mut def = holder.lock();
                if let Some(pos) = def.env.iter().position(|(k, _)| k == name) {
                    def.env[pos].1 = value.into();
                } else {
                    def.env.push((name.into(), value.into()));
                }
                this.clone()
            })
            .with_fn("env", move |this: &mut T, vars: Array| {
                let holder = ext(this);
                let mut def = holder.lock();
                for item in vars {
                    if let Ok(map) = item.try_cast::<rhai::Map>() {
                        let name = map
                            .get("name")
                            .and_then(|v| v.clone().into_string().ok())
                            .unwrap_or_default();
                        let value = map
                            .get("value")
                            .and_then(|v| v.clone().into_string().ok())
                            .unwrap_or_default();
                        if let Some(pos) = def.env.iter().position(|(k, _)| k == &name) {
                            def.env[pos].1 = value;
                        } else {
                            def.env.push((name, value));
                        }
                    }
                }
                this.clone()
            });

        // r[container.mount-volume]
        builder
            .with_fn("mount", move |this: &mut T, path: &str, volume: Volume| {
                ext(this)
                    .lock()
                    .volume_mounts
                    .insert(path.into(), VolumeMount::Volume(volume));
                this.clone()
            })
            .with_fn(
                "mount",
                move |this: &mut T, path: &str, volume: ExternalVolume| {
                    ext(this)
                        .lock()
                        .volume_mounts
                        .insert(path.into(), VolumeMount::ExternalVolume(volume));
                    this.clone()
                },
            );

        // r[container.on-exit]
        builder.with_fn("on_exit", move |this: &mut T, strategy: OnExit| {
            ext(this).lock().on_exit = strategy;
            this.clone()
        });
    }
}
