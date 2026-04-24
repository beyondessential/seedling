use std::{
    collections::BTreeMap,
    path::{Component, PathBuf},
};

use rhai::{Array, Dynamic, EvalAltResult, TypeBuilder};
use seedling_protocol::env::EnvVar;

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

fn validate_memory_limit(limit: &str) -> Result<(), Box<EvalAltResult>> {
    let (num, suffix) = if limit.ends_with(|c: char| c.is_ascii_alphabetic()) {
        let (n, s) = limit.split_at(limit.len() - 1);
        (n, s)
    } else {
        return Err("memory limit must end with a unit suffix (k, m, or g)".into());
    };
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) || num.starts_with('0') {
        return Err(format!(
            "memory limit must be a positive integer followed by k, m, or g: '{limit}'"
        )
        .into());
    }
    match suffix.to_ascii_lowercase().as_str() {
        "k" | "m" | "g" => Ok(()),
        _ => Err(format!("memory limit suffix must be k, m, or g, got '{suffix}'").into()),
    }
}

const ALLOWED_CAPS: &[&str] = &[
    "AUDIT_CONTROL",
    "AUDIT_READ",
    "AUDIT_WRITE",
    "BLOCK_SUSPEND",
    "BPF",
    "CHECKPOINT_RESTORE",
    "CHOWN",
    "DAC_OVERRIDE",
    "DAC_READ_SEARCH",
    "FOWNER",
    "FSETID",
    "IPC_LOCK",
    "IPC_OWNER",
    "KILL",
    "LEASE",
    "LINUX_IMMUTABLE",
    "MAC_ADMIN",
    "MAC_OVERRIDE",
    "MKNOD",
    "NET_ADMIN",
    "NET_BIND_SERVICE",
    "NET_BROADCAST",
    "NET_RAW",
    "PERFMON",
    "SETFCAP",
    "SETGID",
    "SETPCAP",
    "SETUID",
    "SYS_ADMIN",
    "SYS_BOOT",
    "SYS_CHROOT",
    "SYS_MODULE",
    "SYS_NICE",
    "SYS_PACCT",
    "SYS_PTRACE",
    "SYS_RAWIO",
    "SYS_RESOURCE",
    "SYS_TIME",
    "SYS_TTY_CONFIG",
    "SYSLOG",
    "WAKE_ALARM",
];

fn validate_capability(cap: &str) -> Result<String, Box<EvalAltResult>> {
    let normalised = cap.to_ascii_uppercase();
    if ALLOWED_CAPS.contains(&normalised.as_str()) {
        Ok(normalised)
    } else {
        Err(format!("unknown Linux capability: '{cap}'").into())
    }
}

fn take_nonneg_secs(
    map: &mut rhai::Map,
    key: &str,
    default: u64,
) -> Result<u64, Box<EvalAltResult>> {
    let Some(value) = map.remove(key) else {
        return Ok(default);
    };
    let n = value.as_int().map_err(|t| -> Box<EvalAltResult> {
        format!("healthcheck `{key}` must be a non-negative integer number of seconds, got {t}")
            .into()
    })?;
    if n < 0 {
        return Err(format!("healthcheck `{key}` must not be negative, got {n}").into());
    }
    Ok(n as u64)
}

fn take_retries(map: &mut rhai::Map) -> Result<u32, Box<EvalAltResult>> {
    let Some(value) = map.remove("retries") else {
        return Ok(HEALTHCHECK_DEFAULT_RETRIES);
    };
    let n = value.as_int().map_err(|t| -> Box<EvalAltResult> {
        format!("healthcheck `retries` must be a positive integer, got {t}").into()
    })?;
    if n <= 0 {
        return Err(format!("healthcheck `retries` must be positive, got {n}").into());
    }
    if n > u32::MAX as i64 {
        return Err(format!("healthcheck `retries` exceeds maximum {}", u32::MAX).into());
    }
    Ok(n as u32)
}

