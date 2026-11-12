use std::{
    collections::{BTreeMap, HashMap},
    net::Ipv6Addr,
    path::Path,
    sync::Arc,
};

use ipnet::Ipv6Net;
use parking_lot::Mutex as ParkingMutex;
use seedling_protocol::events::EventSender;
use seedling_protocol::names::{AppName, ExternalVolumeName, VolumeRef};
use snafu::{IntoError, ResultExt, Snafu};

use crate::{
    defs::{
        container::VolumeMount,
        enums::{OnExit, Priority},
        resource::{Resource, ResourceKind},
    },
    runtime::{
        db::DbHandle,
        external_volume_mappings,
        identity::{ResourceInstance, VolumeName},
        priority::AppPriority,
        registry::InstanceRegistry,
        restart_gens, site_volumes,
    },
    system::{
        System,
        priority::WorkloadStanding,
        translate::{
            container::{deployment_spec, job_spec, podman_args, spec_hash},
            proxy::pod_network_prefix,
        },
    },
};

mod pod;

pub(crate) use pod::unit_name;
mod pull;

use pod::{ContainerVolume, collect_container_volumes, map_on_exit};
pub use pod::{TMPFS_VOLUMES_DIR, tmpfs_volume_path};
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
    // spec: FLT#workload-faults
    #[snafu(display("external volume '{name}' is not mapped"))]
    ExternalVolumeNotMapped {
        name: String,
        backtrace: snafu::Backtrace,
    },
}

// spec: VOL#write-paths
/// Write a file into a volume using kernel-confined `openat2(RESOLVE_BENEATH)`.
pub(crate) async fn safe_volume_write(
    mountpoint: &Path,
    rel_path: &str,
    contents: &[u8],
) -> Result<(), ActuateError> {
    super::confined::write_async(mountpoint, rel_path, contents)
        .await
        .map_err(|e| match e {
            super::confined::ConfinedWriteError::Escape { path, .. } => {
                VolumePathEscapeSnafu { path }.build()
            }
            super::confined::ConfinedWriteError::Io { path, source, .. } => {
                VolumeWriteSnafu { path }.into_error(source)
            }
        })
}

/// Bring a named non-tmpfs volume's storage into existence and seed it with
/// its declared writes, as one step. Returns whether this call created it.
///
/// Both the volumes phase and the pods phase reach a named volume, and they
/// run concurrently, so either may be the one that finds it absent. Creation
/// and seeding therefore cannot live in separate places: a volume only ever
/// receives its declared writes at creation, so whichever phase got there
/// first would decide whether they were applied at all.
///
/// A write that fails takes the storage back out, so the next tick creates
/// and seeds afresh instead of finding a directory that counts as created and
/// will never be written to. Only storage this call created is taken back
/// out: the loser of a race must not tear out what the winner is seeding.
///
/// Creation is the decision, not a look before it. Both phases can find the
/// volume absent at the same moment, so asking first and creating second
/// would only narrow the window; `create` reports which call made the
/// storage, and that answer is what settles who owes the writes.
// spec: ACTU#volume-creation-and-seeding
pub(crate) async fn ensure_named_volume(
    vol_store: &crate::system::volume_store::VolumeStore,
    name: &VolumeName,
    writes: &[(String, bytes::Bytes)],
) -> Result<bool, ActuateError> {
    let mountpoint = vol_store.path(name);
    let created = vol_store.create(name).await.map_err(|e| {
        VolumeWriteSnafu {
            path: mountpoint.clone(),
        }
        .into_error(e)
    })?;
    if !created {
        return Ok(false);
    }

    for (path, contents) in writes {
        if let Err(e) = safe_volume_write(&mountpoint, path, contents).await {
            if let Err(cleanup) = vol_store.remove(name).await {
                tracing::error!(
                    volume = %name,
                    path = %mountpoint.display(),
                    "seeding failed and the storage it created could not be removed, \
                     so the volume will stay unseeded: {cleanup}"
                );
            }
            return Err(e);
        }
    }

    Ok(true)
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
    pulling: Arc<ParkingMutex<HashMap<String, PullState>>>,
    db: DbHandle,
    event_tx: EventSender,
}

