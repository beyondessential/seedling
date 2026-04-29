use std::path::{Component, PathBuf};

use rhai::{CustomType, EvalAltResult, Map, TypeBuilder};

use crate::runtime::barrier::runtime::is_in_action_closure;

use super::{Freezable, Holder, export::ExportOptions, resource::ResourceName};

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

#[derive(Debug, Default, Clone)]
pub struct VolumeDef {
    pub read_only: bool,
    pub tmpfs: bool,
    pub writes: Vec<(String, String)>,
    pub exported: Option<ExportOptions>,
    // l[impl bsl.resource.description]
    pub description: Option<String>,
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
    // l[impl app.resources.context.immutable]
    fn is_frozen(&self) -> bool {
        // The action-context check catches static volumes captured from outer scope,
        // which keep `frozen=false` because only the in-action re-fetch stamps the flag.
        self.frozen || (self.name.is_some() && is_in_action_closure())
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
            // l[impl volume.exported]
            .with_fn(
                "exported",
                |this: &mut Self| -> Result<Volume, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    if this.name.is_none() {
                        return Err("only named volumes can be exported".into());
                    }
                    this.def.lock().exported = Some(ExportOptions { description: None });
                    Ok(this.clone())
                },
            )
            // l[impl volume.exported]
            .with_fn(
                "exported",
                |this: &mut Self, options: Map| -> Result<Volume, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    if this.name.is_none() {
                        return Err("only named volumes can be exported".into());
                    }
                    this.def.lock().exported = Some(ExportOptions::from_rhai_map(options)?);
                    Ok(this.clone())
                },
            )
            // l[impl bsl.resource.description]
            .with_fn(
                "description",
                |this: &mut Self, desc: &str| -> Result<Volume, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().description = Some(desc.to_owned());
                    Ok(this.clone())
                },
            );
    }
}

/// An operation-scoped volume binding injected by the runtime before an action closure runs.
#[derive(Debug, Clone)]
pub struct OperationVolumeBinding {
    pub host_path: std::path::PathBuf,
    pub read_only: bool,
}

/// One logical volume binding the runtime wants to hand to an action closure.
///
/// Used with [`build_operation_volume_params`] to turn a map of logical keys
/// (`"source"`, `"destination"`, `"output"`, ...) into a pair of:
///
///  - an [`OperationVolumeBinding`] map keyed by a generated, collision-free
///    name;
///  - a `param` object map populated with `<key>_volume` (and optionally
///    `<key>_filename`) entries that the action closure uses to look up the
///    binding via `app.external_volume(param["<key>_volume"])`.
// r[impl operation.volume-param]
#[derive(Debug, Clone)]
pub struct VolumeParamSpec {
    pub host_path: std::path::PathBuf,
    pub read_only: bool,
    /// Optional filename the action must use within the volume. When set, the
    /// runtime also injects `<key>_filename` into the action's param map.
    // r[impl operation.volume-param.filename]
    pub filename: Option<String>,
}

/// Build operation-scoped volume bindings and the matching `_volume` /
/// `_filename` params for a set of logical keys.
///
/// Each entry `(logical_key, spec)` produces:
///  - a generated name of the form `seedling-op-<short-operation-id>-<key>`,
///  - an [`OperationVolumeBinding`] entry under that generated name,
///  - `param["<logical_key>_volume"]` = the generated name,
///  - `param["<logical_key>_filename"]` = `spec.filename` when present.
///
/// The generated name is derived deterministically from the operation id, so
/// two invocations of the helper with the same operation id and keys produce
/// the same names (useful for replay safety and debuggability) while still
/// being unique across operations.
// r[impl operation.volume-param]
pub fn build_operation_volume_params<I, K>(
    operation_id: &str,
    bindings: I,
) -> (
    std::collections::HashMap<String, OperationVolumeBinding>,
    serde_json::Map<String, serde_json::Value>,
)
where
    I: IntoIterator<Item = (K, VolumeParamSpec)>,
    K: AsRef<str>,
{
    let short = &operation_id[..8.min(operation_id.len())];
    let mut out_bindings = std::collections::HashMap::new();
    let mut out_params = serde_json::Map::new();
    for (key, spec) in bindings {
        let key = key.as_ref();
        let volume_name = format!("seedling-op-{short}-{key}");
        out_bindings.insert(
            volume_name.clone(),
            OperationVolumeBinding {
                host_path: spec.host_path,
                read_only: spec.read_only,
            },
        );
        out_params.insert(
            format!("{key}_volume"),
            serde_json::Value::String(volume_name),
        );
        if let Some(filename) = spec.filename {
            // r[impl operation.volume-param.filename]
            out_params.insert(
                format!("{key}_filename"),
                serde_json::Value::String(filename),
            );
        }
    }
    (out_bindings, out_params)
}

