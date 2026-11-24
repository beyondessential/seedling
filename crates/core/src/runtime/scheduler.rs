use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use seedling_protocol::events::OperationTarget;
use seedling_protocol::names::{ActionName, AppName};

use crate::runtime::apps::AppPhase;
use crate::runtime::barrier::{CancelToken, OperationId};
use crate::runtime::history::AutonomousOperation;
use crate::runtime::identity::InstanceId;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

// spec: OPS#operation
#[derive(Debug, Clone)]
pub struct ActiveOperation {
    pub app: AppName,
    /// The action it runs, or the gate whose run it is.
    pub target: OperationTarget,
    pub operation_id: OperationId,
    /// The generation current when the operation was dispatched.
    // spec: OPS#initiating-events
    pub generation: u64,
    /// Cooperative cancellation signal. Flipped by the cancel-action
    /// endpoint; observed by the action runtime at barrier entry and by the
    /// lifecycle loop's interruptible sleep.
    // spec: OPS#cancellation
    pub cancel_token: Arc<CancelToken>,
}

// spec: OPS#operation
#[derive(Debug, Clone)]
pub struct QueuedOperation {
    pub app: AppName,
    pub target: OperationTarget,
    pub operation_id: OperationId,
    /// Action params passed by the invoker. Empty map for actions with no
    /// params, and for a gate run.
    // spec: OPS#persistence
    pub params: crate::runtime::param_map::ParamMap,
    // spec: OPS#initiating-events
    pub generation: u64,
    // spec: EVT#event-types
    pub trigger: String,
    /// Who asked for the operation, where a person or client did; its events
    /// carry them when it starts.
    // spec: WIRE#actor-attribution
    pub actor: Option<Arc<seedling_protocol::actor::Actor>>,
}

// spec: OPS#concurrency
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleResult {
    /// Operation started immediately (no prior active op).
    Accepted,
    /// Operation added to the queue (another app was active).
    Queued,
    /// The request was rejected.
    Rejected(RejectReason),
}

// spec: OPS#cancellation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The token was flipped; the action runtime will observe the cancel at
    /// its next barrier or while waiting between replay cycles. Carries the
    /// operation_id so the caller can persist the cancel flag against the
    /// right row.
    Cancelled(OperationId),
    /// A cancel has already been requested for this operation; the call is a
    /// no-op but not an error. Carries the operation_id so the caller can
    /// re-persist the flag idempotently if a prior persist failed.
    AlreadyCancelled(OperationId),
    /// No operation is currently active for the named app.
    NoActiveOp,
}

// spec: OPS#concurrency
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// A lifecycle operation for this application is already in progress.
    SameAppOperationInProgress,
    /// A lifecycle operation for this application is already queued.
    SameAppAlreadyQueued,
    // spec: TRD#uninstall-admission
    /// The application is being torn down, so nothing further may run
    /// against it.
    AppUninstalling,
    /// A change to the application is being applied; see [`AppHold`].
    ChangeBeingApplied,
    // spec: OPS#unreadable-operation-state
    /// The persisted operation, its cancel or the persisted queue cannot be
    /// read, so which operation is in progress, or waiting, is not known.
    OperationStateUnreadable,
}

/// What keeps an operation from being admitted for an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppBlocker {
    /// The action or gate run is the scheduler's active operation.
    Active(OperationTarget),
    /// The action or gate run is queued behind another app's operation.
    Queued(OperationTarget),
    /// A change to the app is being applied; see [`AppHold`].
    Change,
}

impl std::fmt::Display for AppBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active(OperationTarget::Action(action)) => {
                write!(f, "'{action}' is in progress")
            }
            Self::Active(OperationTarget::Gate(gate)) => {
                write!(f, "a run of gate '{gate}' is in progress")
            }
            Self::Queued(OperationTarget::Action(action)) => write!(f, "'{action}' is queued"),
            Self::Queued(OperationTarget::Gate(gate)) => {
                write!(f, "a run of gate '{gate}' is queued")
            }
            Self::Change => f.write_str("a change to it is being applied"),
        }
    }
}

// spec: OPS#cycles
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleError {
    /// The action whose invocation would form a cycle.
    pub action: String,
    /// The call stack at the point the cycle was detected.
    pub stack: Vec<String>,
}

