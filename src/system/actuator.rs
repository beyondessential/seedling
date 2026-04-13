use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use ipnet::Ipv6Net;
use parking_lot::Mutex;
use snafu::Snafu;

use crate::{
    defs::{
        container::VolumeMount,
        enums::OnExit,
        resource::{Resource, ResourceKind},
        service::ServicePort,
        volume::VolumeDef,
    },
    runtime::{identity::ResourceInstance, registry::InstanceRegistry},
    system::{
        System,
        translate::{
            container::{anon_vol_name, deployment_spec, job_spec, podman_args, spec_hash},
            proxy::{instance_ipv6, pod_network_prefix},
        },
        types::{ActiveState, TransientRestart, TransientUnitSpec},
    },
};

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
// Naming helpers
// ---------------------------------------------------------------------------

fn pod_network_name(instance: &ResourceInstance) -> String {
    format!("seedling-{}", instance.display_name)
}

fn unit_name(instance: &ResourceInstance) -> String {
    format!("seedling-{}.service", instance.display_name)
}

fn map_on_exit(on_exit: OnExit) -> TransientRestart {
    match on_exit {
        OnExit::Restart => TransientRestart::Always,
        OnExit::Terminate => TransientRestart::No,
        OnExit::RestartOnFailure => TransientRestart::OnFailure,
    }
}

/// Collect the anonymous volume (name, def) pairs for a pod instance.
/// These are volumes declared in the BSL without a name that need to be
/// created and seeded before the container starts.
struct ContainerVolume {
    /// Podman volume name.
    name: String,
    def: VolumeDef,
    /// Whether to remove this volume when the container stops.
    /// True for anonymous volumes (ephemeral per-container), false for named
    /// volumes (static reconciler-managed or dynamic user-named).
    remove_on_stop: bool,
}

/// Collect all volumes declared in the pod's container mounts.
///
/// Anonymous volumes (`v.name.is_none()`) get a deterministic podman name
/// derived from the instance and mount path; they are marked for removal when
/// the container stops. Named volumes use the app-prefixed BSL name and
/// persist after the container stops.
fn collect_container_volumes(
    pod_def: &crate::defs::pod::PodDef,
    instance: &ResourceInstance,
) -> Vec<ContainerVolume> {
    pod_def
        .container
        .lock()
        .volume_mounts
        .iter()
        .filter_map(|(path, vm)| match vm {
            VolumeMount::Volume(v) => {
                let (name, remove_on_stop) = match &v.name {
                    None => {
                        let vol_name = match &v.anon_id {
                            Some(id) => id.clone(),
                            None => anon_vol_name(instance, &path.to_string_lossy()),
                        };
                        (vol_name, true)
                    }
                    Some(n) => (format!("{}-{}", instance.app, n.as_str()), false),
                };
                Some(ContainerVolume {
                    name,
                    def: v.def.lock().clone(),
                    remove_on_stop,
                })
            }
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Actuator
// ---------------------------------------------------------------------------

pub struct Actuator {
    driver: Arc<System>,
    node_prefix: Ipv6Net,
    registry: Arc<dyn InstanceRegistry>,
    /// Images currently being pulled in background tasks.
    pulling: Arc<Mutex<HashSet<String>>>,
}

async fn ensure_volumes(driver: &System, volumes: &[ContainerVolume]) -> Result<(), ActuateError> {
    for vol in volumes {
        let just_created = if !driver
            .container
            .volume_exists(&vol.name)
            .await
            .map_err(|e| ActuateError::Container { source: e })?
        {
            driver
                .container
                .create_volume(&vol.name)
                .await
                .map_err(|e| ActuateError::Container { source: e })?;
            true
        } else {
            false
        };
        if just_created && !vol.def.writes.is_empty() {
            let mountpoint = driver
                .container
                .volume_mountpoint(&vol.name)
                .await
                .map_err(|e| ActuateError::Container { source: e })?;
            for (path, contents) in &vol.def.writes {
                let dest = mountpoint.join(path.trim_start_matches('/'));
                if let Some(parent) = dest.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        ActuateError::VolumeWrite {
                            path: dest.clone(),
                            source: e,
                        }
                    })?;
                }
                tokio::fs::write(&dest, contents)
                    .await
                    .map_err(|e| ActuateError::VolumeWrite {
                        path: dest.clone(),
                        source: e,
                    })?;
            }
        }
    }
    Ok(())
}

