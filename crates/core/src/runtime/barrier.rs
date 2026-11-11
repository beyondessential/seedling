pub mod action_call;
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

// spec: OPS#operation
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

// spec: SUSP#barriers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarrierCondition {
    pub resources: Vec<ResourceInstance>,
    pub required_state: LifecycleState,
    /// `None` means the barrier has no deadline (waits indefinitely, resumed
    /// only when the condition is satisfied or the operation is cancelled).
    // spec: SUSP#deadlines
    #[serde(default)]
    pub deadline_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallKind {
    Start,
    Stop,
    Query,
    /// `rt.warm_certs(...)` — pre-provision TLS certificates without routing
    /// traffic. Records intent without affecting the standard desired state.
    // spec: ACTU#certificate-warming
    WarmCerts,
    /// `rt.warm_images(...)` — pre-pull container images without running them,
    /// pinning the references for autonomous-GC exemption. The image refs are
    /// not stored on the log entry: pins persist directly to `image_pins`
    /// at call time, which is what the reconciler and barrier consult.
    // spec: IMG#image-warming
    WarmImages,
    /// `rt.signal(...)` — deliver a POSIX signal to one or more container
    /// instances. The signal name is stored separately on the entry; on
    /// replay, an already-committed signal is not re-sent.
    // spec: RT#signal-delivery
    Signal,
    /// `rt.write(...)` — write a file into a volume at action runtime. The
    /// target volume is recorded as the entry's single resource; the path is
    /// stored in `extra`. On replay, an already-committed write is not
    /// re-executed.
    // spec: RT#volume-writes
    Write,
    /// `rt.exec(...)` — run a command inside a running container at action
    /// runtime. The target container instance is recorded as the entry's
    /// single resource; the exit code is stored in `extra` (decimal). On
    /// replay, an already-committed exec is not re-executed; the recorded
    /// exit code is returned through the Executed handle.
    // spec: RT#command-execution
    Exec,
    /// `Action.call(params?)` — a sub-action invocation. The entry has no
    /// resources; `extra` carries a JSON payload `{"action": <name>,
    /// "params": <validated map>}` so replay can recover the called
    /// action's name and the post-validation params without re-running
    /// validation (whose result must be deterministic across replays per
    /// `OPS#sub-action-parameters`).
    // spec: OPS#action-composition
    // spec: HIST#action-calls
    SubAction,
}

// spec: HIST#action-calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLogEntry {
    pub call_index: usize,
    pub call_kind: CallKind,
    pub resources: Vec<ResourceInstance>,
    /// The barriers asserted on this call, each under its own
    /// [`BarrierRecord::seq`].
    ///
    /// A call carries as many as the closure asserts on it: `s.running()`
    /// then `s.ready()` are two, and a group a collection method narrowed
    /// is another. They were one record between them until each got its
    /// own, which meant whichever wrote last displaced the rest — so a
    /// barrier could take another's passing for its own, or lose the start
    /// its deadline was measured from.
    // spec: SUSP#barrier-anchoring
    #[serde(default, alias = "barrier", deserialize_with = "de_barriers")]
    pub barriers: Vec<BarrierRecord>,
    /// Per-call_kind metadata. For `CallKind::Signal` this carries the
    /// canonical signal name (e.g. `"SIGHUP"`). Other kinds leave it `None`.
    // spec: RT#signal-delivery
    #[serde(default)]
    pub extra: Option<String>,
}

impl ActionLogEntry {
    // spec: SUSP#barrier-anchoring
    /// The record of the `seq`th barrier on this call, if it has one.
    pub fn barrier(&self, seq: usize) -> Option<&BarrierRecord> {
        self.barriers.iter().find(|b| b.seq == seq)
    }

    /// Record `record` against the barrier it names, replacing any record
    /// that barrier already had and leaving the others alone.
    pub fn set_barrier(&mut self, record: BarrierRecord) {
        match self.barriers.iter_mut().find(|b| b.seq == record.seq) {
            Some(existing) => *existing = record,
            None => self.barriers.push(record),
        }
    }
}

/// Reads a `barriers` list, and also a lone `barrier` as written before
/// an entry could hold more than one.
fn de_barriers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<BarrierRecord>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<BarrierRecord>),
        One(Option<BarrierRecord>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::Many(v) => v,
        OneOrMany::One(Some(b)) => vec![b],
        OneOrMany::One(None) => Vec::new(),
    })
}

// spec: SUSP#deadlines
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarrierRecord {
    /// Which barrier on the call this is: the position of the barrier
    /// among those the closure asserts on that call, counted from zero.
    ///
    /// The closure makes the same calls in the same order on every pass,
    /// so a barrier is at the same position on each, and this names it
    /// without depending on what it waits for. Its resources are not usable
    /// for that — a replica may come or go between passes and it is still
    /// the same barrier — and neither is its required state, since nothing
    /// stops a closure asserting the same state twice on one call.
    // spec: SUSP#barrier-anchoring
    #[serde(default)]
    pub seq: usize,
    pub required_state: LifecycleState,
    /// `None` means the barrier has no deadline.
    #[serde(default)]
    pub deadline_secs: Option<u64>,
    pub satisfied: bool,
    pub started_at_secs: Option<u64>,
}