impl Actuator {
    pub fn new(
        driver: Arc<System>,
        node_prefix: Ipv6Net,
        registry: Arc<dyn InstanceRegistry>,
        dns_servers: Vec<Ipv6Addr>,
        db: DbHandle,
        event_tx: EventSender,
    ) -> Self {
        Self {
            driver,
            node_prefix,
            registry,
            dns_servers,
            pulling: Arc::new(ParkingMutex::new(HashMap::new())),
            db,
            event_tx,
        }
    }

    fn resolve_external_volumes(
        &self,
        app: &AppName,
    ) -> HashMap<ExternalVolumeName, crate::system::types::ResolvedExternalMount> {
        resolve_external_volumes(&self.db, &self.driver.volume_store, app)
    }

    // spec: ACTU#deployment-and-job-instances
    /// Ensure all primitives for this instance exist and are running.
    #[tracing::instrument(skip_all, fields(instance = %instance.display_name))]
    /// `app_priority` is the standing the reconciler resolved for this app on
    /// this tick. Passed in rather than read here so the weights the slices were
    /// given and the kill preference baked into the unit come from one read, and
    /// so a failed read is decided once per tick instead of once per instance.
    // spec: PRIO#kill-order
    pub async fn start(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
        app_priority: AppPriority,
    ) -> Result<Option<String>, ActuateError> {
        match resource {
            Resource::Deployment(dep) => {
                let (image, raw_mounts, restart, vols, required_ext_vols) = {
                    let def = dep.def.lock();
                    let pod = def.pod.lock();
                    let container = pod.container.lock();
                    let image = container.image.clone().unwrap_or_default();
                    let raw_mounts = pod.service_mounts.clone();
                    // spec: CTR#exit-strategy
                    let restart = map_on_exit(container.on_exit.unwrap_or(OnExit::Restart));
                    let required_ext_vols: Vec<String> = container
                        .volume_mounts
                        .values()
                        .filter_map(|m| match m {
                            VolumeMount::ExternalVolume(ev) if ev.operation_binding.is_none() => {
                                Some(ev.name.as_ref().to_string())
                            }
                            _ => None,
                        })
                        .collect();
                    drop(container);
                    let vols = collect_container_volumes(
                        &pod,
                        instance,
                        Some(self.driver.volume_store.volumes_dir()),
                    );
                    (image, raw_mounts, restart, vols, required_ext_vols)
                };
                let external_vols = self.resolve_external_volumes(&instance.app);
                // spec: FLT#workload-faults
                for name in &required_ext_vols {
                    if !external_vols.contains_key(name.as_str()) {
                        tracing::warn!(
                            app = %instance.app,
                            instance = %instance.display_name,
                            volume = %name,
                            "external volume not mapped, blocking start"
                        );
                        return Err(ExternalVolumeNotMappedSnafu { name: name.clone() }.build());
                    }
                }
                // spec: CTL#deployment-restart
                let restart_gen = {
                    let app_name = instance.app.clone();
                    let dep_name = instance.name.as_deref().unwrap_or("").to_owned();
                    self.db.call(move |db| {
                        restart_gens::load_restart_gen(db, &app_name, &dep_name).unwrap_or(0)
                    })
                };
                let (kill_signal, timeout_stop_secs, declared_priority) = {
                    let def = dep.def.lock();
                    let priority = def.priority;
                    let pod = def.pod.lock();
                    let c = pod.container.lock();
                    (c.stop_signal.clone(), c.stop_timeout_secs, priority)
                };
                let standing = WorkloadStanding::new(app_priority, declared_priority);
                self.start_pod_instance(
                    instance,
                    &image,
                    &raw_mounts,
                    restart,
                    &vols,
                    kill_signal,
                    timeout_stop_secs,
                    standing,
                    |net_name, net_prefix, mounts| {
                        let guard = dep.def.lock();
                        let spec = deployment_spec(
                            &guard,
                            instance,
                            &BTreeMap::new(),
                            &(net_name, net_prefix),
                            mounts,
                            &self.dns_servers,
                            Some(self.driver.volume_store.volumes_dir()),
                            &external_vols,
                            restart_gen,
                        );
                        podman_args(&spec)
                    },
                )
                .await
            }
            Resource::Job(job) => {
                let (image, raw_mounts, restart, vols, required_ext_vols) = {
                    let def = job.def.lock();
                    let pod = def.pod.lock();
                    let container = pod.container.lock();
                    let image = container.image.clone().unwrap_or_default();
                    let raw_mounts = pod.service_mounts.clone();
                    // spec: CTR#exit-strategy
                    // Jobs default to Terminate so systemd does not restart them on completion.
                    let restart = map_on_exit(container.on_exit.unwrap_or(OnExit::Terminate));
                    let required_ext_vols: Vec<String> = container
                        .volume_mounts
                        .values()
                        .filter_map(|m| match m {
                            VolumeMount::ExternalVolume(ev) if ev.operation_binding.is_none() => {
                                Some(ev.name.as_ref().to_string())
                            }
                            _ => None,
                        })
                        .collect();
                    drop(container);
                    let vols = collect_container_volumes(
                        &pod,
                        instance,
                        Some(self.driver.volume_store.volumes_dir()),
                    );
                    (image, raw_mounts, restart, vols, required_ext_vols)
                };
                let external_vols = self.resolve_external_volumes(&instance.app);
                // spec: FLT#workload-faults
                for name in &required_ext_vols {
                    if !external_vols.contains_key(name.as_str()) {
                        tracing::warn!(
                            app = %instance.app,
                            instance = %instance.display_name,
                            volume = %name,
                            "external volume not mapped, blocking start"
                        );
                        return Err(ExternalVolumeNotMappedSnafu { name: name.clone() }.build());
                    }
                }
                let (kill_signal, timeout_stop_secs) = {
                    let def = job.def.lock();
                    let pod = def.pod.lock();
                    let c = pod.container.lock();
                    (c.stop_signal.clone(), c.stop_timeout_secs)
                };
                // spec: DEP#priority
                // Priority is declared on Deployments only; every other
                // workload stands at Normal within its app.
                let standing = WorkloadStanding::new(app_priority, Priority::Normal);
                self.start_pod_instance(
                    instance,
                    &image,
                    &raw_mounts,
                    restart,
                    &vols,
                    kill_signal,
                    timeout_stop_secs,
                    standing,
                    |net_name, net_prefix, mounts| {
                        let guard = job.def.lock();
                        let spec = job_spec(
                            &guard,
                            instance,
                            &BTreeMap::new(),
                            &(net_name, net_prefix),
                            mounts,
                            &self.dns_servers,
                            Some(self.driver.volume_store.volumes_dir()),
                            &external_vols,
                            0,
                        );
                        podman_args(&spec)
                    },
                )
                .await
            }
            // Everything else has no unit and so no standing to carry.
            other => self.start_volume(instance, other).await,
        }
    }

