//! Adapters from the system's async container and volume operations to the
//! synchronous traits the BSL runtime calls while an operation's closure runs.

use std::sync::Arc;

use seedling_protocol::names::AppName;

use super::Engine;

// spec: RT#signal-delivery
/// Adapter from the system actuator's async `signal_container` to the
/// synchronous `ContainerSignaler` trait expected by the BSL runtime. The
/// adapter is constructed here rather than in the BSL runtime so that
/// language-only tests don't drag in a tokio runtime.
struct OperationSignaler {
    container: Arc<dyn crate::system::ContainerRuntime>,
}

impl crate::runtime::barrier::ContainerSignaler for OperationSignaler {
    fn signal(&self, container_name: &str, signal: &str) -> Result<bool, String> {
        let container = Arc::clone(&self.container);
        let name = container_name.to_owned();
        let sig = signal.to_owned();
        // The operation loop runs inside `spawn_blocking`, so we can step
        // back into the async runtime via `block_in_place` + the current
        // handle to deliver the signal synchronously.
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { container.signal_container(&name, &sig).await })
        });
        result.map_err(|e| e.to_string())
    }
}

pub(crate) fn make_container_signaler(
    state: &Engine,
) -> Option<Arc<dyn crate::runtime::barrier::ContainerSignaler>> {
    Some(Arc::new(OperationSignaler {
        container: Arc::clone(&state.driver.container),
    }))
}

// spec: RT#volume-writes
/// Adapter from the system actuator's async `safe_volume_write` to the
/// synchronous `VolumeWriter` trait expected by the BSL runtime, mirroring
/// `OperationSignaler`. Resolves the BSL-side volume identity to a host path
/// the same way the actuator does at container start.
struct OperationVolumeWriter {
    driver: Arc<crate::system::System>,
    db: crate::runtime::db::DbHandle,
}

impl crate::runtime::barrier::VolumeWriter for OperationVolumeWriter {
    // spec: VOL#external-volumes
    fn resolve_external(
        &self,
        app: &str,
        name: &str,
    ) -> Result<Option<crate::runtime::barrier::ResolvedExternalWrite>, String> {
        use crate::system::types::MountSource;

        let app = app
            .parse::<AppName>()
            .map_err(|e| format!("{app} is not an app name: {e}"))?;
        let mappings = crate::system::actuator::try_resolve_external_volumes(
            &self.db,
            &self.driver.volume_store,
            &app,
        )
        .map_err(|e| format!("the external volume mappings cannot be read: {e}"))?;
        match mappings.get(name) {
            None => Ok(None),
            Some(mount) => match &mount.source {
                MountSource::Bind(host_path) => {
                    Ok(Some(crate::runtime::barrier::ResolvedExternalWrite {
                        host_path: host_path.clone(),
                        read_only: mount.read_only,
                    }))
                }
                MountSource::Volume(_) | MountSource::Tmpfs => Err(format!(
                    "the mapping of external volume {name} is not a directory on the host"
                )),
            },
        }
    }

    fn write(
        &self,
        app: &str,
        target: crate::runtime::barrier::VolumeWriteTarget,
        path: &str,
        contents: &[u8],
    ) -> Result<(), String> {
        use std::path::PathBuf;

        use crate::{
            runtime::{barrier::VolumeWriteTarget, identity::VolumeName},
            system::actuator::{
                ensure_named_volume_storage, ensure_tmpfs_volume_dir, safe_volume_write,
                tmpfs_volume_path,
            },
        };

        let driver = Arc::clone(&self.driver);
        let app = app.to_owned();
        let path = path.to_owned();
        let contents = contents.to_vec();

        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let mountpoint: PathBuf = match target {
                    // spec: RT#volume-writes
                    // Storage that does not exist yet is created and seeded
                    // first, as starting the volume would.
                    VolumeWriteTarget::NamedVolume {
                        name,
                        tmpfs,
                        writes,
                    } => {
                        if tmpfs {
                            // spec: ACTU#volume-storage
                            let host_path =
                                tmpfs_volume_path(VolumeName::for_app(&app, &name).as_str());
                            ensure_tmpfs_volume_dir(&driver.volume_store, &host_path, &writes)
                                .await
                                .map_err(|e| format!("rt.write: {e}"))?;
                            host_path
                        } else {
                            ensure_named_volume_storage(&driver.volume_store, &app, &name, &writes)
                                .await
                                .map_err(|e| format!("rt.write: {e}"))?
                        }
                    }
                    VolumeWriteTarget::AnonymousVolume { anon_id, tmpfs } => {
                        if tmpfs {
                            // spec: ACTU#volume-storage
                            tmpfs_volume_path(&anon_id)
                        } else {
                            driver
                                .container
                                .volume_mountpoint(&anon_id)
                                .await
                                .map_err(|e| format!("rt.write: {e}"))?
                        }
                    }
                    VolumeWriteTarget::ExternalBound { host_path } => host_path,
                };
                safe_volume_write(&driver.volume_store, &mountpoint, &path, &contents)
                    .await
                    .map_err(|e| format!("rt.write: {e}"))
            })
        })
    }
}

pub(crate) fn make_volume_writer(
    state: &Engine,
) -> Option<Arc<dyn crate::runtime::barrier::VolumeWriter>> {
    Some(Arc::new(OperationVolumeWriter {
        driver: Arc::clone(&state.driver),
        db: state.db.clone(),
    }))
}

// spec: RT#command-execution
/// Adapter from the system actuator's async `exec_command` to the
/// synchronous `Executor` trait expected by the BSL runtime.
struct OperationExecutor {
    container: Arc<dyn crate::system::ContainerRuntime>,
}

impl crate::runtime::barrier::Executor for OperationExecutor {
    fn check_running(&self, target: &crate::runtime::ResourceInstance) -> Result<(), String> {
        let container = Arc::clone(&self.container);
        let name = target.display_name.clone();
        let state = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { container.inspect(&name).await })
        });
        match state {
            Ok(Some(s)) if matches!(s.status, crate::system::types::ContainerStatus::Running) => {
                Ok(())
            }
            Ok(Some(s)) => Err(format!(
                "{} is not running (it is {:?})",
                target.display_name, s.status
            )),
            Ok(None) => Err(format!(
                "{} is not running: no such container",
                target.display_name
            )),
            Err(e) => Err(format!(
                "cannot tell whether {} is running: {e}",
                target.display_name
            )),
        }
    }

    fn exec(
        &self,
        target: &crate::runtime::ResourceInstance,
        argv: &[String],
        extra_env: &[(String, String)],
    ) -> Result<i32, String> {
        let container = Arc::clone(&self.container);
        let name = target.display_name.clone();
        let argv: Vec<String> = argv.to_vec();
        let extra_env: Vec<(String, String)> = extra_env.to_vec();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { container.exec_command(&name, &argv, &extra_env).await })
        });
        let output = result.map_err(|e| e.to_string())?;
        // spec: RT#command-execution
        // What the command wrote goes with the container's logs, not to the
        // script and not to the daemon's own.
        crate::system::journal::record_exec_output(target, &output.stdout, &output.stderr);
        Ok(output.exit_code)
    }
}

pub(crate) fn make_executor(state: &Engine) -> Option<Arc<dyn crate::runtime::barrier::Executor>> {
    Some(Arc::new(OperationExecutor {
        container: Arc::clone(&state.driver.container),
    }))
}
