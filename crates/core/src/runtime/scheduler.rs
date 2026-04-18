use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::runtime::barrier::OperationId;
use crate::runtime::history::AutonomousOperation;
use crate::runtime::identity::InstanceId;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

// r[impl operation.lifecycle]
#[derive(Debug, Clone)]
pub struct ActiveOperation {
    pub app: String,
    pub action: String,
    pub operation_id: OperationId,
    /// Generation that was current immediately before the change that
    /// triggered this operation. Equal to target for operations not
    /// triggered by a generation bump.
    // r[impl operation.lifecycle.generations]
    pub source_generation: u64,
    /// Generation produced by the change that triggered this operation, or
    /// the current generation at dispatch for operator-invoked actions.
    // r[impl operation.lifecycle.generations]
    pub target_generation: u64,
}

// r[impl operation.lifecycle]
#[derive(Debug, Clone)]
pub struct QueuedOperation {
    pub app: String,
    pub action: String,
    pub operation_id: OperationId,
    /// Action params passed by the invoker. For install actions, contains the
    /// validated requirements. Empty map for actions with no params.
    // r[impl operation.params]
    pub params: serde_json::Map<String, serde_json::Value>,
    // r[impl operation.lifecycle.generations]
    pub source_generation: u64,
    // r[impl operation.lifecycle.generations]
    pub target_generation: u64,
    // i[impl event.types]
    pub trigger: String,
}

// r[impl operation.lifecycle.single]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleResult {
    /// Operation started immediately (no prior active op).
    Accepted,
    /// Operation added to the queue (another app was active).
    Queued,
    /// The request was rejected.
    Rejected(RejectReason),
}

// r[impl operation.lifecycle.single.intra-app]
// r[impl operation.lifecycle.single.inter-app]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// A lifecycle operation for this application is already in progress.
    SameAppOperationInProgress,
    /// A lifecycle operation for this application is already queued.
    SameAppAlreadyQueued,
}

// r[impl operation.composition.cycles]
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
// r[impl operation.lifecycle.single]
#[derive(Debug, Default)]
pub struct Scheduler {
    active: Option<ActiveOperation>,
    /// Pending operations in insertion (request) order; at most one per app.
    queue: VecDeque<QueuedOperation>,
    /// Inline action invocation chain for the active operation.
    call_stack: Vec<String>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently active operation, if any.
    pub fn active(&self) -> Option<&ActiveOperation> {
        self.active.as_ref()
    }

    /// Iterator over queued operations.
    pub fn queue_iter(&self) -> impl Iterator<Item = &QueuedOperation> {
        self.queue.iter()
    }

    /// Returns true if there is an active or queued operation for the given app.
    pub fn has_operation_for(&self, app: &str) -> bool {
        self.active.as_ref().is_some_and(|a| a.app == app)
            || self.queue.iter().any(|q| q.app == app)
    }

    /// Request a lifecycle operation for `app` / `action`.
    ///
    /// - If no operation is active: the operation starts immediately.
    /// - If a different app is active and this app is not yet queued: queued.
    /// - If this app is active or already queued: rejected.
    // r[impl operation.lifecycle.single]
    // r[impl operation.lifecycle.single.intra-app]
    // r[impl operation.lifecycle.single.inter-app]
    pub fn request(
        &mut self,
        app: &str,
        action: &str,
        params: serde_json::Map<String, serde_json::Value>,
        source_generation: u64,
        target_generation: u64,
        trigger: &str,
    ) -> ScheduleResult {
        self.request_with_id(
            app,
            action,
            params,
            source_generation,
            target_generation,
            trigger,
            OperationId::new(),
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
        app: &str,
        action: &str,
        params: serde_json::Map<String, serde_json::Value>,
        source_generation: u64,
        target_generation: u64,
        trigger: &str,
        operation_id: OperationId,
    ) -> ScheduleResult {
        match &self.active {
            None => {
                // No active operation — start immediately.
                self.call_stack.clear();
                self.active = Some(ActiveOperation {
                    app: app.to_owned(),
                    action: action.to_owned(),
                    operation_id,
                    source_generation,
                    target_generation,
                });
                ScheduleResult::Accepted
            }
            Some(active) if active.app == app => {
                // r[impl operation.lifecycle.single.intra-app]
                ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress)
            }
            Some(_) => {
                // r[impl operation.lifecycle.single.inter-app]
                if self.queue.iter().any(|q| q.app == app) {
                    return ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued);
                }
                self.queue.push_back(QueuedOperation {
                    app: app.to_owned(),
                    action: action.to_owned(),
                    operation_id,
                    params,
                    source_generation,
                    target_generation,
                    trigger: trigger.to_owned(),
                });
                ScheduleResult::Queued
            }
        }
    }

    /// Mark the current operation as complete and dequeue the next one, if any.
    ///
    /// Returns the newly-started operation so the caller knows what to run.
    // r[impl operation.lifecycle.completion]
    pub fn complete_current(&mut self) -> Option<QueuedOperation> {
        self.active = None;
        self.call_stack.clear();

        let next = self.queue.pop_front()?;
        self.active = Some(ActiveOperation {
            app: next.app.clone(),
            action: next.action.clone(),
            operation_id: next.operation_id.clone(),
            source_generation: next.source_generation,
            target_generation: next.target_generation,
        });
        Some(next)
    }

    /// Push an action name onto the composition call stack.
    ///
    /// Returns `Err(CycleError)` if `action_name` is already on the stack,
    /// which means this invocation would form a direct or transitive cycle.
    // r[impl operation.composition]
    // r[impl operation.composition.cycles]
    pub fn push_call(&mut self, action_name: &str) -> Result<(), CycleError> {
        if self.call_stack.iter().any(|a| a == action_name) {
            return Err(CycleError {
                action: action_name.to_owned(),
                stack: self.call_stack.clone(),
            });
        }
        self.call_stack.push(action_name.to_owned());
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
// r[impl history.operations.rate-limiting]
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