fn take_on_failure(map: &mut rhai::Map) -> Result<HealthcheckOnFailure, Box<EvalAltResult>> {
    let Some(value) = map.remove("on_failure") else {
        return Ok(HealthcheckOnFailure::None);
    };
    let s = value.into_string().map_err(|t| -> Box<EvalAltResult> {
        format!("healthcheck `on_failure` must be a string, got {t}").into()
    })?;
    match s.as_str() {
        "none" => Ok(HealthcheckOnFailure::None),
        "kill" => Ok(HealthcheckOnFailure::Kill),
        "restart" => Ok(HealthcheckOnFailure::Restart),
        "stop" => Ok(HealthcheckOnFailure::Stop),
        other => Err(format!(
            "healthcheck `on_failure` must be one of \"none\", \"kill\", \"restart\", \"stop\"; got '{other}'"
        )
        .into()),
    }
}

fn take_command_cmd(map: &mut rhai::Map) -> Result<Vec<String>, Box<EvalAltResult>> {
    let value = map.remove("cmd").ok_or_else(|| -> Box<EvalAltResult> {
        "healthcheck of kind \"command\" requires a `cmd` field".into()
    })?;
    let cmd = if let Some(s) = value.clone().try_cast::<String>() {
        if s.is_empty() {
            return Err("healthcheck `cmd` must not be empty".into());
        }
        vec!["CMD-SHELL".to_string(), s]
    } else if let Some(arr) = value.try_cast::<rhai::Array>() {
        let parts: Vec<String> = arr
            .into_iter()
            .map(|v| v.into_string().unwrap_or_default())
            .collect();
        if parts.is_empty() || parts.iter().all(String::is_empty) {
            return Err("healthcheck `cmd` must not be empty".into());
        }
        parts
    } else {
        return Err("healthcheck `cmd` must be a string or array of strings".into());
    };
    Ok(cmd)
}

// l[impl container.healthcheck]
pub(super) fn parse_healthcheck(mut map: rhai::Map) -> Result<HealthcheckDef, Box<EvalAltResult>> {
    // l[impl container.healthcheck.kind]
    let kind_raw = map.remove("kind").ok_or_else(|| -> Box<EvalAltResult> {
        "healthcheck requires a `kind` field (\"command\")".into()
    })?;
    let kind_str = kind_raw.into_string().map_err(|t| -> Box<EvalAltResult> {
        format!("healthcheck `kind` must be a string, got {t}").into()
    })?;
    let kind = match kind_str.as_str() {
        // l[impl container.healthcheck.command]
        "command" => HealthcheckKind::Command {
            cmd: take_command_cmd(&mut map)?,
        },
        "http" | "tcp" | "grpc" => {
            return Err(format!(
                "healthcheck kind '{kind_str}' is reserved for future use and not yet supported"
            )
            .into());
        }
        other => {
            return Err(format!("healthcheck `kind` must be \"command\"; got '{other}'").into());
        }
    };

    let interval_secs = take_nonneg_secs(&mut map, "interval", HEALTHCHECK_DEFAULT_INTERVAL_SECS)?;
    let timeout_secs = take_nonneg_secs(&mut map, "timeout", HEALTHCHECK_DEFAULT_TIMEOUT_SECS)?;
    let start_period_secs = take_nonneg_secs(
        &mut map,
        "start_period",
        HEALTHCHECK_DEFAULT_START_PERIOD_SECS,
    )?;
    let retries = take_retries(&mut map)?;
    // l[impl container.healthcheck.on-failure]
    let on_failure = take_on_failure(&mut map)?;

    if !map.is_empty() {
        let extras: Vec<String> = map.keys().map(|k| k.to_string()).collect();
        return Err(format!(
            "healthcheck map contains unknown keys: {}",
            extras.join(", ")
        )
        .into());
    }

    Ok(HealthcheckDef {
        kind,
        interval_secs,
        timeout_secs,
        retries,
        start_period_secs,
        on_failure,
    })
}

// l[impl container.interface]
#[derive(Debug, Default, Clone)]
pub struct ContainerDef {
    pub image: Option<String>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub env: Vec<EnvVar>,
    pub volume_mounts: BTreeMap<PathBuf, VolumeMount>,
    pub on_exit: Option<OnExit>,
    pub memory: Option<String>,
    pub cpus: Option<f64>,
    pub extra_caps: Vec<String>,
    pub writable_rootfs: bool,
    pub pids_limit: Option<u32>,
    pub workdir: Option<String>,
    pub healthcheck: Option<HealthcheckDef>,
}

