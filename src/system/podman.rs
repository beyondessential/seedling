use ipnet::Ipv6Net;
use snafu::Snafu;

use crate::system::{
    ContainerRuntime,
    types::{ContainerFilter, ContainerState, ContainerSummary, ExecHandle, ExecSpec},
};

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

#[derive(Debug, Snafu)]
pub(crate) enum PodmanError {
    #[snafu(display("libpod API error: {message}"))]
    Api { message: String },
    #[snafu(display("I/O error communicating with podman socket: {source}"))]
    Io { source: std::io::Error },
    #[snafu(display("unexpected response from podman: {message}"))]
    Protocol { message: String },
}

// ---------------------------------------------------------------------------
// PodmanRuntime
// ---------------------------------------------------------------------------

/// `ContainerRuntime` implementation backed by the podman libpod REST API
/// over a unix socket at `/run/podman/podman.sock`.
///
/// Uses the `podman-rest-client` crate (uds feature) pinned to API v5.
/// All methods are currently stubs; implement incrementally starting with
/// `inspect`, `list`, and `create_network`.
pub(crate) struct PodmanRuntime {
    // TODO: add podman_rest_client::PodmanRestClient field once the
    // `podman-rest-client` crate dependency is added.
    _private: (),
}

impl PodmanRuntime {
    /// Connect to the default podman socket at `/run/podman/podman.sock`.
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }
}

impl ContainerRuntime for PodmanRuntime {
    type Error = PodmanError;

    async fn inspect(&self, _name: &str) -> Result<Option<ContainerState>, Self::Error> {
        todo!("PodmanRuntime::inspect")
    }

    async fn list(
        &self,
        _filter: ContainerFilter<'_>,
    ) -> Result<Vec<ContainerSummary>, Self::Error> {
        todo!("PodmanRuntime::list")
    }

    async fn image_exists(&self, _reference: &str) -> Result<bool, Self::Error> {
        todo!("PodmanRuntime::image_exists")
    }

    async fn pull_image(&self, _reference: &str) -> Result<(), Self::Error> {
        todo!("PodmanRuntime::pull_image")
    }

    async fn network_exists(&self, _name: &str) -> Result<bool, Self::Error> {
        todo!("PodmanRuntime::network_exists")
    }

    async fn create_network(&self, _name: &str, _prefix: Ipv6Net) -> Result<(), Self::Error> {
        todo!("PodmanRuntime::create_network")
    }

    async fn remove_network(&self, _name: &str) -> Result<(), Self::Error> {
        todo!("PodmanRuntime::remove_network")
    }

    async fn volume_exists(&self, _name: &str) -> Result<bool, Self::Error> {
        todo!("PodmanRuntime::volume_exists")
    }

    async fn create_volume(&self, _name: &str) -> Result<(), Self::Error> {
        todo!("PodmanRuntime::create_volume")
    }

    async fn remove_volume(&self, _name: &str) -> Result<(), Self::Error> {
        todo!("PodmanRuntime::remove_volume")
    }

    async fn remove_container(&self, _name: &str, _force: bool) -> Result<(), Self::Error> {
        todo!("PodmanRuntime::remove_container")
    }

    async fn exec(&self, _name: &str, _spec: ExecSpec) -> Result<ExecHandle, Self::Error> {
        todo!("PodmanRuntime::exec")
    }
}