impl fmt::Display for CycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "action '{}' forms a cycle in the call stack {:?}",
            self.action, self.stack
        )
    }
}

impl std::error::Error for CycleError {}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// Enforces the single-active-operation rule, per-app queuing, and cycle
/// detection for action composition. The scheduler tracks policy only; it
/// does not execute operations.
// spec: OPS#concurrency
#[derive(Debug, Default)]
pub struct Scheduler {
    active: Option<ActiveOperation>,
    /// Pending operations in insertion (request) order; at most one per app.
    queue: VecDeque<QueuedOperation>,
    /// The persisted operation state could not be read, so no operation is
    /// started until it can be.
    // spec: OPS#unreadable-operation-state
    state_unreadable: bool,
    /// Inline action invocation chain for the active operation.
    call_stack: Vec<String>,
    /// Apps held against admission while a change to them is committed; see
    /// [`Self::hold`].
    held: HashSet<AppName>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently active operation, if any.
    /// Whether the persisted operation state is unreadable, so that no
    /// operation may start.
    pub fn is_state_unreadable(&self) -> bool {
        self.state_unreadable
    }

    // spec: OPS#unreadable-operation-state
    pub fn set_state_unreadable(&mut self, unreadable: bool) {
        self.state_unreadable = unreadable;
    }

    pub fn active(&self) -> Option<&ActiveOperation> {
        self.active.as_ref()
    }

    /// Iterator over queued operations.
    pub fn queue_iter(&self) -> impl Iterator<Item = &QueuedOperation> {
        self.queue.iter()
    }

    /// What keeps an operation from being admitted for `app`: its active or
    /// queued operation, or a change to it that holds it.
    pub fn blocker_for(&self, app: &AppName) -> Option<AppBlocker> {
        if let Some(active) = self.active.as_ref().filter(|a| a.app == *app) {
            return Some(AppBlocker::Active(active.target.clone()));
        }
        if let Some(queued) = self.queue.iter().find(|q| q.app == *app) {
            return Some(AppBlocker::Queued(queued.target.clone()));
        }
        self.held.contains(app).then_some(AppBlocker::Change)
    }

    // spec: OPS#changes-to-an-app
    /// Hold `app` against operation admission while a change to it is
    /// applied.
    ///
    /// Changes to an app apply one at a time, so a change is refused while
    /// another holds the app. An operation already running or queued for the
    /// app does not refuse it: that operation goes on against its own
    /// generation.
    fn hold(&mut self, app: &AppName) -> Result<(), AppBlocker> {
        if self.held.insert(app.clone()) {
            Ok(())
        } else {
            Err(AppBlocker::Change)
        }
    }

