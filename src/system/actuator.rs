use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, path::Path, sync::Arc};

use ipnet::Ipv6Net;
use parking_lot::Mutex;
use snafu::{ResultExt, Snafu};

use crate::{
    defs::resource::{Resource, ResourceKind},
    runtime::{identity::ResourceInstance, registry::InstanceRegistry},
    system::{
        System,
        translate::{
            container::{deployment_spec, job_spec, podman_args, spec_hash},
            proxy::pod_network_prefix,
        },
    },
};

mod pod;
mod pull;

use pod::{collect_container_volumes, map_on_exit};
use pull::PullState;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Actuation-time error. The backend variant is intentionally erased:
/// callers see `ActuateError::Container` but cannot match on internal types.
#[derive(Debug, Snafu)]
pub enum ActuateError {
    #[snafu(display("container backend: {source}"))]
    Container {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("process manager: {source}"))]
    Process {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("proxy: {source}"))]
    Proxy {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("data plane: {source}"))]
    DataPlane {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("image {reference} not found and pull failed"))]
    ImageUnavailable {
        reference: String,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("resource kind {kind:?} is not supported by this actuator"))]
    UnsupportedKind {
        kind: ResourceKind,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("volume write {path:?}: {source}"))]
    VolumeWrite {
        path: std::path::PathBuf,
        source: std::io::Error,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("volume write path escapes volume root: {path:?}"))]
    VolumePathEscape {
        path: std::path::PathBuf,
        backtrace: snafu::Backtrace,
    },
}

// l[impl volume.write.validation]
/// Write a file into a volume, verifying the resolved path stays within
/// `mountpoint` and setting permissions to 0640.
pub(crate) async fn safe_volume_write(
    mountpoint: &Path,
    rel_path: &str,
    contents: &str,
) -> Result<(), ActuateError> {
    let dest = mountpoint.join(rel_path.trim_start_matches('/'));

    // Canonicalise the destination *logically* (the parent dirs may not exist
    // yet, so we canonicalise the mountpoint from disk and resolve the
    // remainder manually).
    let canon_mount = tokio::fs::canonicalize(mountpoint)
        .await
        .context(VolumeWriteSnafu {
            path: mountpoint.to_path_buf(),
        })?;

    // Build logical canonical form of dest by starting from canon_mount and
    // walking the relative portion component-by-component.
    let rel = rel_path.trim_start_matches('/');
    let mut canon_dest = canon_mount.clone();
    for component in Path::new(rel).components() {
        match component {
            std::path::Component::Normal(seg) => canon_dest.push(seg),
            std::path::Component::ParentDir => {
                canon_dest.pop();
            }
            std::path::Component::CurDir | std::path::Component::RootDir => {}
            std::path::Component::Prefix(_) => {}
        }
    }

    if !canon_dest.starts_with(&canon_mount) || canon_dest == canon_mount {
        return Err(VolumePathEscapeSnafu { path: dest }.build());
    }

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context(VolumeWriteSnafu { path: dest.clone() })?;
    }
    tokio::fs::write(&dest, contents)
        .await
        .context(VolumeWriteSnafu { path: dest.clone() })?;
    tokio::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o640))
        .await
        .context(VolumeWriteSnafu { path: dest })?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Actuator
// ---------------------------------------------------------------------------

pub struct Actuator {
    driver: Arc<System>,
    node_prefix: Ipv6Net,
    registry: Arc<dyn InstanceRegistry>,
    /// Images currently being pulled or that have exhausted retries.
    pulling: Arc<Mutex<std::collections::HashMap<String, PullState>>>,
}