// spec: SUSP#replay
/// The committed log does not describe the calls the closure is making.
///
/// Means the script changed between the crash and the replay, or the engine
/// took a different branch. Either way the recorded results cannot be
/// attributed to the calls now being made.
// spec: SUSP#positional-replay
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayMismatch {
    /// A different kind of call is being made at this position.
    Kind {
        call_index: usize,
        expected: CallKind,
        found: CallKind,
    },
    /// The right kind of call, but its recorded argument differs.
    ///
    /// Only for arguments that are stable across passes by construction. The
    /// resolved instance set is *not* one of those — a replica may be added
    /// or retired between passes and it is still the same call — but a
    /// literal like a signal name is: it comes from the script text, so a
    /// change means the script changed under the log.
    Extra {
        call_index: usize,
        kind: CallKind,
        expected: String,
        found: Option<String>,
    },
}

impl ReplayMismatch {
    pub fn call_index(&self) -> usize {
        match self {
            Self::Kind { call_index, .. } | Self::Extra { call_index, .. } => *call_index,
        }
    }
}

impl std::fmt::Display for ReplayMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kind {
                call_index,
                expected,
                found,
            } => write!(
                f,
                "replay diverged at call {call_index}: the log records a {found:?} but the \
                 script is making a {expected:?}"
            ),
            // This message is the whole explanation an operator gets for why
            // an operation refused to resume, so it prints the recorded
            // argument itself rather than its `Option` wrapper.
            Self::Extra {
                call_index,
                kind,
                expected,
                found: Some(found),
            } => write!(
                f,
                "replay diverged at call {call_index}: the log records a {kind:?} of `{found}` \
                 but the script is making one of `{expected}`"
            ),
            Self::Extra {
                call_index,
                kind,
                expected,
                found: None,
            } => write!(
                f,
                "replay diverged at call {call_index}: the log records a {kind:?} with no \
                 recorded argument but the script is making one of `{expected}`"
            ),
        }
    }
}

pub struct ReplayContext {
    pub operation_id: OperationId,
    pub call_index: usize,
    pub committed: Vec<ActionLogEntry>,
    pub pending: Vec<ActionLogEntry>,
    /// How many barriers each call has had asserted on it so far this pass,
    /// which is where the next one's position comes from.
    // spec: SUSP#barrier-anchoring
    pub barrier_seq: std::collections::HashMap<usize, usize>,
    pub pending_barrier: Option<BarrierCondition>,
    pub now_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub world: Arc<dyn oracle::WorldStateOracle>,
    /// Cancellation signal for the current operation. Checked at the entry of
    /// every barrier / stop call so an in-flight cancel aborts cleanly instead
    /// of waiting for the next deadline.
    // spec: OPS#cancellation
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
    // spec: IMG#image-discovery
    pub probe_images: Option<Arc<Mutex<std::collections::BTreeSet<String>>>>,
    /// Hook for `rt.signal()`. The runtime calls this synchronously to deliver
    /// a POSIX signal to a running container. `None` in test / stub contexts
    /// where no real container runtime is present.
    // spec: RT#signal-delivery
    pub container_signaler: Option<Arc<dyn ContainerSignaler>>,
    /// Hook for `rt.write()`. The runtime calls this synchronously to write a
    /// file into a volume during action execution. `None` in test / stub
    /// contexts where no real filesystem is involved.
    // spec: RT#volume-writes
    pub volume_writer: Option<Arc<dyn VolumeWriter>>,
    /// Hook for `rt.exec()`. The runtime calls this synchronously to run a
    /// command inside a running container during action execution. `None` in
    /// test / stub contexts where no real container runtime is present.
    // spec: RT#command-execution
    pub executor: Option<Arc<dyn Executor>>,
}

/// Synchronous side-effect handle the BSL `rt.signal` call uses to actually
/// deliver a signal to a running container. Implemented in the operation
/// loop (`oi/handler/actions/lifecycle.rs`) on top of the system actuator;
/// stubbed out in language-only tests where no real runtime exists.
// spec: RT#signal-delivery
pub trait ContainerSignaler: Send + Sync {
    /// Deliver `signal` to the named container's PID 1.
    /// Returns `Ok(true)` when the signal was sent, `Ok(false)` when the
    /// container was already gone (no error condition for replay safety).
    fn signal(&self, container_name: &str, signal: &str) -> Result<bool, String>;
}

