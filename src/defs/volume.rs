use rhai::{CustomType, TypeBuilder};

use super::{Holder, app::App, resource::ResourceName};

#[derive(Debug, Default, Clone)]
pub struct VolumeDef {}

#[derive(Debug, Clone)]
pub struct Volume {
    #[expect(dead_code, reason = "not yet used")]
    pub app: App,
    #[expect(dead_code, reason = "not yet used")]
    pub name: ResourceName,
    #[expect(dead_code, reason = "not yet used")]
    pub def: Holder<VolumeDef>,
}

impl CustomType for Volume {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Volume");
    }
}