    // spec: TRD#uninstall-admission
    /// Admit a lifecycle operation for `app` / `target`, subject to its phase.
    ///
    /// The only way in from outside this module. [`Self::request`] is the queue
    /// mechanic underneath and stays private, so the phase gate cannot be
    /// skipped by a caller that does not know to make it: a caller that has not
    /// said what phase the app is in has not said enough to be admitted. Each
    /// call site that admits an operation — an operator action and a scheduled
    /// firing — would otherwise have to apply a rule for itself.
    ///
    /// `phase` is read under the scheduler's own lock, so an uninstall that
    /// transitions the app while holding that lock falls wholly before or
    /// wholly after this decision, never inside it.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors request() signature plus the phase it decides on"
    )]
    pub fn admit(
        &mut self,
        app: &AppName,
        target: &OperationTarget,
        params: crate::runtime::param_map::ParamMap,
        generation: u64,
        trigger: &str,
        phase: &Mutex<AppPhase>,
        actor: Option<Arc<seedling_protocol::actor::Actor>>,
    ) -> ScheduleResult {
        if matches!(*phase.lock(), AppPhase::Uninstalling) {
            return ScheduleResult::Rejected(RejectReason::AppUninstalling);
        }
        self.request(app, target, params, generation, trigger, actor)
    }

    /// Request a lifecycle operation for `app` / `target`.
    ///
    /// - If no operation is active: the operation starts immediately.
    /// - If a different app is active and this app is not yet queued: queued.
    /// - If this app is active or already queued: rejected.
    ///
    /// The queue mechanic alone, with no view of the app's phase: reach it
    /// through [`Self::admit`], which decides whether the app may have one.
    // spec: OPS#concurrency
    fn request(
        &mut self,
        app: &AppName,
        target: &OperationTarget,
        params: crate::runtime::param_map::ParamMap,
        generation: u64,
        trigger: &str,
        actor: Option<Arc<seedling_protocol::actor::Actor>>,
    ) -> ScheduleResult {
        self.request_with_id(
            app,
            target,
            params,
            generation,
            trigger,
            OperationId::new(),
            actor,
        )
    }

    /// Like [`request`], but uses a caller-provided `operation_id` instead of
    /// generating a new one. Use this when the ID must be known before the
    /// scheduler slot is acquired (e.g. to return it to an API caller).
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors request() signature plus operation_id"
    )]
    pub fn request_with_id(
        &mut self,
        app: &AppName,
        target: &OperationTarget,
        params: crate::runtime::param_map::ParamMap,
        generation: u64,
        trigger: &str,
        operation_id: OperationId,
        actor: Option<Arc<seedling_protocol::actor::Actor>>,
    ) -> ScheduleResult {
        // spec: OPS#unreadable-operation-state
        if self.state_unreadable {
            return ScheduleResult::Rejected(RejectReason::OperationStateUnreadable);
        }
        if self.held.contains(app) {
            return ScheduleResult::Rejected(RejectReason::ChangeBeingApplied);
        }
        match &self.active {
            None => {
                // No active operation — start immediately.
                self.call_stack.clear();
                self.active = Some(ActiveOperation {
                    app: app.clone(),
                    target: target.clone(),
                    operation_id,
                    generation,
                    cancel_token: Arc::new(CancelToken::new()),
                });
                ScheduleResult::Accepted
            }
            Some(active) if active.app == *app => {
                // spec: OPS#concurrency
                ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress)
            }
            Some(_) => {
                // spec: OPS#concurrency
                if self.queue.iter().any(|q| q.app == *app) {
                    return ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued);
                }
                self.queue.push_back(QueuedOperation {
                    app: app.clone(),
                    target: target.clone(),
                    operation_id,
                    params,
                    generation,
                    trigger: trigger.to_owned(),
                    actor,
                });
                ScheduleResult::Queued
            }
        }
    }

    /// The queued request with `operation_id`, if it is still queued.
    pub fn queued(&self, operation_id: &OperationId) -> Option<&QueuedOperation> {
        self.queue.iter().find(|q| q.operation_id == *operation_id)
    }

    /// Take the queued request `operation_id` back out of the queue, if it is
    /// still there.
    pub fn unqueue(&mut self, operation_id: &OperationId) -> Option<QueuedOperation> {
        let pos = self
            .queue
            .iter()
            .position(|q| q.operation_id == *operation_id)?;
        self.queue.remove(pos)
    }

    /// Mark the current operation as complete and dequeue the next one, if any.
    ///
    /// Returns the newly-started operation so the caller knows what to run,
    /// carrying the generation it runs against: the one `current_generation`
    /// gives for its app, or the one it was requested at where that gives none.
    // spec: OPS#completion
    pub fn complete_current(
        &mut self,
        current_generation: impl FnOnce(&AppName) -> Option<u64>,
    ) -> Option<QueuedOperation> {
        self.active = None;
        self.call_stack.clear();

        let mut next = self.queue.pop_front()?;
        // spec: OPS#initiating-events
        if let Some(current) = current_generation(&next.app) {
            next.generation = current;
        }
        self.active = Some(ActiveOperation {
            app: next.app.clone(),
            target: next.target.clone(),
            operation_id: next.operation_id.clone(),
            generation: next.generation,
            cancel_token: Arc::new(CancelToken::new()),
        });
        Some(next)
    }

    /// Request cancellation of the active operation if it belongs to `app`.
    // spec: OPS#cancellation
    pub fn request_cancel(&self, app: &AppName) -> CancelOutcome {
        let Some(active) = self.active.as_ref() else {
            return CancelOutcome::NoActiveOp;
        };
        if active.app != *app {
            return CancelOutcome::NoActiveOp;
        }
        if active.cancel_token.is_cancelled() {
            return CancelOutcome::AlreadyCancelled(active.operation_id.clone());
        }
        active.cancel_token.request();
        CancelOutcome::Cancelled(active.operation_id.clone())
    }

    /// Push an action name onto the composition call stack.
    ///
    /// Returns `Err(CycleError)` if `action_name` is already on the stack,
    /// which means this invocation would form a direct or transitive cycle.
    // spec: OPS#action-composition
    pub fn push_call(&mut self, action_name: &ActionName) -> Result<(), CycleError> {
        // spec: OPS#cycles
        if self.call_stack.iter().any(|a| a == action_name.as_str()) {
            return Err(CycleError {
                action: action_name.as_str().to_owned(),
                stack: self.call_stack.clone(),
            });
        }
        self.call_stack.push(action_name.as_str().to_owned());
        Ok(())
    }

    /// Pop the most recent action name from the composition call stack.
    ///
    /// Called when an invoked action closure returns.
    pub fn pop_call(&mut self) {
        self.call_stack.pop();
    }

    /// The current composition call stack (most-recently-pushed is last).
    pub fn call_stack(&self) -> &[String] {
        &self.call_stack
    }
}

