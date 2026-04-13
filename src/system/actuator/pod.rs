use ipnet::Ipv6Net;
use snafu::ResultExt;

use crate::{
    defs::{
        container::VolumeMount, enums::OnExit, resource::ResourceKind, service::ServicePort,
        volume::VolumeDef,
    },
    runtime::identity::ResourceInstance,
    system::{
        System,
        translate::{
            container::anon_vol_name,
            proxy::{instance_ipv6, pod_network_prefix},
        },
        types::{ActiveState, TransientRestart, TransientUnitSpec},
    },
};

use super::{ActuateError, Actuator, ContainerSnafu, ProcessSnafu, VolumeWriteSnafu};

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
    /// Podman volume name.
    pub(super) name: String,
    pub(super) def: VolumeDef,
    /// Whether to remove this volume when the container stops.
    /// True for anonymous volumes (ephemeral per-container), false for named
    /// volumes (static reconciler-managed or dynamic user-named).
    pub(super) remove_on_stop: bool,
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

async fn ensure_volumes(driver: &System, volumes: &[ContainerVolume]) -> Result<(), ActuateError> {
    for vol in volumes {
        let just_created = if !driver
            .container
            .volume_exists(&vol.name)
            .await
            .context(ContainerSnafu)?
        {
            driver
                .container
                .create_volume(&vol.name)
                .await
                .context(ContainerSnafu)?;
            true
        } else {
            false
        };
        if just_created && !vol.def.writes.is_empty() {
            let mountpoint = driver
                .container
                .volume_mountpoint(&vol.name)
                .await
                .context(ContainerSnafu)?;
            for (path, contents) in &vol.def.writes {
                let dest = mountpoint.join(path.trim_start_matches('/'));
                if let Some(parent) = dest.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .context(VolumeWriteSnafu { path: dest.clone() })?;
                }
                tokio::fs::write(&dest, contents)
                    .await
                    .context(VolumeWriteSnafu { path: dest.clone() })?;
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

        // Handle any pre-existing unit with this name.
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
                    return Ok(());
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

        self.driver
            .container
            .remove_container(&instance.display_name, true)
            .await
            .context(ContainerSnafu)?;

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
