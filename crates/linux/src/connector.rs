//! How the daemon reaches the host's container runtime and process manager.

use std::sync::Arc;

use futures_util::FutureExt;
use seedling_core::system::{
    BoxError, BoxFuture, ContainerRuntime, ProcessManager, backends::Connector,
};

use crate::{podman::PodmanRuntime, systemd::SystemdManager};

/// Connects to the host's Podman and systemd.
// spec: LINUX#host-facilities
pub struct HostConnector {
    /// The data directory, where the process manager keeps its records.
    pub data_dir: std::path::PathBuf,
}

impl Connector for HostConnector {
    fn container(&self) -> BoxFuture<'_, Result<Arc<dyn ContainerRuntime>, BoxError>> {
        async {
            let podman = PodmanRuntime::new().await?;
            Ok(Arc::new(podman) as Arc<dyn ContainerRuntime>)
        }
        .boxed()
    }

    fn process(&self) -> BoxFuture<'_, Result<Arc<dyn ProcessManager>, BoxError>> {
        async {
            let systemd = SystemdManager::connect(&self.data_dir).await?;
            Ok(Arc::new(systemd) as Arc<dyn ProcessManager>)
        }
        .boxed()
    }
}