    /// Start a resource that is not backed by a supervised unit, and so has
    /// no standing to carry: volumes, and the kinds that exist only as
    /// declarations.
    pub async fn start_volume(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Result<Option<String>, ActuateError> {
        match resource {
            // spec: ACTU#volume-creation-and-seeding
            Resource::Volume(vol) => {
                let (tmpfs, writes) = {
                    let def = vol.def.lock();
                    (def.tmpfs, def.writes.clone())
                };
                if tmpfs {
                    // spec: ACTU#volume-storage
                    // The directory containers bind-mount *is* the volume, so
                    // it is what gets created and seeded here. Routing this
                    // through the container runtime's tmpfs driver instead
                    // would seed a mountpoint nothing ever mounts.
                    let host_path = tmpfs_volume_path(&instance.display_name);
                    // spec: ACTU#storage-confinement — classify the path before
                    // making anything at it. `create_dir_all` succeeds
                    // straight through a symlink standing at the name, and
                    // the declared writes would then land wherever it points.
                    crate::system::confined::owned_dir_async(&host_path)
                        .await
                        .context(VolumeWriteSnafu {
                            path: host_path.clone(),
                        })?;
                    tokio::fs::create_dir_all(&host_path)
                        .await
                        .context(VolumeWriteSnafu {
                            path: host_path.clone(),
                        })?;
                    // Tmpfs contents don't survive a reboot; always re-apply writes.
                    for (path, contents) in &writes {
                        safe_volume_write(&host_path, path, contents).await?;
                    }
                } else {
                    // spec: ACTU#volume-storage
                    let vol_store = &self.driver.volume_store;
                    let name = VolumeName::of_instance(instance);
                    if let Some(bsl_name) = instance.name.as_deref() {
                        let legacy = format!("{}-{}", instance.app, bsl_name);
                        vol_store
                            .migrate_legacy(&legacy, &name, instance.app.as_str())
                            .await
                            .map_err(|e| {
                                VolumeWriteSnafu {
                                    path: vol_store.path(&name),
                                }
                                .into_error(e)
                            })?;
                    }
                    ensure_named_volume(vol_store, &name, &writes).await?;
                }
                Ok(None)
            }
            Resource::ExternalVolume(_) | Resource::ExternalService(_) => Ok(None),
            Resource::Service(_) => Ok(None),
            Resource::Ingress(_) => Ok(None),
            // A pod-backed workload is started through `start`, which
            // carries the standing its unit needs.
            Resource::Deployment(_) | Resource::Job(_) => UnsupportedKindSnafu {
                kind: instance.kind,
            }
            .fail(),
        }
    }

    // spec: RT#signal-delivery
    /// Send a POSIX signal to the running container's PID 1. Used to wire
    /// `rt.signal` in the BSL runtime to the underlying container runtime.
    /// Non-fatal if the container is already gone — caller can decide whether
    /// that constitutes failure.
    #[tracing::instrument(skip_all, fields(instance = %instance.display_name, signal))]
    pub async fn signal(
        &self,
        instance: &ResourceInstance,
        signal: &str,
    ) -> Result<bool, ActuateError> {
        self.driver
            .container
            .signal_container(&instance.display_name, signal)
            .await
            .map_err(|e| ContainerSnafu.into_error(e))
    }

    // spec: ACTU#deployment-and-job-instances
    // spec: ACTU#volume-removal
    /// Stop and remove all primitives for this instance.
    #[tracing::instrument(skip_all, fields(instance = %instance.display_name))]
    pub async fn stop(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Result<(), ActuateError> {
        match resource {
            Resource::Deployment(dep) => {
                let anon_volumes: Vec<ContainerVolume> = {
                    let def = dep.def.lock();
                    let pod = def.pod.lock();
                    collect_container_volumes(
                        &pod,
                        instance,
                        Some(self.driver.volume_store.volumes_dir()),
                    )
                    .into_iter()
                    .filter(|v| v.remove_on_stop)
                    .collect()
                };
                self.stop_pod_instance(instance, &anon_volumes).await
            }
            Resource::Job(job) => {
                let anon_volumes: Vec<ContainerVolume> = {
                    let def = job.def.lock();
                    let pod = def.pod.lock();
                    collect_container_volumes(
                        &pod,
                        instance,
                        Some(self.driver.volume_store.volumes_dir()),
                    )
                    .into_iter()
                    .filter(|v| v.remove_on_stop)
                    .collect()
                };
                self.stop_pod_instance(instance, &anon_volumes).await
            }
            Resource::Volume(vol) => {
                let tmpfs = vol.def.lock().tmpfs;
                if tmpfs {
                    // spec: ACTU#volume-storage
                    // Remove the directory holding the data, not a stand-in
                    // for it: the tmpfs backing under /run is reclaimed once
                    // the directory is gone.
                    let host_path = tmpfs_volume_path(&instance.display_name);
                    // spec: ACTU#storage-confinement — a symlink wearing the
                    // name is refused rather than unlinked, so a stop never
                    // reports a cleanup that removed only a link.
                    if crate::system::confined::owned_dir_async(&host_path)
                        .await
                        .context(VolumeWriteSnafu {
                            path: host_path.clone(),
                        })?
                        && let Err(e) = tokio::fs::remove_dir_all(&host_path).await
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        return Err(VolumeWriteSnafu { path: host_path }.into_error(e));
                    }
                    // Earlier releases stood a podman tmpfs-driver volume up
                    // beside the directory. Reap it on the way past, by the
                    // exact name those releases gave it.
                    let podman_name = &instance.display_name;
                    if self
                        .driver
                        .container
                        .volume_exists(podman_name)
                        .await
                        .context(ContainerSnafu)?
                    {
                        self.driver
                            .container
                            .remove_volume(podman_name)
                            .await
                            .context(ContainerSnafu)?;
                    }
                } else {
                    // spec: ACTU#held-volumes
                    let vol_store = &self.driver.volume_store;
                    let name = VolumeName::of_instance(instance);
                    let held = vol_store
                        .hold(&name, instance.app.as_str(), "removed from app definition")
                        .await
                        .map_err(|e| {
                            VolumeWriteSnafu {
                                path: vol_store.path(&name),
                            }
                            .into_error(e)
                        })?;
                    if let Some(meta) = held {
                        // spec: ACTU#held-volumes
                        self.event_tx.held_volume_created(
                            meta.id,
                            &instance.app,
                            &meta.volume_name,
                            &meta.reason,
                            None,
                        );
                    }
                }
                Ok(())
            }
            Resource::ExternalVolume(_) | Resource::ExternalService(_) => Ok(()),
            Resource::Service(_) => Ok(()),
            Resource::Ingress(_) => Ok(()),
        }
    }

    /// Hold a named volume's data for operator review instead of deleting it.
    ///
    /// Used when a volume's storage backend needs migration: the old data is
    /// held and a fresh volume will be created by a subsequent `start` call.
    // spec: ACTU#held-volumes
    #[tracing::instrument(skip_all, fields(instance = %instance.display_name))]
    pub async fn hold_volume(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
        reason: &str,
    ) -> Result<(), ActuateError> {
        if let Resource::Volume(vol) = resource {
            let tmpfs = vol.def.lock().tmpfs;
            if !tmpfs {
                let vol_store = &self.driver.volume_store;
                let name = crate::runtime::identity::VolumeName::of_instance(instance);
                let held = vol_store
                    .hold(&name, instance.app.as_str(), reason)
                    .await
                    .map_err(|e| {
                        VolumeWriteSnafu {
                            path: vol_store.path(&name),
                        }
                        .into_error(e)
                    })?;
                if let Some(meta) = held {
                    // spec: ACTU#held-volumes
                    self.event_tx.held_volume_created(
                        meta.id,
                        &instance.app,
                        &meta.volume_name,
                        &meta.reason,
                        None,
                    );
                }
            }
        }
        Ok(())
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
                // spec: CTL#deployment-restart
                let dep_name = instance.name.as_deref().unwrap_or("").to_owned();
                let restart_gen = {
                    let app_name = instance.app.clone();
                    self.db.call(move |db| {
                        restart_gens::load_restart_gen(db, &app_name, &dep_name).unwrap_or(0)
                    })
                };
                deployment_spec(
                    &def,
                    instance,
                    &std::collections::BTreeMap::new(),
                    &(net_name, net_prefix),
                    &mounts,
                    &self.dns_servers,
                    Some(self.driver.volume_store.volumes_dir()),
                    &self.resolve_external_volumes(&instance.app),
                    restart_gen,
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
                    Some(self.driver.volume_store.volumes_dir()),
                    &self.resolve_external_volumes(&instance.app),
                    0,
                )
            }
            _ => return None,
        };

