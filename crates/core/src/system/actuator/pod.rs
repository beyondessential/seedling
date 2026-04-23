use ipnet::Ipv6Net;
use snafu::{IntoError, ResultExt};

use super::safe_volume_write;

use crate::{
    defs::{
        container::VolumeMount, enums::OnExit, resource::ResourceKind, service::ServicePort,
        volume::VolumeDef,
    },
    runtime::{
        identity::{ResourceInstance, VolumeName},
        registry::RegistryError,
    },
    system::{
        System,
        translate::proxy::{instance_ipv6, pod_network_prefix},
        types::{ActiveState, TransientRestart, TransientUnitSpec},
    },
};

use super::{ActuateError, Actuator, ContainerSnafu, ProcessSnafu, RegistrySnafu};

fn pod_network_name(instance: &ResourceInstance) -> String {
    format!("seedling-{}", instance.display_name)
}

fn unit_name(instance: &ResourceInstance) -> String {
    format!("seedling-{}.service", instance.display_name)
}

pub(super) fn map_on_exit(on_exit: OnExit) -> TransientRestart {
    match on_exit {
        OnExit::Restart => TransientRestart::Always,
        OnExit::Terminate => TransientRestart::No,
        OnExit::RestartOnFailure => TransientRestart::OnFailure,
    }
}

/// Collect the anonymous volume (name, def) pairs for a pod instance.
/// These are volumes declared in the BSL without a name that need to be
/// created and seeded before the container starts.
pub(crate) struct ContainerVolume {
    /// Podman volume name. For named bind-mounted volumes this equals
    /// `bind_name.as_str()`; for anonymous / podman-managed volumes it is the
    /// raw anon id or other podman-level name.
    pub(super) name: String,
    /// The canonical `VolumeName` when this is a named app volume bind-mounted
    /// through the `VolumeStore`. `None` for anonymous / podman-managed
    /// volumes.
    pub(super) bind_name: Option<VolumeName>,
    pub(super) def: VolumeDef,
    /// Whether to remove this volume when the container stops.
    /// True for anonymous volumes (ephemeral per-container), false for named
    /// volumes (static reconciler-managed or dynamic user-named).
    pub(super) remove_on_stop: bool,
    /// Host path for bind-mounted volumes (named non-tmpfs volumes managed
    /// by the VolumeStore). None for podman-managed volumes.
    pub(super) host_path: Option<std::path::PathBuf>,
}

/// Collect all volumes declared in the pod's container mounts.
///
/// Anonymous volumes (`v.name.is_none()`) get a deterministic podman name
/// derived from the instance and mount path; they are marked for removal when
/// the container stops. Named volumes use the app-prefixed BSL name and
/// persist after the container stops.
pub(super) fn collect_container_volumes(
    pod_def: &crate::defs::pod::PodDef,
    instance: &ResourceInstance,
    volumes_dir: Option<&std::path::Path>,
) -> Vec<ContainerVolume> {
    pod_def
        .container
        .lock()
        .volume_mounts
        .values()
        .filter_map(|vm| match vm {
            VolumeMount::Volume(v) => {
                let (name, bind_name, remove_on_stop, host_path) = match &v.name {
                    None => {
                        let vol_name = v
                            .anon_id
                            .clone()
                            .expect("anonymous volume must have an anon_id");
                        (vol_name, None, true, None)
                    }
                    Some(n) => {
                        let vol_name = VolumeName::for_app(instance.app.as_str(), n.as_str());
                        let tmpfs = v.def.lock().tmpfs;
                        let host_path = if !tmpfs {
                            volumes_dir.map(|dir| dir.join(vol_name.as_str()))
                        } else {
                            None
                        };
                        (
                            vol_name.as_str().to_owned(),
                            Some(vol_name),
                            false,
                            host_path,
                        )
                    }
                };
                Some(ContainerVolume {
                    name,
                    bind_name,
                    def: v.def.lock().clone(),
                    remove_on_stop,
                    host_path,
                })
            }
            _ => None,
        })
        .collect()
}

