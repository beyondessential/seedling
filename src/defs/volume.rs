use rhai::{CustomType, TypeBuilder};

use super::{Holder, app::App, resource::ResourceName};

#[derive(Debug, Default, Clone)]
pub struct VolumeDef {}

#[derive(Debug, Clone)]
pub struct Volume {
    pub app: App,
    pub name: ResourceName,
    pub def: Holder<VolumeDef>,
}

impl CustomType for Volume {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Volume");
    }
}