        Some(spec_hash(&spec))
    }
}

/// Resolve the external-volume mappings for an app into concrete
/// `ResolvedExternalMount` entries keyed by the BSL external-volume name.
///
/// Shared with the shell session path so an operator-opened shell mounts the
/// same bind sources the reconciler would use for a Job or Deployment.
// spec: ACN#shells
pub fn resolve_external_volumes(
    db: &DbHandle,
    vol_store: &crate::system::volume_store::VolumeStore,
    app: &AppName,
) -> HashMap<ExternalVolumeName, crate::system::types::ResolvedExternalMount> {
    use crate::system::types::{MountSource, ResolvedExternalMount};

    let app_owned = app.clone();
    let (mappings, site_vols) = db.call(move |db| {
        let mappings = external_volume_mappings::list_for_app(db, &app_owned).unwrap_or_else(|e| {
            tracing::warn!(app = %app_owned, error = %e, "failed to load external volume mappings");
            vec![]
        });
        let site_vols = site_volumes::list(db).unwrap_or_default();
        (mappings, site_vols)
    });

    let mut resolved = HashMap::new();
    for mapping in mappings {
        let mount = match &mapping.target {
            VolumeRef::Site { name } => {
                let site_vol = site_vols.iter().find(|s| s.name == *name);
                match site_vol {
                    Some(sv) => {
                        let path = match &sv.kind {
                            site_volumes::SiteVolumeKind::Managed => {
                                vol_store.site_path(sv.name.as_str())
                            }
                            site_volumes::SiteVolumeKind::Bind { host_path } => {
                                std::path::PathBuf::from(host_path)
                            }
                            site_volumes::SiteVolumeKind::Snapshot { .. } => {
                                vol_store.site_path(sv.name.as_str())
                            }
                        };
                        // spec: SITE#site-volume-snapshots
                        // Snapshot volumes are inherently read-only at the filesystem level.
                        let effective_read_only = mapping.read_only || sv.is_read_only();
                        ResolvedExternalMount {
                            source: MountSource::Bind(path),
                            read_only: effective_read_only,
                        }
                    }
                    None => {
                        tracing::warn!(
                            app = %app,
                            external = %mapping.external_name,
                            target = %name,
                            "site volume not found for external volume mapping"
                        );
                        continue;
                    }
                }
            }
            VolumeRef::App {
                app: target_app,
                volume,
            } => {
                let vol_name = VolumeName::for_app(target_app.as_str(), volume.as_str());
                let path = vol_store.path(&vol_name);
                ResolvedExternalMount {
                    source: MountSource::Bind(path),
                    read_only: mapping.read_only,
                }
            }
        };
        resolved.insert(mapping.external_name, mount);
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::volume_store::VolumeStore;

    fn store() -> (tempfile::TempDir, VolumeStore) {
        let data = tempfile::tempdir().unwrap();
        let store = VolumeStore::new(data.path(), false).unwrap();
        (data, store)
    }

    // spec: ACTU#volume-creation-and-seeding
    #[tokio::test]
    async fn creating_a_named_volume_seeds_it_in_the_same_step() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");
        let writes = vec![("conf/app.toml".to_owned(), bytes::Bytes::from("port = 80"))];

        assert!(ensure_named_volume(&store, &name, &writes).await.unwrap());

        let seeded = store.path(&name).join("conf/app.toml");
        assert_eq!(std::fs::read_to_string(seeded).unwrap(), "port = 80");
    }

    /// Whichever phase reaches the volume second must leave its contents
    /// alone: the declared writes seed the volume, they do not own it.
    // spec: ACTU#volume-creation-and-seeding
    #[tokio::test]
    async fn a_volume_that_is_already_there_is_not_seeded_again() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");
        let writes = vec![("conf/app.toml".to_owned(), bytes::Bytes::from("port = 80"))];

        ensure_named_volume(&store, &name, &writes).await.unwrap();
        let seeded = store.path(&name).join("conf/app.toml");
        std::fs::write(&seeded, "port = 8080").unwrap();

        assert!(!ensure_named_volume(&store, &name, &writes).await.unwrap());

        assert_eq!(
            std::fs::read_to_string(&seeded).unwrap(),
            "port = 8080",
            "the app owns the volume's contents once it has them"
        );
    }

    /// A volume only ever receives its writes at creation, so storage left
    /// behind by a failed seed would stay unseeded for good.
    // spec: ACTU#volume-creation-and-seeding
    #[tokio::test]
    async fn a_failed_write_takes_the_storage_it_created_back_out() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");
        let writes = vec![
            ("conf/app.toml".to_owned(), bytes::Bytes::from("port = 80")),
            ("../escape".to_owned(), bytes::Bytes::from("elsewhere")),
        ];

        let err = ensure_named_volume(&store, &name, &writes)
            .await
            .unwrap_err();

        assert!(
            matches!(err, ActuateError::VolumePathEscape { .. }),
            "{err}"
        );
        assert!(
            !store.exists(&name),
            "the next attempt must create and seed afresh"
        );
    }

    /// Both phases can find the volume absent at once. The one whose create
    /// did not take effect owes the volume nothing — it must not report the
    /// name as an error, and above all must not take the winner's storage
    /// back out on a write it was never responsible for.
    // spec: ACTU#volume-creation-and-seeding
    #[tokio::test]
    async fn losing_the_race_to_create_is_not_a_failure_and_undoes_nothing() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");

        assert!(store.create(&name).await.unwrap(), "the winner creates it");
        let seeded = store.path(&name).join("conf/app.toml");
        std::fs::create_dir_all(seeded.parent().unwrap()).unwrap();
        std::fs::write(&seeded, "port = 80").unwrap();

        let writes = vec![("../escape".to_owned(), bytes::Bytes::from("elsewhere"))];
        assert!(
            !ensure_named_volume(&store, &name, &writes).await.unwrap(),
            "the loser reports that it did not create the volume"
        );

        assert!(store.exists(&name));
        assert_eq!(std::fs::read_to_string(&seeded).unwrap(), "port = 80");
    }

    /// `create` is the decision about who owns seeding, so it has to answer
    /// that question rather than fail on a name already in use.
    // spec: ACTU#volume-creation-and-seeding
    #[tokio::test]
    async fn creating_a_volume_that_is_already_there_reports_no_creation() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");

        assert!(store.create(&name).await.unwrap());
        assert!(!store.create(&name).await.unwrap());
    }
}