/// The exists-then-create pattern has an inherent TOCTOU gap, but the reconciler is the sole
/// actor managing volumes on this node so no concurrent race is possible. If concurrent
/// reconciliation is ever introduced, call create unconditionally and treat "already exists"
/// as success.
async fn ensure_volumes(driver: &System, volumes: &[ContainerVolume]) -> Result<(), ActuateError> {
    for vol in volumes {
        if let Some(host_path) = &vol.host_path {
            // r[impl actuate.volume.storage]
            // Named non-tmpfs volume managed by VolumeStore; ensure the
            // host directory/subvolume exists.
            if !host_path.exists() {
                let bind_name = vol
                    .bind_name
                    .as_ref()
                    .expect("named bind volume must carry a VolumeName");
                driver.volume_store.create(bind_name).await.map_err(|e| {
                    super::VolumeWriteSnafu {
                        path: host_path.clone(),
                    }
                    .into_error(e)
                })?;
            }
            // Named volumes only get writes on first creation.
            // (The reconciler handles their lifecycle.)
        } else {
            let just_created = if !driver
                .container
                .volume_exists(&vol.name)
                .await
                .context(ContainerSnafu)?
            {
                driver
                    .container
                    .create_volume(&vol.name, vol.def.tmpfs)
                    .await
                    .context(ContainerSnafu)?;
                true
            } else {
                false
            };
            // r[impl actuate.volume.tmpfs]
            let needs_writes = just_created || vol.def.tmpfs;
            if needs_writes && !vol.def.writes.is_empty() {
                let mountpoint = driver
                    .container
                    .volume_mountpoint(&vol.name)
                    .await
                    .context(ContainerSnafu)?;
                for (path, contents) in &vol.def.writes {
                    safe_volume_write(&mountpoint, path, contents).await?;
                }
            }
        }
    }
    Ok(())
}

/// Returns the bridge name if a new network was created, `None` if it already existed.
///
/// The check-then-create TOCTOU gap is benign because the reconciler is the sole actor managing
/// networks on this node. If concurrent reconciliation is ever introduced, call create
/// unconditionally and treat "already exists" as success.
async fn ensure_network(
    driver: &System,
    net_name: &str,
    net_prefix: Ipv6Net,
) -> Result<Option<String>, ActuateError> {
    if !driver
        .container
        .network_exists(net_name)
        .await
        .context(ContainerSnafu)?
    {
        Ok(Some(
            driver
                .container
                .create_network(net_name, net_prefix, None)
                .await
                .context(ContainerSnafu)?,
        ))
    } else {
        Ok(None)
    }
}

impl Actuator {
    /// Resolves `service_mounts` declared on a pod to `(mount_port, service_ip,
    /// service_port)` tuples, computing each service's stable IPv6 address from
    /// the node prefix and the service's persisted instance identity.
    pub(crate) fn resolve_service_mounts(
        &self,
        instance: &ResourceInstance,
        mounts: &[ServicePort],
    ) -> Result<Vec<(u16, std::net::Ipv6Addr, u16)>, RegistryError> {
        mounts
            .iter()
            .map(|sp| {
                let svc_instance = self.registry.get_or_create_singleton(
                    &instance.app,
                    ResourceKind::Service,
                    Some(sp.service.name.as_str()),
                )?;
                let service_ip = instance_ipv6(&self.node_prefix, &svc_instance);
                Ok((sp.port.get(), service_ip, sp.port.get()))
            })
            .collect::<Result<Vec<_>, _>>()
    }

