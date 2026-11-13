use ipnet::Ipv6Net;
use snafu::ResultExt;

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
        priority::WorkloadStanding,
        translate::proxy::instance_ipv6,
        types::{ActiveState, TransientRestart, TransientUnitSpec},
    },
};

use super::{ActuateError, Actuator, ContainerSnafu, ProcessSnafu, RegistrySnafu};

fn pod_network_name(instance: &ResourceInstance) -> String {
    format!("seedling-{}", instance.display_name)
}

// spec: LINUX#supervised-units
pub(crate) fn unit_name(instance: &ResourceInstance) -> String {
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

/// Host path under which tmpfs volumes are bind-mounted. /run is tmpfs-backed
/// on systemd hosts, so volumes here satisfy the BSL spec's "RAM-based
/// filesystem" semantic without seedling having to mount its own tmpfs.
// spec: ACTU#volume-storage
pub const TMPFS_VOLUMES_DIR: &str = "/run/seedling/tmpfs-volumes";

/// Host path backing one tmpfs volume, named or anonymous.
///
/// Every part of the runtime that touches a tmpfs volume — the actuator that
/// creates and removes it, the observer that reports whether it is there, the
/// translator that builds the container's bind mount, and the `rt.write` path
/// — must address the same directory, so the join lives here rather than
/// being repeated beside a comment asking callers to keep it in sync.
// spec: ACTU#volume-storage
pub fn tmpfs_volume_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(TMPFS_VOLUMES_DIR).join(name)
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
                let tmpfs = v.def.lock().tmpfs;
                let (name, bind_name, remove_on_stop, host_path) = match &v.name {
                    None => {
                        let vol_name = v
                            .anon_id
                            .clone()
                            .expect("anonymous volume must have an anon_id");
                        // spec: ACTU#volume-storage
                        // Anonymous tmpfs volumes get a host bind path under
                        // /run so that `volume.write` declarations actually
                        // propagate into the container — podman's own tmpfs
                        // volume driver creates a fresh tmpfs at every mount,
                        // wiping any data we'd written to its host _data path.
                        let host_path = if tmpfs {
                            Some(tmpfs_volume_path(&vol_name))
                        } else {
                            None
                        };
                        (vol_name, None, true, host_path)
                    }
                    Some(n) => {
                        let vol_name = VolumeName::for_app(instance.app.as_str(), n.as_str());
                        let host_path = if tmpfs {
                            // spec: ACTU#volume-storage
                            Some(tmpfs_volume_path(vol_name.as_str()))
                        } else {
                            volumes_dir.map(|dir| dir.join(vol_name.as_str()))
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

/// Materialise the volumes a pod's container mounts, before it starts.
///
/// The exists-then-create pattern here has an inherent TOCTOU gap. It is
/// benign for the anonymous volumes the container runtime manages, since the
/// reconciler is the only actor creating those. It is *not* benign for named
/// volumes: this phase and the volumes phase run concurrently and both reach
/// them, which is why creating one goes through `ensure_named_volume` rather
/// than being open-coded here. Losing that race once meant losing the
/// volume's declared writes for good.
async fn ensure_volumes(driver: &System, volumes: &[ContainerVolume]) -> Result<(), ActuateError> {
    for vol in volumes {
        if let Some(host_path) = &vol.host_path {
            if vol.def.tmpfs {
                // spec: ACTU#volume-storage
                // Tmpfs volumes are bind-mounted from a tmpfs-backed host
                // path under /run/seedling/tmpfs-volumes. Ensure the path
                // exists and re-apply declared writes every time the volume
                // is materialised — tmpfs contents do not survive a reboot
                // (the entire /run/seedling/tmpfs-volumes tree is recreated
                // by the daemon at startup), and even within a single boot
                // the directory may have been removed when an earlier
                // container exited.
                tokio::fs::create_dir_all(host_path)
                    .await
                    .context(super::VolumeWriteSnafu {
                        path: host_path.clone(),
                    })?;
                for (path, contents) in &vol.def.writes {
                    safe_volume_write(host_path, path, contents).await?;
                }
            } else {
                // spec: ACTU#volume-storage
                // spec: ACTU#volume-creation-and-seeding
                // Named non-tmpfs volume managed by VolumeStore. This phase
                // runs alongside the volumes phase, so it may well be the one
                // that finds the volume absent — in which case it owes the
                // volume its declared writes, which only ever land at
                // creation.
                let bind_name = vol
                    .bind_name
                    .as_ref()
                    .expect("named bind volume must carry a VolumeName");
                super::ensure_named_volume(&driver.volume_store, bind_name, &vol.def.writes)
                    .await?;
            }
        } else {
            // Anonymous non-tmpfs volume: managed by the container runtime.
            let just_created = if !driver
                .container
                .volume_exists(&vol.name)
                .await
                .context(ContainerSnafu)?
            {
                driver
                    .container
                    .create_volume(&vol.name, false)
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
                    Some(sp.service.name().as_str()),
                )?;
                let service_ip = instance_ipv6(&self.node_prefix, &svc_instance);
                Ok((sp.port.get(), service_ip, sp.port.get()))
            })
            .collect::<Result<Vec<_>, _>>()
    }

    // spec: ACTU#deployment-and-job-instances
    #[expect(
        clippy::too_many_arguments,
        reason = "single internal helper that fans out the per-instance start; \
                  packing into a config struct would just relocate the parameter list"
    )]
    pub(crate) async fn start_pod_instance(
        &self,
        instance: &ResourceInstance,
        image: &str,
        raw_mounts: &[ServicePort],
        restart: TransientRestart,
        volumes: &[ContainerVolume],
        kill_signal: Option<String>,
        timeout_stop_secs: Option<u32>,
        standing: WorkloadStanding,
        build_argv: impl FnOnce(String, Ipv6Net, &[(u16, std::net::Ipv6Addr, u16)]) -> Vec<String>,
    ) -> Result<Option<String>, ActuateError> {
        self.ensure_image_available(image).await?;

        // Set up volumes and network concurrently — neither depends on the other.
        let net_name = pod_network_name(instance);
        let net_prefix = self.allocate_pod_prefix(instance)?;

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

        let resource_kind_str = crate::system::journal::resource_kind_slug(instance.kind);
        let unit_for_breadcrumb = unit.clone();
        self.driver
            .process
            .start_transient(TransientUnitSpec {
                name: unit,
                description: format!("seedling container {}", instance.display_name),
                exec_start: argv,
                restart,
                kill_signal,
                timeout_stop_secs,
                // spec: PRIO#resource-control-groups
                // spec: LINUX#resource-control-groups
                // The tier slice carries the Deployment weight and nests under
                // the app slice, which carries the app weight.
                slice: Some(standing.slice(&instance.app)),
                // spec: PRIO#kill-order
                // spec: LINUX#kill-order
                oom_score_adjust: Some(standing.oom_score_adjust()),
                // spec: AUTO#restart-pacing
                // spec: LINUX#restart-pacing
                // Pod containers: 5 second pause between restarts, and a
                // 10-minute / 10-attempt window before systemd gives up. The
                // wider window means a slow-failing container (e.g. takes
                // 30s to crash) still gets multiple chances; the per-attempt
                // delay means a fast-failing container can't burn through
                // the budget in a heartbeat.
                restart_sec: Some(5),
                start_limit_interval_sec: Some(600),
                start_limit_burst: Some(10),
                log_extra_fields: crate::system::journal::instance_log_fields(instance),
            })
            .await
            .context(ProcessSnafu)?;

        // Trace which rt.* call produced this systemd unit so the unit's
        // own log starts with a Seedling-side breadcrumb identifying it.
        // Anonymous resources fall back to the resource_kind/display_name
        // since they have no operator-facing BSL name.
        let source_call = match &instance.name {
            Some(name) => format!("rt.start({resource_kind_str}/{name})"),
            None => format!(
                "rt.start(anon-{resource_kind_str}/{})",
                instance.display_name
            ),
        };
        crate::system::breadcrumb::Breadcrumb {
            app: Some(&instance.app),
            kind: crate::system::breadcrumb::BreadcrumbKind::UnitCreate {
                unit: &unit_for_breadcrumb,
                source_call: &source_call,
            },
            script_pos: None,
        }
        .emit();

        Ok(bridge_name)
    }

    // spec: RCN#reconciliation-loop
    pub(crate) async fn stop_pod_instance(
        &self,
        instance: &ResourceInstance,
        anon_volumes: &[ContainerVolume],
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
        // The network is gone, so its prefix is free for another.
        self.release_pod_prefix(instance);

        for vol in anon_volumes {
            if let Some(host_path) = &vol.host_path {
                // Tmpfs anonymous volume: bind-mounted from /run. Remove the
                // host directory; the tmpfs backing under /run is reclaimed
                // automatically when the directory empties.
                if host_path.exists()
                    && let Err(e) = tokio::fs::remove_dir_all(host_path).await
                {
                    tracing::warn!(
                        path = %host_path.display(),
                        "failed to remove tmpfs volume directory: {e}"
                    );
                }
            } else if self
                .driver
                .container
                .volume_exists(&vol.name)
                .await
                .context(ContainerSnafu)?
            {
                self.driver
                    .container
                    .remove_volume(&vol.name)
                    .await
                    .context(ContainerSnafu)?;
            }
        }

        Ok(())
    }
}
