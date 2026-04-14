use std::{
    collections::BTreeMap,
    path::{Component, PathBuf},
};

use rhai::{Array, Dynamic, EvalAltResult, TypeBuilder};

const FORBIDDEN_ENV_NAMES: &[&str] = &[
    "PATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
    "LD_PROFILE",
];

// l[impl container.env.validation]
fn validate_env_name(name: &str) -> Result<(), Box<EvalAltResult>> {
    if name.is_empty() {
        return Err("environment variable name must not be empty".into());
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(
            format!("environment variable name must not start with a digit: '{name}'").into(),
        );
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "environment variable name must contain only ASCII letters, digits, and underscores: '{name}'"
        )
        .into());
    }
    if FORBIDDEN_ENV_NAMES.contains(&name) {
        return Err(format!("environment variable '{name}' is forbidden").into());
    }
    Ok(())
}

fn validate_env_value(value: &str) -> Result<(), Box<EvalAltResult>> {
    if value.contains('\0') {
        return Err("environment variable value must not contain null bytes".into());
    }
    Ok(())
}

const FORBIDDEN_MOUNTPOINTS: &[&str] = &[
    "/", "/proc", "/sys", "/dev", "/etc", "/bin", "/sbin", "/lib", "/lib64", "/usr", "/boot",
    "/run",
];

// l[impl container.mount-volume.validation]
fn validate_mountpoint(path: &str) -> Result<(), Box<EvalAltResult>> {
    if path.contains('\0') {
        return Err("mountpoint must not contain null bytes".into());
    }
    if !path.starts_with('/') {
        return Err(format!("mountpoint must be an absolute path, got '{path}'").into());
    }

    // Canonicalise without touching the filesystem: resolve `.`, `..`, and
    // collapse repeated `/` separators.
    let mut canonical = PathBuf::from("/");
    for component in PathBuf::from(path).components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                canonical.pop();
            }
            Component::Normal(seg) => {
                canonical.push(seg);
            }
            Component::Prefix(_) => {}
        }
    }

    let canon_str = canonical.to_string_lossy();
    for &forbidden in FORBIDDEN_MOUNTPOINTS {
        if canon_str == forbidden {
            return Err(
                format!("mountpoint '{path}' resolves to forbidden path '{forbidden}'").into(),
            );
        }
    }

    Ok(())
}

// l[impl container.image]
fn validate_image_ref(image: &str) -> Result<(), Box<EvalAltResult>> {
    let (name_tag, digest) = match image.split_once('@') {
        Some((left, right)) => (left, Some(right)),
        None => (image, None),
    };

    if let Some(digest) = digest {
        let Some((algo, hex)) = digest.split_once(':') else {
            return Err(format!("invalid image digest (expected algorithm:hex): '{image}'").into());
        };
        if algo.is_empty()
            || !algo
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return Err(format!("invalid digest algorithm: '{algo}'").into());
        }
        if hex.len() < 32
            || !hex
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return Err(format!(
                "invalid digest hex (must be at least 32 lowercase hex characters): '{hex}'"
            )
            .into());
        }
    }

    let Some((registry, rest)) = name_tag.split_once('/') else {
        return Err(format!(
            "image must be fully qualified (registry/path[:tag][@digest]): '{image}'"
        )
        .into());
    };

    if !registry.contains('.') && !registry.contains(':') {
        return Err(format!(
            "image registry must be a hostname (contain '.' or ':'): '{registry}'"
        )
        .into());
    }

    let (path, tag) = if let Some(colon_pos) = rest.rfind(':') {
        let (p, t) = rest.split_at(colon_pos);
        (p, Some(&t[1..]))
    } else {
        (rest, None)
    };

    if tag.is_none() && digest.is_none() {
        return Err(format!(
            "image must have a tag or digest (registry/path:tag or registry/path@digest): '{image}'"
        )
        .into());
    }

    if let Some(tag) = tag
        && (tag.is_empty()
            || !tag
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
    {
        return Err(format!("invalid image tag: '{tag}'").into());
    }

    if path.is_empty() {
        return Err(format!("image path must not be empty: '{image}'").into());
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(format!("image path contains empty segment: '{image}'").into());
        }
        if !segment.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
        }) {
            return Err(
                format!("image path segment contains invalid characters: '{segment}'").into(),
            );
        }
    }

    Ok(())
}