    // r[impl actuate.deployment.anon-volume.start]
    pub(crate) async fn start_pod_instance(
        &self,
        instance: &ResourceInstance,
        image: &str,
        raw_mounts: &[ServicePort],
        restart: TransientRestart,
        volumes: &[ContainerVolume],
        build_argv: impl FnOnce(String, Ipv6Net, &[(u16, std::net::Ipv6Addr, u16)]) -> Vec<String>,
    ) -> Result<Option<String>, ActuateError> {
        self.ensure_image_available(image).await?;

        // Set up volumes and network concurrently — neither depends on the other.
        let net_name = pod_network_name(instance);
        let net_prefix = pod_network_prefix(&self.node_prefix, instance);

        let ((), bridge_name) = tokio::try_join!(
            ensure_volumes(&self.driver, volumes),
            ensure_network(&self.driver, &net_name, net_prefix),
        )?;

        // Handle any pre-existing unit or orphaned container with this name.
        let unit = unit_name(instance);
        if let Some(state) = self
            .driver
            .process
            .unit_state(&unit)
            .await
            .context(ProcessSnafu)?
        {
            match state.active {
                ActiveState::Active | ActiveState::Activating => return Ok(bridge_name),
                _ => {
                    self.driver
                        .process
                        .reset_failed_unit(&unit)
                        .await
                        .context(ProcessSnafu)?;
                }
            }
        }

        // Remove any orphaned container left behind by a previous stop that
        // returned before cleanup finished (e.g. unit was Deactivating).
        // Ignore errors — the container may not exist, which is the common case.
        let _ = self
            .driver
            .container
            .remove_container(&instance.display_name, true)
            .await;

        // Resolve service mounts and build the argv.
        let mounts = self
            .resolve_service_mounts(instance, raw_mounts)
            .context(RegistrySnafu)?;
        let argv = build_argv(net_name, net_prefix, &mounts);

        let resource_kind_str = match instance.kind {
            ResourceKind::Parameter => "parameter",
            ResourceKind::Service => "service",
            ResourceKind::HttpService => "http_service",
            ResourceKind::Ingress => "ingress",
            ResourceKind::Deployment => "deployment",
            ResourceKind::Job => "job",
            ResourceKind::Volume => "volume",
            ResourceKind::ExternalVolume => "external_volume",
            ResourceKind::ExternalService => "external_service",
            ResourceKind::Action => "action",
        };
        let resource_name = instance.name.as_deref().unwrap_or(&instance.display_name);

        self.driver
            .process
            .start_transient(TransientUnitSpec {
                name: unit,
                description: format!("seedling container {}", instance.display_name),
                exec_start: argv,
                restart,
                log_extra_fields: vec![
                    ("SEEDLING_APP".to_owned(), instance.app.as_str().to_owned()),
                    (
                        "SEEDLING_RESOURCE_KIND".to_owned(),
                        resource_kind_str.to_owned(),
                    ),
                    ("SEEDLING_RESOURCE".to_owned(), resource_name.to_owned()),
                    (
                        "SEEDLING_INSTANCE".to_owned(),
                        instance.display_name.clone(),
                    ),
                ],
            })
            .await
            .context(ProcessSnafu)?;

        Ok(bridge_name)
    }

    // r[impl reconciliation.liveness]
    pub(crate) async fn stop_pod_instance(
        &self,
        instance: &ResourceInstance,
        anon_volume_names: &[String],
    ) -> Result<(), ActuateError> {
        let unit = unit_name(instance);

        if let Some(state) = self
            .driver
            .process
            .unit_state(&unit)
            .await
            .context(ProcessSnafu)?
        {
            match state.active {
                ActiveState::Active | ActiveState::Activating | ActiveState::Deactivating => {
                    self.driver
                        .process
                        .stop_unit(&unit)
                        .await
                        .context(ProcessSnafu)?;
                }
                ActiveState::Inactive | ActiveState::Failed => {
                    self.driver
                        .process
                        .reset_failed_unit(&unit)
                        .await
                        .context(ProcessSnafu)?;
                }
            }
        }

        // Container may already be gone (podman --rm removes it on exit).
        // Ignore not-found errors so network and volume cleanup always proceeds.
        let _ = self
            .driver
            .container
            .remove_container(&instance.display_name, true)
            .await;

        let net_name = pod_network_name(instance);
        if self
            .driver
            .container
            .network_exists(&net_name)
            .await
            .context(ContainerSnafu)?
        {
            self.driver
                .container
                .remove_network(&net_name)
                .await
                .context(ContainerSnafu)?;
        }

        for vol_name in anon_volume_names {
            if self
                .driver
                .container
                .volume_exists(vol_name)
                .await
                .context(ContainerSnafu)?
            {
                self.driver
                    .container
                    .remove_volume(vol_name)
                    .await
                    .context(ContainerSnafu)?;
            }
        }

        Ok(())
    }
}