#[derive(Debug, Clone)]
pub struct HealthcheckDef {
    pub kind: HealthcheckKind,
    pub interval_secs: u64,
    pub timeout_secs: u64,
    pub retries: u32,
    pub start_period_secs: u64,
    pub on_failure: HealthcheckOnFailure,
}

#[derive(Debug, Clone)]
pub enum HealthcheckKind {
    Command { cmd: Vec<String> },
    // Reserved for future expansion: Http, Tcp, Grpc. Runtime currently
    // rejects any kind other than Command at BSL-evaluation time.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthcheckOnFailure {
    None,
    Kill,
    Restart,
    Stop,
}

impl HealthcheckOnFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Kill => "kill",
            Self::Restart => "restart",
            Self::Stop => "stop",
        }
    }
}

const HEALTHCHECK_DEFAULT_INTERVAL_SECS: u64 = 30;
const HEALTHCHECK_DEFAULT_TIMEOUT_SECS: u64 = 30;
const HEALTHCHECK_DEFAULT_RETRIES: u32 = 3;
const HEALTHCHECK_DEFAULT_START_PERIOD_SECS: u64 = 0;

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
        // l[impl bsl.builder]
        // Canonical builder method: takes `&mut T`, returns `this.clone()` so
        // calls chain. Every `with_fn` below this follows the same shape.
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
                    let var = EnvVar::new(name, value)
                        .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
                    let holder = ext(this);
                    let mut def = holder.lock();
                    if let Some(pos) = def.env.iter().position(|ev| ev.name == *var.name.as_str()) {
                        def.env[pos] = var;
                    } else {
                        def.env.push(var);
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
                            let var = EnvVar::new(&name, value)
                                .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
                            if let Some(pos) =
                                def.env.iter().position(|ev| ev.name == *var.name.as_str())
                            {
                                def.env[pos] = var;
                            } else {
                                def.env.push(var);
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
                ext(this).lock().on_exit = Some(strategy);
                Ok(this.clone())
            },
        );

        // l[impl container.memory]
        builder.with_fn(
            "memory",
            move |this: &mut T, limit: &str| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                validate_memory_limit(limit)?;
                ext(this).lock().memory = Some(limit.to_ascii_lowercase());
                Ok(this.clone())
            },
        );

        // l[impl container.cpus]
        builder.with_fn(
            "cpus",
            move |this: &mut T, limit: f64| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                if limit <= 0.0 || !limit.is_finite() {
                    return Err(format!("cpus limit must be a positive number, got {limit}").into());
                }
                ext(this).lock().cpus = Some(limit);
                Ok(this.clone())
            },
        );

        // l[impl container.cap-add]
        builder.with_fn(
            "cap_add",
            move |this: &mut T, cap: &str| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                let normalised = validate_capability(cap)?;
                let holder = ext(this);
                let mut def = holder.lock();
                if !def.extra_caps.contains(&normalised) {
                    def.extra_caps.push(normalised);
                }
                Ok(this.clone())
            },
        );

        // l[impl container.writable-rootfs]
        builder.with_fn(
            "writable_rootfs",
            move |this: &mut T| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                ext(this).lock().writable_rootfs = true;
                Ok(this.clone())
            },
        );

        // l[impl container.pids-limit]
        builder.with_fn(
            "pids_limit",
            move |this: &mut T, limit: i64| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                if limit <= 0 {
                    return Err(
                        format!("pids_limit must be a positive integer, got {limit}").into(),
                    );
                }
                ext(this).lock().pids_limit = Some(limit as u32);
                Ok(this.clone())
            },
        );

        // l[impl container.workdir]
        builder.with_fn(
            "workdir",
            move |this: &mut T, path: String| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                if !path.starts_with('/') {
                    return Err(format!("workdir must be an absolute path, got '{path}'").into());
                }
                ext(this).lock().workdir = Some(path);
                Ok(this.clone())
            },
        );

        builder.with_fn(
            "healthcheck",
            move |this: &mut T, config: rhai::Map| -> Result<T, Box<EvalAltResult>> {
                this.ensure_unfrozen()?;
                // l[impl container.healthcheck]
                let hc = parse_healthcheck(config)?;
                ext(this).lock().healthcheck = Some(hc);
                Ok(this.clone())
            },
        );
    }
}

#[cfg(test)]
mod tests;