/// Returns the bridge name if a new network was created, `None` if it already existed.
async fn ensure_network(
    driver: &System,
    net_name: &str,
    net_prefix: Ipv6Net,
) -> Result<Option<String>, ActuateError> {
    if !driver
        .container
        .network_exists(net_name)
        .await
        .map_err(|e| ActuateError::Container { source: e })?
    {
        Ok(Some(
            driver
                .container
                .create_network(net_name, net_prefix, None)
                .await
                .map_err(|e| ActuateError::Container { source: e })?,
        ))
    } else {
        Ok(None)
    }
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
            pulling: Arc::new(Mutex::new(HashSet::new())),
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

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    /// Resolves `service_mounts` declared on a pod to `(mount_port, service_ip,
    /// service_port)` tuples, computing each service's stable IPv6 address from
    /// the node prefix and the service's persisted instance identity.
    fn resolve_service_mounts(
        &self,
        instance: &ResourceInstance,
        mounts: &[ServicePort],
    ) -> Vec<(u16, std::net::Ipv6Addr, u16)> {
        mounts
            .iter()
            .map(|sp| {
                let svc_instance = self.registry.get_or_create_singleton(
                    &instance.app,
                    ResourceKind::Service,
                    Some(sp.service.name.as_str()),
                );
                let service_ip = instance_ipv6(&self.node_prefix, &svc_instance);
                (sp.port, service_ip, sp.port)
            })
            .collect()
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

    // r[impl actuate.deployment.anon-volume.start]
    async fn start_pod_instance(
        &self,
        instance: &ResourceInstance,
        image: &str,
        raw_mounts: &[ServicePort],
        restart: TransientRestart,
        volumes: &[ContainerVolume],
        build_argv: impl FnOnce(String, Ipv6Net, &[(u16, std::net::Ipv6Addr, u16)]) -> Vec<String>,
    ) -> Result<Option<String>, ActuateError> {
        // r[impl reconciliation.liveness]
        // Check image availability; spawn background pull if missing.
        if !self
            .driver
            .container
            .image_exists(image)
            .await
            .map_err(|e| ActuateError::Container { source: e })?
        {
            let mut pulling = self.pulling.lock();
            if !pulling.contains(image) {
                pulling.insert(image.to_owned());
                let driver = Arc::clone(&self.driver);
                let image_owned = image.to_owned();
                let pulling_set = Arc::clone(&self.pulling);
                tokio::spawn(async move {
                    let result = driver.container.pull_image(&image_owned).await;
                    pulling_set.lock().remove(&image_owned);
                    if let Err(e) = result {
                        tracing::warn!(image = %image_owned, error = %e, "background image pull failed");
                    }
                });
            }
            return Err(ActuateError::ImageUnavailable {
                reference: image.to_owned(),
            });
        }

        // Set up volumes and network concurrently — neither depends on the other.
        let net_name = pod_network_name(instance);
        let net_prefix = pod_network_prefix(&self.node_prefix, instance);

        let ((), bridge_name) = tokio::try_join!(
            ensure_volumes(&self.driver, volumes),
            ensure_network(&self.driver, &net_name, net_prefix),
        )?;

        // Handle any pre-existing unit with this name.
        let unit = unit_name(instance);
        if let Some(state) = self
            .driver
            .process
            .unit_state(&unit)
            .await
            .map_err(|e| ActuateError::Process { source: e })?
        {
            match state.active {
                ActiveState::Active | ActiveState::Activating => return Ok(bridge_name),
                _ => {
                    self.driver
                        .process
                        .reset_failed_unit(&unit)
                        .await
                        .map_err(|e| ActuateError::Process { source: e })?;
                }
            }
        }

        // Resolve service mounts and build the argv.
        let mounts = self.resolve_service_mounts(instance, raw_mounts);
        let argv = build_argv(net_name, net_prefix, &mounts);

        self.driver
            .process
            .start_transient(TransientUnitSpec {
                name: unit,
                description: format!("seedling container {}", instance.display_name),
                exec_start: argv,
                restart,
            })
            .await
            .map_err(|e| ActuateError::Process { source: e })?;

        Ok(bridge_name)
    }

    // r[impl reconciliation.liveness]
    async fn stop_pod_instance(
        &self,
        instance: &ResourceInstance,
        anon_volume_names: &[String],
    ) -> Result<(), ActuateError> {
        let unit = unit_name(instance);

        // Check unit state and act accordingly. Active or deactivating units
        // get a stop signal but we return immediately — the next reconciler
        // tick will re-observe and continue cleanup once the unit is gone.
        if let Some(state) = self
            .driver
            .process
            .unit_state(&unit)
            .await
            .map_err(|e| ActuateError::Process { source: e })?
        {
            match state.active {
                ActiveState::Active | ActiveState::Activating | ActiveState::Deactivating => {
                    self.driver
                        .process
                        .stop_unit(&unit)
                        .await
                        .map_err(|e| ActuateError::Process { source: e })?;
                    return Ok(());
                }
                ActiveState::Inactive | ActiveState::Failed => {
                    self.driver
                        .process
                        .reset_failed_unit(&unit)
                        .await
                        .map_err(|e| ActuateError::Process { source: e })?;
                }
            }
        }

        // Unit is gone — clean up remaining resources.

        self.driver
            .container
            .remove_container(&instance.display_name, true)
            .await
            .map_err(|e| ActuateError::Container { source: e })?;

        let net_name = pod_network_name(instance);
        if self
            .driver
            .container
            .network_exists(&net_name)
            .await
            .map_err(|e| ActuateError::Container { source: e })?
        {
            self.driver
                .container
                .remove_network(&net_name)
                .await
                .map_err(|e| ActuateError::Container { source: e })?;
        }

        for vol_name in anon_volume_names {
            if self
                .driver
                .container
                .volume_exists(vol_name)
                .await
                .map_err(|e| ActuateError::Container { source: e })?
            {
                self.driver
                    .container
                    .remove_volume(vol_name)
                    .await
                    .map_err(|e| ActuateError::Container { source: e })?;
            }
        }

        Ok(())
    }
}
