//! How a request for an operation is admitted, queued and persisted.

use std::sync::Arc;

use seedling_protocol::{events::OperationTarget, names::AppName};

use super::{OperationParams, spawn_accepted_operation};
use crate::{
    oi::state::OiState,
    runtime::{AppPhase, barrier::OperationId},
};

/// What came of asking for a lifecycle operation.
pub(crate) enum Admission {
    /// It started at once.
    Started(OperationId),
    /// It is queued behind another app's operation, and persisted.
    Queued(OperationId),
    Rejected(crate::runtime::scheduler::RejectReason),
    /// It was queued and could not be persisted, so it was taken off the queue.
    CannotPersist(seedling_protocol::error::OiError),
}

/// Ask for an operation, and start it if it is accepted at once.
///
/// Every request for an operation goes through here: an operator's action, a
/// schedule's firing, and a gate that is due. The scheduler decides, under its
/// own lock, whether it starts, waits or is refused.
// spec: OPS#concurrency
// spec: TRD#uninstall-admission
#[expect(
    clippy::too_many_arguments,
    reason = "the request is what the scheduler needs to admit it"
)]
pub(crate) fn admit_and_spawn(
    state: &Arc<OiState>,
    app: &AppName,
    target: OperationTarget,
    params: crate::runtime::param_map::ParamMap,
    generation: u64,
    trigger: &str,
    phase: &Arc<parking_lot::Mutex<AppPhase>>,
    actor: Option<Arc<seedling_protocol::actor::Actor>>,
) -> Admission {
    use crate::runtime::scheduler::ScheduleResult;
    let (result, operation_id) = {
        let mut sched = state.scheduler.lock();
        let result = sched.admit(
            app,
            &target,
            params.clone(),
            generation,
            trigger,
            phase,
            actor.clone(),
        );
        let operation_id = match &result {
            ScheduleResult::Accepted => sched.active().map(|a| a.operation_id.clone()),
            ScheduleResult::Queued => sched
                .queue_iter()
                .find(|q| q.app == *app)
                .map(|q| q.operation_id.clone()),
            ScheduleResult::Rejected(_) => None,
        };
        (
            result,
            operation_id.unwrap_or_else(|| OperationId(String::new())),
        )
    };
    match result {
        ScheduleResult::Accepted => {
            spawn_accepted_operation(
                Arc::clone(state),
                app.clone(),
                target,
                operation_id.clone(),
                OperationParams::Requested(params),
                generation,
                trigger.to_owned(),
                actor,
            );
            Admission::Started(operation_id)
        }
        ScheduleResult::Queued => match persist_queued(state, &operation_id) {
            Ok(()) => Admission::Queued(operation_id),
            Err(e) => Admission::CannotPersist(e),
        },
        ScheduleResult::Rejected(reason) => Admission::Rejected(reason),
    }
}

/// Persist the request the scheduler has just queued as `operation_id`, so
/// the queue survives a restart, and refuse the request if that fails.
///
/// The request is on the scheduler's queue until this returns, and is taken
/// off it again when it cannot be persisted, so a request that is refused is
/// one that never ran. A request that has already started by the time it is
/// persisted leaves no record behind.
// spec: OPS#concurrency
pub fn persist_queued(
    state: &OiState,
    operation_id: &OperationId,
) -> Result<(), seedling_protocol::error::OiError> {
    let queued = state.scheduler.lock().queued(operation_id).cloned();
    let Some(queued) = queued else {
        return Ok(());
    };
    let cipher = Arc::clone(&state.cipher);
    let saved = state
        .db
        .call(move |db| crate::runtime::history::save_queued_operation(db, &cipher, &queued));
    if let Err(e) = saved {
        state.scheduler.lock().unqueue(operation_id);
        return Err(seedling_protocol::error::OiError::internal(format!(
            "cannot persist the queued operation, so it is refused: {e}"
        )));
    }
    if state.scheduler.lock().queued(operation_id).is_none() {
        let id = operation_id.clone();
        state.db.call(move |db| {
            let _ = crate::runtime::history::clear_queued_operation(db, &id);
        });
    }
    Ok(())
}

/// Restore the persisted queue after the decision to replay an interrupted
/// operation, in the order the requests were made.
///
/// A request whose app is no longer registered is discarded. With nothing in
/// progress the first restored request starts, and the rest queue behind it.
// spec: OPS#concurrency
pub fn restore_queue(
    state: &Arc<OiState>,
    queued: Vec<crate::runtime::scheduler::QueuedOperation>,
) {
    use crate::runtime::scheduler::ScheduleResult;
    for op in queued {
        // spec: OPS#initiating-events
        // A request that starts at once is dispatched now, so it runs against
        // the generation current now; one that queues is resolved again as it
        // leaves the queue.
        let current = state
            .registry
            .read()
            .get(op.app.as_str())
            .map(|entry| entry.current_generation);
        let Some(generation) = current else {
            tracing::warn!(app = %op.app, operation = %op.target, "queued operation names an unregistered app; discarding");
            let id = op.operation_id.clone();
            state.db.call(move |db| {
                let _ = crate::runtime::history::clear_queued_operation(db, &id);
            });
            continue;
        };
        let result = state.scheduler.lock().request_with_id(
            &op.app,
            &op.target,
            op.params.clone(),
            generation,
            &op.trigger,
            op.operation_id.clone(),
            op.actor.clone(),
        );
        match result {
            ScheduleResult::Accepted => {
                spawn_accepted_operation(
                    Arc::clone(state),
                    op.app,
                    op.target,
                    op.operation_id,
                    OperationParams::Requested(op.params),
                    generation,
                    op.trigger,
                    op.actor,
                );
            }
            ScheduleResult::Queued => {}
            ScheduleResult::Rejected(reason) => {
                tracing::warn!(app = %op.app, operation = %op.target, ?reason, "restored queued operation refused; discarding");
                let id = op.operation_id.clone();
                state.db.call(move |db| {
                    let _ = crate::runtime::history::clear_queued_operation(db, &id);
                });
            }
        }
    }
}

/// Persist a scheduled fire the scheduler queued, and stamp its fire time once
/// it is on record. A fire that cannot be persisted is taken off the queue and
/// left unstamped, so the next check fires it again.
// spec: SCHED#fire-times
// spec: OPS#concurrency
pub fn persist_queued_fire(
    state: &OiState,
    fire: &crate::runtime::schedules::FiredSchedule,
    now: jiff::Timestamp,
) {
    let Some(operation_id) = &fire.operation_id else {
        return;
    };
    match persist_queued(state, operation_id) {
        Ok(()) => {
            let (app, action, cron) =
                (fire.app.clone(), fire.action.clone(), fire.cronexpr.clone());
            state.db.call(move |db| {
                if let Err(e) =
                    crate::runtime::db::upsert_schedule_fired(db, &app, &action, &cron, &now.to_string())
                {
                    tracing::error!(app = %app, action = %action, "failed to stamp a queued fire: {e}");
                }
            });
        }
        Err(e) => {
            tracing::error!(app = %fire.app, action = %fire.action, "scheduled fire refused: {}", e.message);
        }
    }
}