/// Identifies which volume a runtime-time `rt.write` should land in. Resolved
/// to a host path by the [`VolumeWriter`] impl in the operation loop.
// spec: RT#volume-writes
#[derive(Debug, Clone)]
pub enum VolumeWriteTarget {
    /// A named static volume scoped to the current app.
    NamedVolume { name: String, tmpfs: bool },
    /// An anonymous volume created earlier in the action closure.
    AnonymousVolume { anon_id: String, tmpfs: bool },
    /// An external volume bound by the operation (`ACN#volume-params`).
    ExternalBound { host_path: std::path::PathBuf },
}

/// Synchronous side-effect handle the BSL `rt.write` call uses to materialise
/// a file into a volume at action runtime. Implemented in the operation loop
/// on top of the system actuator's `safe_volume_write`; stubbed out in
/// language-only tests where no real filesystem is involved.
// spec: RT#volume-writes
pub trait VolumeWriter: Send + Sync {
    /// Resolve `target` to a host path and write `contents` to `path` within
    /// it. The implementation must enforce `openat2(RESOLVE_BENEATH)`-style
    /// path confinement so the write cannot escape the volume root.
    fn write(
        &self,
        app: &str,
        target: VolumeWriteTarget,
        path: &str,
        contents: &[u8],
    ) -> Result<(), String>;
}

/// Synchronous side-effect handle the BSL `rt.exec` call uses to run a
/// command inside a running container at action runtime. Implemented in the
/// operation loop on top of `ContainerRuntime::exec_command`; stubbed out in
/// language-only tests where no real container runtime is present.
// spec: RT#command-execution
pub trait Executor: Send + Sync {
    /// Run `argv` inside the running container `name`, layering `extra_env`
    /// on top of the container's environment. Blocks until the command exits
    /// and returns the exit code.
    fn exec(
        &self,
        name: &str,
        argv: &[String],
        extra_env: &[(String, String)],
    ) -> Result<i32, String>;
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
            barrier_seq: std::collections::HashMap::new(),
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
            executor: None,
        }
    }

    /// Construct a `ReplayContext` configured for probe execution: no
    /// committed entries, no real oracle, and an image-capture buffer.
    // spec: IMG#image-discovery
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

    // spec: SUSP#barrier-anchoring
    /// Hand out the position of the next barrier asserted on `call_index`.
    ///
    /// Counted afresh each pass, which is what makes it the same number for
    /// the same barrier every time: the closure is re-executed from the
    /// start and makes the same calls in the same order.
    pub fn next_barrier_seq(&mut self, call_index: usize) -> usize {
        let next = self.barrier_seq.entry(call_index).or_default();
        let seq = *next;
        *next += 1;
        seq
    }

    pub fn committed_entry(&self) -> Option<&ActionLogEntry> {
        self.committed.get(self.call_index)
    }

    /// Consume the committed entry for the call being made *at this position*,
    /// or `None` when this call is running for the first time.
    ///
    /// The action log is positional: `call_index` walks `committed` in the
    /// order the closure makes its calls. `do_exec` always understood that;
    /// `do_signal` did not, and scanned the whole log for any entry with the
    /// same resources and signal. Both halves of that are wrong. A second,
    /// identical `rt.signal` later in the same closure matched the first
    /// entry and was swallowed — the signal was never delivered. And when the
    /// resolved instance set changed between passes, no entry matched, so a
    /// signal already delivered before the crash was delivered again.
    ///
    /// Advancing the index is part of consuming the entry, so a caller cannot
    /// check without advancing or advance without checking.
    // spec: SUSP#positional-replay
    /// `expect_extra` is checked when the caller's argument is stable across
    /// passes by construction; pass `None` to skip the check. Positional
    /// matching alone would treat a script edit that changes the argument at
    /// this position — `SIGHUP` to `SIGTERM`, say — as already replayed, and
    /// silently never deliver the new one.
    pub fn replay_step(
        &mut self,
        expect: CallKind,
        expect_extra: Option<&str>,
    ) -> Result<Option<ActionLogEntry>, ReplayMismatch> {
        if !self.is_replaying() {
            return Ok(None);
        }
        let entry = self.committed[self.call_index].clone();
        if entry.call_kind != expect {
            return Err(ReplayMismatch::Kind {
                call_index: self.call_index,
                expected: expect,
                found: entry.call_kind,
            });
        }
        if let Some(expected) = expect_extra
            && entry.extra.as_deref() != Some(expected)
        {
            return Err(ReplayMismatch::Extra {
                call_index: self.call_index,
                kind: expect,
                expected: expected.to_owned(),
                found: entry.extra.clone(),
            });
        }
        self.call_index += 1;
        Ok(Some(entry))
    }

    pub fn take_pending(&mut self) -> Vec<ActionLogEntry> {
        std::mem::take(&mut self.pending)
    }
}

pub type SharedContext = Arc<Mutex<ReplayContext>>;
