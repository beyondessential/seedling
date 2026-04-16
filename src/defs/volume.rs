use std::path::{Component, PathBuf};

use rhai::{CustomType, EvalAltResult, Map, TypeBuilder};

use super::{Freezable, Holder, resource::ResourceName};

// l[impl volume.write.validation]
fn validate_volume_write_path(path: &str) -> Result<(), Box<EvalAltResult>> {
    if path.contains('\0') {
        return Err("volume write path must not contain null bytes".into());
    }
    if !path.starts_with('/') {
        return Err(format!("volume write path must be absolute, got '{path}'").into());
    }

    for component in PathBuf::from(path).components() {
        if matches!(component, Component::ParentDir) {
            return Err(
                format!("volume write path must not contain '..' components: '{path}'").into(),
            );
        }
    }

    // After stripping `.` and redundant `/`, the path must have at least one
    // real segment (i.e. it must not resolve to just `/`).
    let has_normal = PathBuf::from(path)
        .components()
        .any(|c| matches!(c, Component::Normal(_)));
    if !has_normal {
        return Err("volume write path must not resolve to '/'".into());
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub description: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct VolumeDef {
    pub read_only: bool,
    pub tmpfs: bool,
    pub writes: Vec<(String, String)>,
    pub exported: Option<ExportOptions>,
}

// l[impl volume.type]
#[derive(Debug, Clone)]
pub struct Volume {
    pub name: Option<ResourceName>,
    /// Stable identifier for anonymous dynamic volumes.
    /// Set at creation time inside an action closure from the operation context.
    /// Used to derive the podman volume name.
    pub anon_id: Option<String>,
    pub def: Holder<VolumeDef>,
    pub frozen: bool,
}

impl Volume {
    pub fn new(name: Option<ResourceName>) -> Self {
        Self {
            name,
            anon_id: None,
            def: Default::default(),
            frozen: false,
        }
    }

    pub fn new_anonymous(anon_id: String) -> Self {
        Self {
            name: None,
            anon_id: Some(anon_id),
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
            // l[impl volume.tmpfs]
            .with_fn(
                "tmpfs",
                |this: &mut Self| -> Result<Volume, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().tmpfs = true;
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
                    validate_volume_write_path(path)?;
                    this.def.lock().writes.push((path.into(), contents.into()));
                    Ok(this.clone())
                },
            )
            // l[impl volume.export]
            .with_fn(
                "export",
                |this: &mut Self| -> Result<Volume, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    if this.name.is_none() {
                        return Err("only named volumes can be exported".into());
                    }
                    this.def.lock().exported = Some(ExportOptions { description: None });
                    Ok(this.clone())
                },
            )
            // l[impl volume.export]
            .with_fn(
                "export",
                |this: &mut Self, options: Map| -> Result<Volume, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    if this.name.is_none() {
                        return Err("only named volumes can be exported".into());
                    }
                    let description = if let Some(desc) = options.get("description") {
                        Some(
                            desc.clone()
                                .into_string()
                                .map_err(|e| -> Box<EvalAltResult> {
                                    format!("export description must be a string: {e}").into()
                                })?,
                        )
                    } else {
                        None
                    };
                    this.def.lock().exported = Some(ExportOptions { description });
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
