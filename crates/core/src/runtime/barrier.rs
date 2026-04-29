pub mod cancel;
pub mod oracle;
pub mod replay;
pub mod runtime;
pub mod shell;

use crate::runtime::{LifecycleState, ResourceInstance};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

pub use cancel::CancelToken;

// r[impl operation.lifecycle]
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

// r[impl barrier.condition]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarrierCondition {
    pub resources: Vec<ResourceInstance>,
    pub required_state: LifecycleState,
    /// `None` means the barrier has no deadline (waits indefinitely, resumed
    /// only when the condition is satisfied or the operation is cancelled).
    // r[impl barrier.deadline]
    #[serde(default)]
    pub deadline_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallKind {
    Start,
    Stop,
    Query,
    /// `rt.warm_certs(...)` — pre-provision TLS certificates without routing
    /// traffic. Records intent without affecting the standard desired state.
    // r[impl actuate.ingress.warm-certs]
    WarmCerts,
    /// `rt.warm_images(...)` — pre-pull container images without running them,
    /// pinning the references for autonomous-GC exemption. The image refs are
    /// not stored on the log entry: pins persist directly to `image_pins`
    /// at call time, which is what the reconciler and barrier consult.
    // r[impl actuate.image.warm]
    WarmImages,
    /// `rt.signal(...)` — deliver a POSIX signal to one or more container
    /// instances. The signal name is stored separately on the entry; on
    /// replay, an already-committed signal is not re-sent.
    // l[impl rt.signal]
    Signal,
    /// `rt.write(...)` — write a file into a volume at action runtime. The
    /// target volume is recorded as the entry's single resource; the path is
    /// stored in `extra`. On replay, an already-committed write is not
    /// re-executed.
    // l[impl rt.write]
    Write,
}

// r[impl history.action-log.entries]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLogEntry {
    pub call_index: usize,
    pub call_kind: CallKind,
    pub resources: Vec<ResourceInstance>,
    pub barrier: Option<BarrierRecord>,
    /// Per-call_kind metadata. For `CallKind::Signal` this carries the
    /// canonical signal name (e.g. `"SIGHUP"`). Other kinds leave it `None`.
    // l[impl rt.signal]
    #[serde(default)]
    pub extra: Option<String>,
}

// r[impl barrier.deadline]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarrierRecord {
    pub required_state: LifecycleState,
    /// `None` means the barrier has no deadline.
    #[serde(default)]
    pub deadline_secs: Option<u64>,
    pub satisfied: bool,
    pub started_at_secs: Option<u64>,
}

// r[impl barrier.replay]
pub struct ReplayContext {
    pub operation_id: OperationId,
    pub call_index: usize,
    pub committed: Vec<ActionLogEntry>,
    pub pending: Vec<ActionLogEntry>,
    pub pending_barrier: Option<BarrierCondition>,
    pub now_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub world: Arc<dyn oracle::WorldStateOracle>,
    /// Cancellation signal for the current operation. Checked at the entry of
    /// every barrier / stop call so an in-flight cancel aborts cleanly instead
    /// of waiting for the next deadline.
    // r[impl operation.cancel]
    pub cancel_token: Arc<CancelToken>,
    /// Definitions of dynamic (anonymous) resources started during this pass.
    /// Populated by rt.start() calls in the action closure; read by the
    /// reconciler to compute desired state for resources not in the static AppDef.
    pub dynamic_defs: std::collections::HashMap<
        crate::runtime::ResourceInstance,
        crate::defs::resource::Resource,
    >,
    /// Counter for assigning stable operation-scoped IDs to anonymous resources.
    /// Incremented each time an anonymous resource instance is created.
    pub anon_counter: u32,
    /// When set, the replay is running in _probe_ mode: all `rt.*` calls that
    /// would normally mutate state or wait on the world are short-circuited,
    /// and image references extracted from `rt.start` / `rt.warm_images`
    /// resources are appended to this set. The call site never needs to
    /// inspect it directly — it uses [`probe_mode`](Self::probe_mode) instead.
    // r[impl image.discover]
    pub probe_images: Option<Arc<Mutex<std::collections::BTreeSet<String>>>>,
    /// Hook for `rt.signal()`. The runtime calls this synchronously to deliver
    /// a POSIX signal to a running container. `None` in test / stub contexts
    /// where no real container runtime is present.
    // l[impl rt.signal]
    pub container_signaler: Option<Arc<dyn ContainerSignaler>>,
    /// Hook for `rt.write()`. The runtime calls this synchronously to write a
    /// file into a volume during action execution. `None` in test / stub
    /// contexts where no real filesystem is involved.
    // l[impl rt.write]
    pub volume_writer: Option<Arc<dyn VolumeWriter>>,
}

