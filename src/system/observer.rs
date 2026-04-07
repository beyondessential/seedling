use std::time::SystemTime;

use snafu::Snafu;

use crate::{
    defs::resource::Resource,
    runtime::identity::ResourceInstance,
    system::{
        ContainerRuntime, DataPlane, NetworkProxy, ProcessManager, SystemDriver,
        types::ObservationFact,
    },
};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Observation-time error. The backend variant is intentionally erased:
/// callers see `ObserveError::Container` but cannot match on internal types.
#[derive(Debug, Snafu)]
pub enum ObserveError {
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
}

// ---------------------------------------------------------------------------
// Observer
// ---------------------------------------------------------------------------

pub struct Observer<C, P, N, D> {
    driver: SystemDriver<C, P, N, D>,
}

impl<C, P, N, D> Observer<C, P, N, D>
where
    C: ContainerRuntime,
    P: ProcessManager,
    N: NetworkProxy,
    D: DataPlane,
{
    pub fn new(driver: SystemDriver<C, P, N, D>) -> Self {
        Self { driver }
    }

    /// Inspect all system primitives backing one resource instance.
    ///
    /// Returns timestamped facts; the reconciler loop persists them to
    /// `world_observations`.
    pub async fn observe(
        &self,
        _instance: &ResourceInstance,
        _resource: &Resource,
    ) -> Result<Vec<(ObservationFact, SystemTime)>, ObserveError> {
        todo!("observe: not yet implemented")
    }
}
