use std::{collections::BTreeMap, sync::Arc};

use ipnet::Ipv6Net;
use parking_lot::Mutex;
use snafu::Snafu;

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
    },
    #[snafu(display("process manager: {source}"))]
    Process {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("proxy: {source}"))]
    Proxy {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("data plane: {source}"))]
    DataPlane {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("image {reference} not found and pull failed"))]
    ImageUnavailable { reference: String },
    #[snafu(display("resource kind {kind:?} is not supported by this actuator"))]
    UnsupportedKind { kind: ResourceKind },
    #[snafu(display("volume write {path:?}: {source}"))]
    VolumeWrite {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
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
                    .map_err(|e| ActuateError::Container { source: e })?
                {
                    self.driver
                        .container
                        .create_volume(&name)
                        .await
                        .map_err(|e| ActuateError::Container { source: e })?;
                }
                let writes = vol.def.lock().writes.clone();
                if !writes.is_empty() {
                    let mountpoint = self
                        .driver
                        .container
                        .volume_mountpoint(&name)
                        .await
                        .map_err(|e| ActuateError::Container { source: e })?;
                    for (path, contents) in &writes {
                        let dest = mountpoint.join(path.trim_start_matches('/'));
                        if let Some(parent) = dest.parent() {
                            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                                ActuateError::VolumeWrite {
                                    path: dest.clone(),
                                    source: e,
                                }
                            })?;
                        }
                        tokio::fs::write(&dest, contents).await.map_err(|e| {
                            ActuateError::VolumeWrite {
                                path: dest.clone(),
                                source: e,
                            }
                        })?;
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
                    .map_err(|e| ActuateError::Container { source: e })?
                {
                    self.driver
                        .container
                        .remove_volume(&name)
                        .await
                        .map_err(|e| ActuateError::Container { source: e })?;
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