/// Extract the registry from a validated image reference.
/// Returns the part before the first `/` (hostname with optional port).
pub fn image_registry(image: &str) -> Option<&str> {
    let name_tag = image.split_once('@').map_or(image, |(left, _)| left);
    name_tag.split_once('/').map(|(registry, _)| registry)
}

use super::{
    Freezable, Holder,
    enums::OnExit,
    resource::ResourceId,
    volume::{ExternalVolume, Volume},
};

// l[impl container.interface]
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
    pub(super) fn mixin<T: Clone + Freezable + 'static>(
        builder: &mut TypeBuilder<T>,
        ext: impl Fn(&mut T) -> Holder<Self> + Copy + 'static,
        _resource: impl Fn(&mut T) -> ResourceId + Copy + 'static,
    ) {
        // l[impl container.image]
        builder.with_fn(
            "image",
            move |this: &mut T, image: &str| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                validate_image_ref(image)?;
                ext(this).lock().image = Some(image.into());
                Ok(this.clone())
            },
        );

        // l[impl container.command]
        builder
            .with_fn(
                "command",
                move |this: &mut T, cmd: &str| -> Result<T, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    ext(this).lock().command = Some(vec![cmd.into()]);
                    Ok(this.clone())
                },
            )
            .with_fn(
                "command",
                move |this: &mut T, entrypoint: Array| -> Result<T, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    ext(this).lock().command = Some(
                        entrypoint
                            .into_iter()
                            .map(|v| v.into_string().unwrap_or_default())
                            .collect(),
                    );
                    Ok(this.clone())
                },
            );

        // l[impl container.arg]
        builder
            .with_fn(
                "arg",
                move |this: &mut T, arg: &str| -> Result<T, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    ext(this)
                        .lock()
                        .args
                        .get_or_insert_default()
                        .push(arg.into());
                    Ok(this.clone())
                },
            )
            .with_fn(
                "arg",
                move |this: &mut T, args: Array| -> Result<T, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let holder = ext(this);
                    let mut def = holder.lock();
                    let list = def.args.get_or_insert_default();
                    for v in args {
                        list.push(v.into_string().unwrap_or_default());
                    }
                    Ok(this.clone())
                },
            );

        // l[impl container.env]
        builder
            .with_fn(
                "env",
                move |this: &mut T, name: &str, value: &str| -> Result<T, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    validate_env_name(name)?;
                    validate_env_value(value)?;
                    let holder = ext(this);
                    let mut def = holder.lock();
                    if let Some(pos) = def.env.iter().position(|(k, _)| k == name) {
                        def.env[pos].1 = value.into();
                    } else {
                        def.env.push((name.into(), value.into()));
                    }
                    Ok(this.clone())
                },
            )
            .with_fn(
                "env",
                move |this: &mut T, vars: Array| -> Result<T, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let holder = ext(this);
                    let mut def = holder.lock();
                    for item in vars {
                        if let Some(map) = item.try_cast::<rhai::Map>() {
                            let name = map
                                .get("name")
                                .and_then(|v: &Dynamic| v.clone().into_string().ok())
                                .unwrap_or_default();
                            let value = map
                                .get("value")
                                .and_then(|v: &Dynamic| v.clone().into_string().ok())
                                .unwrap_or_default();
                            validate_env_name(&name)?;
                            validate_env_value(&value)?;
                            if let Some(pos) = def.env.iter().position(|(k, _)| k == &name) {
                                def.env[pos].1 = value;
                            } else {
                                def.env.push((name, value));
                            }
                        }
                    }
                    Ok(this.clone())
                },
            );

        // l[impl container.mount-volume]
        builder
            .with_fn(
                "mount",
                move |this: &mut T, path: &str, volume: Volume| -> Result<T, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    validate_mountpoint(path)?;
                    ext(this)
                        .lock()
                        .volume_mounts
                        .insert(path.into(), VolumeMount::Volume(volume));
                    Ok(this.clone())
                },
            )
            .with_fn(
                "mount",
                move |this: &mut T,
                      path: &str,
                      volume: ExternalVolume|
                      -> Result<T, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    validate_mountpoint(path)?;
                    ext(this)
                        .lock()
                        .volume_mounts
                        .insert(path.into(), VolumeMount::ExternalVolume(volume));
                    Ok(this.clone())
                },
            );

        // l[impl container.on-exit]
        builder.with_fn(
            "on_exit",
            move |this: &mut T, strategy: OnExit| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                ext(this).lock().on_exit = strategy;
                Ok(this.clone())
            },
        );
    }
}
