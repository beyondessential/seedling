pub mod oracle;
pub mod replay;
pub mod runtime;

use crate::runtime::{LifecycleState, ResourceInstance};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationId(pub String);

impl OperationId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for OperationId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarrierCondition {
    pub resources: Vec<ResourceInstance>,
    pub required_state: LifecycleState,
    pub deadline_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallKind {
    Start,
    Stop,
    Reconcile,
    Query,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLogEntry {
    pub call_index: usize,
    pub call_kind: CallKind,
    pub resources: Vec<ResourceInstance>,
    pub barrier: Option<BarrierRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarrierRecord {
    pub required_state: LifecycleState,
    pub deadline_secs: u64,
    pub satisfied: bool,
    pub started_at_secs: Option<u64>,
}

pub struct ReplayContext {
    pub operation_id: OperationId,
    pub call_index: usize,
    pub committed: Vec<ActionLogEntry>,
    pub pending: Vec<ActionLogEntry>,
    pub pending_barrier: Option<BarrierCondition>,
    pub now_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub world: Arc<dyn oracle::WorldStateOracle>,
}

impl fmt::Debug for ReplayContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplayContext")
            .field("operation_id", &self.operation_id)
            .field("call_index", &self.call_index)
            .field("committed", &self.committed)
            .field("pending", &self.pending)
            .field("pending_barrier", &self.pending_barrier)
            .finish_non_exhaustive()
    }
}

impl ReplayContext {
    #[cfg_attr(not(test), expect(dead_code, reason = "todo"))]
    pub fn new(
        operation_id: OperationId,
        committed: Vec<ActionLogEntry>,
        world: Arc<dyn oracle::WorldStateOracle>,
    ) -> Self {
        Self {
            operation_id,
            call_index: 0,
            committed,
            pending: Vec::new(),
            pending_barrier: None,
            now_secs: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            }),
            world,
        }
    }

    pub fn is_replaying(&self) -> bool {
        self.call_index < self.committed.len()
    }

    pub fn committed_entry(&self) -> Option<&ActionLogEntry> {
        self.committed.get(self.call_index)
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "todo"))]
    pub fn take_pending(&mut self) -> Vec<ActionLogEntry> {
        std::mem::take(&mut self.pending)
    }
}

pub type SharedContext = Arc<Mutex<ReplayContext>>;
