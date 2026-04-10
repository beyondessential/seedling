use rhai::{CustomType, EvalAltResult, TypeBuilder};

use super::{Freezable, Holder, resource::ResourceName};

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
    pub frozen: bool,
}

impl Volume {
    pub fn new(name: Option<ResourceName>) -> Self {
        Self {
            name,
            def: Default::default(),
            frozen: false,
        }
    }
}

impl super::Freezable for Volume {
    fn is_frozen(&self) -> bool {
        self.frozen
    }
}

impl CustomType for Volume {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Volume")
            // l[impl volume.readonly]
            .with_fn(
                "readonly",
                |this: &mut Self| -> Result<Volume, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().read_only = true;
                    Ok(this.clone())
                },
            )
            // l[impl volume.write]
            .with_fn(
                "write",
                |this: &mut Self,
                 path: &str,
                 contents: &str|
                 -> Result<Volume, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().writes.push((path.into(), contents.into()));
                    Ok(this.clone())
                },
            );
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
