use std::{collections::BTreeMap, path::PathBuf};

use rhai::TypeBuilder;

use super::{Holder, resource::ResourceId, volume::Volume};

#[derive(Debug, Default, Clone)]
pub struct ContainerDef {
    kapi: k8s_openapi::api::core::v1::Container,
    volume_mounts: BTreeMap<PathBuf, Volume>,
}

impl ContainerDef {
    pub(super) fn mixin<T: Clone + 'static>(
        builder: &mut TypeBuilder<T>,
        ext: impl Fn(&mut T) -> Holder<Self> + Copy + 'static,
        _resource: impl Fn(&mut T) -> ResourceId + Copy + 'static,
    ) {
        builder
            .with_fn("image", move |this: &mut T, image: &str| {
                ext(this).lock().kapi.image = Some(image.into());
                this.clone()
            })
            .with_fn("command", move |this: &mut T, cmd: &str| {
                ext(this).lock().kapi.command = Some(vec![cmd.into()]);
                this.clone()
            })
            .with_fn("arg", move |this: &mut T, arg: &str| {
                ext(this)
                    .lock()
                    .kapi
                    .args
                    .get_or_insert_default()
                    .push(arg.into());
                this.clone()
            })
            .with_fn("mount", move |this: &mut T, path: &str, volume: Volume| {
                ext(this).lock().volume_mounts.insert(path.into(), volume);
                this.clone()
            });
    }
}