/// Synchronous side-effect handle the BSL `rt.signal` call uses to actually
/// deliver a signal to a running container. Implemented in the operation
/// loop (`oi/handler/actions/lifecycle.rs`) on top of the system actuator;
/// stubbed out in language-only tests where no real runtime exists.
// l[impl rt.signal]
pub trait ContainerSignaler: Send + Sync {
    /// Deliver `signal` to the named container's PID 1.
    /// Returns `Ok(true)` when the signal was sent, `Ok(false)` when the
    /// container was already gone (no error condition for replay safety).
    fn signal(&self, container_name: &str, signal: &str) -> Result<bool, String>;
}

/// Identifies which volume a runtime-time `rt.write` should land in. Resolved
/// to a host path by the [`VolumeWriter`] impl in the operation loop.
// l[impl rt.write]
#[derive(Debug, Clone)]
pub enum VolumeWriteTarget {
    /// A named static volume scoped to the current app.
    NamedVolume { name: String, tmpfs: bool },
    /// An anonymous volume created earlier in the action closure.
    AnonymousVolume { anon_id: String, tmpfs: bool },
    /// An external volume bound by the operation (`l[action.params.volume]`).
    ExternalBound { host_path: std::path::PathBuf },
}

/// Synchronous side-effect handle the BSL `rt.write` call uses to materialise
/// a file into a volume at action runtime. Implemented in the operation loop
/// on top of the system actuator's `safe_volume_write`; stubbed out in
/// language-only tests where no real filesystem is involved.
// l[impl rt.write]
pub trait VolumeWriter: Send + Sync {
    /// Resolve `target` to a host path and write `contents` to `path` within
    /// it. The implementation must enforce `openat2(RESOLVE_BENEATH)`-style
    /// path confinement so the write cannot escape the volume root.
    fn write(
        &self,
        app: &str,
        target: VolumeWriteTarget,
        path: &str,
        contents: &str,
    ) -> Result<(), String>;
}

impl fmt::Debug for ReplayContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplayContext")
            .field("operation_id", &self.operation_id)
            .field("call_index", &self.call_index)
            .field("committed", &self.committed)
            .field("pending", &self.pending)
            .field("pending_barrier", &self.pending_barrier)
            .field(
                "dynamic_defs",
                &self.dynamic_defs.keys().collect::<Vec<_>>(),
            )
            .field("anon_counter", &self.anon_counter)
            .finish_non_exhaustive()
    }
}

impl ReplayContext {
    pub fn new(
        operation_id: OperationId,
        committed: Vec<ActionLogEntry>,
        world: Arc<dyn oracle::WorldStateOracle>,
        cancel_token: Arc<CancelToken>,
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
            cancel_token,
            dynamic_defs: std::collections::HashMap::new(),
            anon_counter: 0,
            probe_images: None,
            container_signaler: None,
            volume_writer: None,
        }
    }

    /// Construct a `ReplayContext` configured for probe execution: no
    /// committed entries, no real oracle, and an image-capture buffer.
    // r[impl image.discover]
    pub fn new_probe(
        operation_id: OperationId,
        world: Arc<dyn oracle::WorldStateOracle>,
        cancel_token: Arc<CancelToken>,
        probe_images: Arc<Mutex<std::collections::BTreeSet<String>>>,
    ) -> Self {
        let mut ctx = Self::new(operation_id, Vec::new(), world, cancel_token);
        ctx.probe_images = Some(probe_images);
        ctx
    }

    /// `true` when the replay is running in probe mode.
    pub fn probe_mode(&self) -> bool {
        self.probe_images.is_some()
    }

    pub fn is_replaying(&self) -> bool {
        self.call_index < self.committed.len()
    }

    pub fn committed_entry(&self) -> Option<&ActionLogEntry> {
        self.committed.get(self.call_index)
    }

    pub fn take_pending(&mut self) -> Vec<ActionLogEntry> {
        std::mem::take(&mut self.pending)
    }
}

pub type SharedContext = Arc<Mutex<ReplayContext>>;
