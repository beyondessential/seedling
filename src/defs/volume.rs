use rhai::{CustomType, TypeBuilder};

use super::{Holder, resource::ResourceName};

#[derive(Debug, Default, Clone)]
pub struct VolumeDef {
    pub read_only: bool,
    pub writes: Vec<(String, String)>,
}

// l[impl volume.type]
#[derive(Debug, Clone)]
pub struct Volume {
    pub name: Option<ResourceName>,
    pub def: Holder<VolumeDef>,
}

impl Volume {
    pub fn new(name: Option<ResourceName>) -> Self {
        Self {
            name,
            def: Default::default(),
        }
    }
}

impl CustomType for Volume {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Volume")
            // l[impl volume.readonly]
            .with_fn("readonly", |this: &mut Self| {
                this.def.lock().read_only = true;
                this.clone()
            })
            // l[impl volume.write]
            .with_fn("write", |this: &mut Self, path: &str, contents: &str| {
                this.def.lock().writes.push((path.into(), contents.into()));
                this.clone()
            });
    }
}

// l[impl volume.external]
#[derive(Debug, Clone)]
pub struct ExternalVolume {
    pub name: ResourceName,
}

impl CustomType for ExternalVolume {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("ExternalVolume");
    }
}
