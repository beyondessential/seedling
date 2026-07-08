//! Erroring backend implementations used when the container runtime (podman)
//! or process manager (systemd) could not be reached at startup. They let the
//! daemon come up in a degraded mode — the operator interface, database, and
//! scheduler run and surface a system-wide fault — instead of crash-looping.
//! Every method fails loudly; unlike the stub backends, they never fake
//! success (which would make workloads look running when they are not).

use ipnet::{Ipv4Net, Ipv6Net};

use crate::system::{
    BoxError, BoxFuture, ContainerRuntime, ProcessManager,
    types::{
        ContainerFilter, ContainerSpec, ContainerState, ContainerSummary, ExecHandle, ImageSummary,
        NetworkSummary, TransientUnitSpec, UnitState, UnitSummary,
    },
};

fn unavailable() -> BoxError {
    "container/process backend unavailable: the podman/systemd connection failed at startup".into()
}

/// A `ContainerRuntime` whose every operation fails with an "unavailable" error.
pub(crate) struct UnavailableContainerRuntime;

#[expect(
    unused_variables,
    reason = "every method ignores its arguments and errors"
)]
impl ContainerRuntime for UnavailableContainerRuntime {
    fn inspect<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<ContainerState>, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn list<'a>(
        &'a self,
        filter: ContainerFilter<'a>,
    ) -> BoxFuture<'a, Result<Vec<ContainerSummary>, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn image_exists<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn pull_image<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn local_image_id<'a>(
        &'a self,
        reference: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn list_images<'a>(&'a self) -> BoxFuture<'a, Result<Vec<ImageSummary>, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn remove_image<'a>(
        &'a self,
        reference: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn network_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn create_network<'a>(
        &'a self,
        name: &'a str,
        prefix: Ipv6Net,
        ipv4: Option<Ipv4Net>,
    ) -> BoxFuture<'a, Result<String, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn remove_network<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn list_networks<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<NetworkSummary>, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn volume_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn create_volume<'a>(
        &'a self,
        name: &'a str,
        tmpfs: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn remove_volume<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn list_volumes_by_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn volume_mountpoint<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<std::path::PathBuf, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn remove_container<'a>(
        &'a self,
        name: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn exec<'a>(&'a self, spec: ContainerSpec) -> BoxFuture<'a, Result<ExecHandle, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn signal_container<'a>(
        &'a self,
        name: &'a str,
        signal: &'a str,
    ) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn exec_command<'a>(
        &'a self,
        name: &'a str,
        argv: &'a [String],
        extra_env: &'a [(String, String)],
    ) -> BoxFuture<'a, Result<i32, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }
}

/// A `ProcessManager` whose every operation fails with an "unavailable" error.
pub(crate) struct UnavailableProcessManager;

#[expect(
    unused_variables,
    reason = "every method ignores its arguments and errors"
)]
impl ProcessManager for UnavailableProcessManager {
    fn start_transient<'a>(
        &'a self,
        spec: TransientUnitSpec,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn stop_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn reset_failed_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn unit_state<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<UnitState>, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn list_units<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<UnitSummary>, BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn write_unit<'a>(
        &'a self,
        name: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn remove_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn daemon_reload<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }

    fn start_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(unavailable()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::types::ContainerFilter;

    // r[verify infra.node.degraded]
    // The degraded-mode backends must fail loudly on every operation rather
    // than fake success — otherwise workloads would appear to run while nothing
    // is actually actuated, defeating the point of the operator-visible fault.
    #[tokio::test]
    async fn unavailable_container_runtime_errors() {
        let rt = UnavailableContainerRuntime;
        assert!(rt.image_exists("busybox").await.is_err());
        assert!(rt.list(ContainerFilter::default()).await.is_err());
        assert!(rt.network_exists("seedling-proxy").await.is_err());
        assert!(rt.volume_exists("vol").await.is_err());
    }

    #[tokio::test]
    async fn unavailable_process_manager_errors() {
        let pm = UnavailableProcessManager;
        assert!(pm.unit_state("seedling-x.service").await.is_err());
        assert!(pm.list_units("seedling-").await.is_err());
        assert!(pm.daemon_reload().await.is_err());
    }
}