/// A [`Scheduler::hold`] on one app, released when dropped.
///
/// Lives across the commit of a change, so it holds the scheduler handle
/// rather than a lock on it.
pub struct AppHold {
    scheduler: Arc<Mutex<Scheduler>>,
    app: AppName,
}

impl AppHold {
    /// Hold `app`, or name what holds it already.
    pub fn take(scheduler: &Arc<Mutex<Scheduler>>, app: &AppName) -> Result<Self, AppBlocker> {
        scheduler.lock().hold(app)?;
        Ok(Self {
            scheduler: Arc::clone(scheduler),
            app: app.clone(),
        })
    }

    /// Hold `app` only while no operation is running or queued for it, or
    /// name what keeps it; checked and taken under one lock, so no operation
    /// is admitted in between.
    pub fn take_idle(scheduler: &Arc<Mutex<Scheduler>>, app: &AppName) -> Result<Self, AppBlocker> {
        let mut locked = scheduler.lock();
        if let Some(blocker) = locked.blocker_for(app) {
            return Err(blocker);
        }
        locked.hold(app)?;
        Ok(Self {
            scheduler: Arc::clone(scheduler),
            app: app.clone(),
        })
    }
}

impl Drop for AppHold {
    fn drop(&mut self) {
        self.scheduler.lock().held.remove(&self.app);
    }
}

// ---------------------------------------------------------------------------
// Backoff
// ---------------------------------------------------------------------------

const BASE_BACKOFF_SECS: u64 = 5;
const MAX_BACKOFF_SECS: u64 = 300;

/// Decide whether an autonomous operation should be deferred due to backoff.
///
/// Filters `recent_ops` to those matching `resource` + `operation`, then
/// applies exponential backoff: `BASE * 2^(n-1)`, capped at `MAX_BACKOFF_SECS`.
///
/// Returns `Some(remaining_wait)` if the operation should be deferred, or
/// `None` if it should proceed now. The backoff counter resets automatically
/// when the gap since the last matching operation exceeds `MAX_BACKOFF_SECS`.
// spec: HIST#autonomous-operations
pub fn should_back_off(
    resource_id: InstanceId,
    operation: &str,
    recent_ops: &[AutonomousOperation],
    now: SystemTime,
) -> Option<Duration> {
    let matching: Vec<_> = recent_ops
        .iter()
        .filter(|op| op.resource_id == resource_id && op.operation == operation)
        .collect();

    if matching.is_empty() {
        return None;
    }

    let now_ms = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Safe: matching is non-empty.
    let last_ms = matching.iter().map(|op| op.recorded_at).max().unwrap();
    let elapsed_secs = ((now_ms - last_ms).max(0) as u64) / 1000;

    // Gap large enough to reset the backoff counter entirely.
    if elapsed_secs >= MAX_BACKOFF_SECS {
        return None;
    }

    let n = matching.len() as u32;
    // BASE * 2^(n-1); saturating to avoid overflow for very large n.
    let backoff_secs = BASE_BACKOFF_SECS
        .saturating_mul(2u64.saturating_pow(n.saturating_sub(1)))
        .min(MAX_BACKOFF_SECS);

    if elapsed_secs >= backoff_secs {
        // Already waited long enough — proceed.
        None
    } else {
        Some(Duration::from_secs(backoff_secs - elapsed_secs))
    }
}

#[cfg(test)]
mod tests;
