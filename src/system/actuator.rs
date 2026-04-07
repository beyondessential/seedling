use snafu::Snafu;

use crate::{
    defs::resource::{Resource, ResourceKind},
    runtime::identity::ResourceInstance,
    system::{ContainerRuntime, DataPlane, NetworkProxy, ProcessManager, SystemDriver},
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
}

// ---------------------------------------------------------------------------
// Actuator
// ---------------------------------------------------------------------------

pub struct Actuator<C, P, N, D> {
    driver: SystemDriver<C, P, N, D>,
}

impl<C, P, N, D> Actuator<C, P, N, D>
where
    C: ContainerRuntime,
    P: ProcessManager,
    N: NetworkProxy,
    D: DataPlane,
{
    pub fn new(driver: SystemDriver<C, P, N, D>) -> Self {
        Self { driver }
    }

    /// Ensure all primitives for this instance exist and are running.
    pub async fn start(
        &self,
        _instance: &ResourceInstance,
        _resource: &Resource,
    ) -> Result<(), ActuateError> {
        todo!("actuator start: not yet implemented")
    }

    /// Stop and remove all primitives for this instance.
    pub async fn stop(
        &self,
        _instance: &ResourceInstance,
        _resource: &Resource,
    ) -> Result<(), ActuateError> {
        todo!("actuator stop: not yet implemented")
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
}
