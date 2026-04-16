// The runtime module is not yet wired to the reconciliation loop binary.
// All items here are production infrastructure, not dead code.
#![allow(dead_code, unused_imports)]

pub mod apps;
pub mod audit;
pub mod barrier;
pub mod db;
pub mod desired;
pub mod faults;
pub mod gc;
pub mod history;
pub mod identity;
pub mod lifecycle;
pub mod registries;
pub mod registry;
pub mod scaling;
pub mod scheduler;
pub mod site_volumes;

pub use apps::{AppEntry, AppPhase, AppRegistry, AppStatus, ScriptError, transition_phase};
pub use barrier::oracle::{DbWorldOracle, TestWorldOracle, WorldStateOracle};
pub use barrier::replay::{
    ActionLog, DbActionLog, InMemoryActionLog, OperationResult, run_operation,
};
pub use desired::{
    DesiredResource, DesiredState, OperationProgress, compute, compute_uninstalling,
};
pub use identity::{InstanceId, InstanceVariant, ResourceInstance};
pub use lifecycle::LifecycleState;
pub use registry::{
    DbInstanceRegistry, EphemeralInstanceRegistry, InstanceRegistry, RegistryError,
};
pub use scheduler::{
    ActiveOperation, CycleError, QueuedOperation, RejectReason, ScheduleResult, Scheduler,
    should_back_off,
};