impl Actuator {
    pub fn new(
        driver: Arc<System>,
        node_prefix: Ipv6Net,
        registry: Arc<dyn InstanceRegistry>,
    ) -> Self {
        Self {
            driver,
            node_prefix,
            registry,
            pulling: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    // r[impl actuate.deployment.start]
    /// Ensure all primitives for this instance exist and are running.
    #[tracing::instrument(skip_all, fields(instance = %instance.display_name))]
    pub async fn start(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Result<Option<String>, ActuateError> {
        match resource {
            Resource::Deployment(dep) => {
                let (image, raw_mounts, restart, vols) = {
                    let def = dep.def.lock();
                    let pod = def.pod.lock();
                    let container = pod.container.lock();
                    let image = container.image.clone().unwrap_or_default();
                    let raw_mounts = pod.service_mounts.clone();
                    let restart = map_on_exit(container.on_exit);
                    drop(container);
                    let vols = collect_container_volumes(&pod, instance);
                    (image, raw_mounts, restart, vols)
                };
                self.start_pod_instance(
                    instance,
                    &image,
                    &raw_mounts,
                    restart,
                    &vols,
                    |net_name, net_prefix, mounts| {
                        let guard = dep.def.lock();
                        let spec = deployment_spec(
                            &guard,
                            instance,
                            &BTreeMap::new(),
                            &(net_name, net_prefix),
                            mounts,
                        );
                        podman_args(&spec)
                    },
                )
                .await
            }
            Resource::Job(job) => {
                let (image, raw_mounts, restart, vols) = {
                    let def = job.def.lock();
                    let pod = def.pod.lock();
                    let container = pod.container.lock();
                    let image = container.image.clone().unwrap_or_default();
                    let raw_mounts = pod.service_mounts.clone();
                    let restart = map_on_exit(container.on_exit);
                    drop(container);
                    let vols = collect_container_volumes(&pod, instance);
                    (image, raw_mounts, restart, vols)
                };
                self.start_pod_instance(
                    instance,
                    &image,
                    &raw_mounts,
                    restart,
                    &vols,
                    |net_name, net_prefix, mounts| {
                        let guard = job.def.lock();
                        let spec = job_spec(
                            &guard,
                            instance,
                            &BTreeMap::new(),
                            &(net_name, net_prefix),
                            mounts,
                        );
                        podman_args(&spec)
                    },
                )
                .await
            }
            // r[impl actuate.volume.start]
            Resource::Volume(vol) => {
                let name = instance.display_name.clone();
                if !self
                    .driver
                    .container
                    .volume_exists(&name)
                    .await
                    .context(ContainerSnafu)?
                {
                    self.driver
                        .container
                        .create_volume(&name)
                        .await
                        .context(ContainerSnafu)?;
                }
                let writes = vol.def.lock().writes.clone();
                if !writes.is_empty() {
                    let mountpoint = self
                        .driver
                        .container
                        .volume_mountpoint(&name)
                        .await
                        .context(ContainerSnafu)?;
                    for (path, contents) in &writes {
                        safe_volume_write(&mountpoint, path, contents).await?;
                    }
                }
                Ok(None)
            }
            Resource::ExternalVolume(_) | Resource::ExternalService(_) => Ok(None),
            Resource::Service(_) | Resource::HttpService(_) => Ok(None),
            Resource::Ingress(_) => Ok(None),
        }
    }

    // r[impl actuate.deployment.stop]
    // r[impl actuate.volume.stop]
    /// Stop and remove all primitives for this instance.
    #[tracing::instrument(skip_all, fields(instance = %instance.display_name))]
    pub async fn stop(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Result<(), ActuateError> {
        match resource {
            Resource::Deployment(dep) => {
                let anon_names: Vec<String> = {
                    let def = dep.def.lock();
                    let pod = def.pod.lock();
                    collect_container_volumes(&pod, instance)
                        .into_iter()
                        .filter(|v| v.remove_on_stop)
                        .map(|v| v.name)
                        .collect()
                };
                self.stop_pod_instance(instance, &anon_names).await
            }
            Resource::Job(job) => {
                let anon_names: Vec<String> = {
                    let def = job.def.lock();
                    let pod = def.pod.lock();
                    collect_container_volumes(&pod, instance)
                        .into_iter()
                        .filter(|v| v.remove_on_stop)
                        .map(|v| v.name)
                        .collect()
                };
                self.stop_pod_instance(instance, &anon_names).await
            }
            Resource::Volume(_) => {
                let name = instance.display_name.clone();
                if self
                    .driver
                    .container
                    .volume_exists(&name)
                    .await
                    .context(ContainerSnafu)?
                {
                    self.driver
                        .container
                        .remove_volume(&name)
                        .await
                        .context(ContainerSnafu)?;
                }
                Ok(())
            }
            Resource::ExternalVolume(_) | Resource::ExternalService(_) => Ok(()),
            Resource::Service(_) | Resource::HttpService(_) => Ok(()),
            Resource::Ingress(_) => Ok(()),
        }
    }

    /// In-place update (e.g. rolling a container to a new image or config).
    pub async fn update(
        &self,
        _instance: &ResourceInstance,
        _old: &Resource,
        _new: &Resource,
    ) -> Result<(), ActuateError> {
        todo!("actuator update: not yet implemented")
    }

    /// Compute the spec hash that would be used if this instance were started
    /// right now. Returns `None` for resource kinds that have no container spec.
    ///
    /// This mirrors the hash stamped on the container at start time and lets the
    /// observer detect config drift for any field — not just the image.
    pub fn desired_spec_hash(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Option<String> {
        let net_name = format!("seedling-{}", instance.display_name);
        let net_prefix = pod_network_prefix(&self.node_prefix, instance);

        let spec = match resource {
            Resource::Deployment(dep) => {
                let def = dep.def.lock();
                let raw_mounts = def.pod.lock().service_mounts.clone();
                let mounts = self.resolve_service_mounts(instance, &raw_mounts);
                deployment_spec(
                    &def,
                    instance,
                    &std::collections::BTreeMap::new(),
                    &(net_name, net_prefix),
                    &mounts,
                )
            }
            Resource::Job(job) => {
                let def = job.def.lock();
                let raw_mounts = def.pod.lock().service_mounts.clone();
                let mounts = self.resolve_service_mounts(instance, &raw_mounts);
                job_spec(
                    &def,
                    instance,
                    &std::collections::BTreeMap::new(),
                    &(net_name, net_prefix),
                    &mounts,
                )
            }
            _ => return None,
        };

        Some(spec_hash(&spec))
    }
}