// l[impl volume.external]
#[derive(Debug, Default, Clone)]
pub struct ExternalVolumeDef {
    // l[impl bsl.resource.description]
    pub description: Option<String>,
}

// l[impl volume.external]
#[derive(Debug, Clone)]
pub struct ExternalVolume {
    pub name: ResourceName,
    /// Set when this external volume resolves to an operation-scoped binding rather
    /// than a static external volume mapping.
    // l[impl volume.external.dynamic]
    pub operation_binding: Option<OperationVolumeBinding>,
    pub def: Holder<ExternalVolumeDef>,
}

impl CustomType for ExternalVolume {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("ExternalVolume")
            // l[impl bsl.resource.description]
            .with_fn("description", |this: &mut Self, desc: &str| -> Self {
                this.def.lock().description = Some(desc.to_owned());
                this.clone()
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // r[impl operation.volume-param] r[impl operation.volume-param.filename]
    #[test]
    fn build_operation_volume_params_injects_volume_and_filename() {
        let (bindings, params) = build_operation_volume_params(
            "abcd1234-5678-4abc-9def-000000000001",
            [
                (
                    "source",
                    VolumeParamSpec {
                        host_path: "/tmp/a".into(),
                        read_only: true,
                        filename: None,
                    },
                ),
                (
                    "output",
                    VolumeParamSpec {
                        host_path: "/tmp/b".into(),
                        read_only: false,
                        filename: Some("snapshots.json".into()),
                    },
                ),
            ],
        );

        let source_name = params["source_volume"].as_str().unwrap();
        let output_name = params["output_volume"].as_str().unwrap();
        assert_ne!(source_name, output_name);
        assert!(source_name.starts_with("seedling-op-abcd1234-"));
        assert!(source_name.ends_with("-source"));
        assert!(output_name.ends_with("-output"));

        assert_eq!(
            params["output_filename"].as_str().unwrap(),
            "snapshots.json"
        );
        assert!(params.get("source_filename").is_none());

        let src = bindings.get(source_name).unwrap();
        assert_eq!(src.host_path, std::path::PathBuf::from("/tmp/a"));
        assert!(src.read_only);
        let out = bindings.get(output_name).unwrap();
        assert_eq!(out.host_path, std::path::PathBuf::from("/tmp/b"));
        assert!(!out.read_only);
    }

    // r[impl operation.volume-param]
    #[test]
    fn build_operation_volume_params_unique_per_operation() {
        let (_, params_a) = build_operation_volume_params(
            "aaaaaaaa-1111-4000-8000-000000000001",
            [(
                "source",
                VolumeParamSpec {
                    host_path: "/x".into(),
                    read_only: true,
                    filename: None,
                },
            )],
        );
        let (_, params_b) = build_operation_volume_params(
            "bbbbbbbb-2222-4000-8000-000000000002",
            [(
                "source",
                VolumeParamSpec {
                    host_path: "/x".into(),
                    read_only: true,
                    filename: None,
                },
            )],
        );
        assert_ne!(params_a["source_volume"], params_b["source_volume"]);
    }
}
