use std::{collections::BTreeMap, net::Ipv6Addr, path::Path, sync::Arc};

use ipnet::Ipv6Net;
use parking_lot::Mutex;
use snafu::{IntoError, ResultExt, Snafu};

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
    #[snafu(display("instance registry error: {source}"))]
    Registry {
        source: crate::runtime::registry::RegistryError,
        backtrace: snafu::Backtrace,
    },
}

// l[impl volume.write.validation]
/// Write a file into a volume using kernel-confined `openat2(RESOLVE_BENEATH)`.
pub(crate) async fn safe_volume_write(
    mountpoint: &Path,
    rel_path: &str,
    contents: &str,
) -> Result<(), ActuateError> {
    super::confined_write::write_async(mountpoint, rel_path, contents.as_bytes())
        .await
        .map_err(|e| match e {
            super::confined_write::ConfinedWriteError::Escape { path, .. } => {
                VolumePathEscapeSnafu { path }.build()
            }
            super::confined_write::ConfinedWriteError::Io { path, source, .. } => {
                VolumeWriteSnafu { path }.into_error(source)
            }
        })
}

// ---------------------------------------------------------------------------
// Actuator
// ---------------------------------------------------------------------------

pub struct Actuator {
    driver: Arc<System>,
    node_prefix: Ipv6Net,
    registry: Arc<dyn InstanceRegistry>,
    dns_servers: Vec<Ipv6Addr>,
    /// Images currently being pulled or that have exhausted retries.
    pulling: Arc<Mutex<std::collections::HashMap<String, PullState>>>,
}

impl Actuator {
    pub fn new(
        driver: Arc<System>,
        node_prefix: Ipv6Net,
        registry: Arc<dyn InstanceRegistry>,
        dns_servers: Vec<Ipv6Addr>,
    ) -> Self {
        Self {
            driver,
            node_prefix,
            registry,
            dns_servers,
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
                            &self.dns_servers,
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
                            &self.dns_servers,
                        );
                        podman_args(&spec)
                    },
                )
                .await
            }
            // r[impl actuate.volume.start]
            Resource::Volume(vol) => {
                let name = instance.display_name.clone();
                let (tmpfs, writes) = {
                    let def = vol.def.lock();
                    (def.tmpfs, def.writes.clone())
                };
                // r[impl actuate.volume.tmpfs]
                let just_created = if !self
                    .driver
                    .container
                    .volume_exists(&name)
                    .await
                    .context(ContainerSnafu)?
                {
                    self.driver
                        .container
                        .create_volume(&name, tmpfs)
                        .await
                        .context(ContainerSnafu)?;
                    true
                } else {
                    false
                };
                // For tmpfs volumes, always re-apply writes: contents do not
                // survive a host reboot, but the volume metadata may persist.
                // For regular volumes, only apply on first creation so that
                // container-written state is not overwritten.
                let needs_writes = just_created || tmpfs;
                if needs_writes && !writes.is_empty() {
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
                let mounts = match self.resolve_service_mounts(instance, &raw_mounts) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::error!(instance = %instance.display_name, error = %e, "registry lookup failed computing spec hash");
                        return None;
                    }
                };
                deployment_spec(
                    &def,
                    instance,
                    &std::collections::BTreeMap::new(),
                    &(net_name, net_prefix),
                    &mounts,
                    &self.dns_servers,
                )
            }
            Resource::Job(job) => {
                let def = job.def.lock();
                let raw_mounts = def.pod.lock().service_mounts.clone();
                let mounts = match self.resolve_service_mounts(instance, &raw_mounts) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::error!(instance = %instance.display_name, error = %e, "registry lookup failed computing spec hash");
                        return None;
                    }
                };
                job_spec(
                    &def,
                    instance,
                    &std::collections::BTreeMap::new(),
                    &(net_name, net_prefix),
                    &mounts,
                    &self.dns_servers,
                )
            }
            _ => return None,
        };

        Some(spec_hash(&spec))
    }
}
