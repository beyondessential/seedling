use std::time::{Duration, UNIX_EPOCH};

use super::*;
use crate::runtime::identity::InstanceId;

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

fn dep_id() -> InstanceId {
    InstanceId::generate()
}

/// Build an AutonomousOperation recorded at `ms` milliseconds since epoch.
fn op_at(resource_id: InstanceId, operation: &str, ms: i64) -> AutonomousOperation {
    AutonomousOperation {
        id: 0,
        recorded_at: ms,
        resource_id,
        operation: operation.to_owned(),
        provenance: serde_json::Value::Null,
        outcome: Some("ok".to_owned()),
        completed_at: None,
    }
}

fn now_from_ms(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms as u64)
}

// -----------------------------------------------------------------------
// Scheduler — single-app behaviour
// -----------------------------------------------------------------------

// r[verify operation.lifecycle.single]
#[test]
fn empty_scheduler_accepts_first_request() {
    let mut s = Scheduler::new();
    assert_eq!(s.request("app1", "start", None), ScheduleResult::Accepted);
    let active = s.active().expect("should be active");
    assert_eq!(active.app, "app1");
    assert_eq!(active.action, "start");
}

// r[verify operation.lifecycle.single.intra-app]
#[test]
fn same_app_second_request_rejected() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    assert_eq!(
        s.request("app1", "start", None),
        ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress)
    );
}

// r[verify operation.lifecycle.single.intra-app]
#[test]
fn same_app_different_action_still_rejected() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    assert_eq!(
        s.request("app1", "deploy", None),
        ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress)
    );
}

// -----------------------------------------------------------------------
// Scheduler — inter-app queuing
// -----------------------------------------------------------------------

// r[verify operation.lifecycle.single.inter-app]
#[test]
fn different_app_request_is_queued() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    assert_eq!(s.request("app2", "start", None), ScheduleResult::Queued);
    // app2 is queued, not yet active.
    assert_eq!(s.active().unwrap().app, "app1");
    assert_eq!(s.queue.len(), 1);
    assert_eq!(s.queue[0].app, "app2");
}

// r[verify operation.lifecycle.single.inter-app]
#[test]
fn already_queued_app_is_rejected() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    s.request("app2", "start", None); // queued
    assert_eq!(
        s.request("app2", "start", None),
        ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued)
    );
}

// r[verify operation.lifecycle.single.inter-app]
#[test]
fn two_different_apps_can_both_queue() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    assert_eq!(s.request("app2", "start", None), ScheduleResult::Queued);
    assert_eq!(s.request("app3", "start", None), ScheduleResult::Queued);
    assert_eq!(s.queue.len(), 2);
}

// r[verify operation.lifecycle.single.inter-app]
#[test]
fn queued_app_rejected_regardless_of_action_name() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    s.request("app2", "start", None);
    assert_eq!(
        s.request("app2", "deploy", None),
        ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued)
    );
}

// -----------------------------------------------------------------------
// Scheduler — completion and dequeue
// -----------------------------------------------------------------------

// r[verify operation.lifecycle.completion]
#[test]
fn complete_current_with_empty_queue_clears_active() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    let next = s.complete_current();
    assert!(next.is_none());
    assert!(s.active().is_none());
}

// r[verify operation.lifecycle.completion]
// r[verify operation.lifecycle.single.inter-app]
#[test]
fn complete_current_dequeues_next_and_makes_it_active() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    s.request("app2", "deploy", None);

    let next = s.complete_current().expect("should dequeue app2");
    assert_eq!(next.app, "app2");
    assert_eq!(next.action, "deploy");

    let active = s.active().expect("app2 should now be active");
    assert_eq!(active.app, "app2");
    assert_eq!(active.operation_id, next.operation_id);
}

// r[verify operation.lifecycle.completion]
// r[verify operation.lifecycle.single.inter-app]
#[test]
fn queue_is_drained_in_fifo_order() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    s.request("app2", "start", None);
    s.request("app3", "start", None);

    let first = s.complete_current().expect("app2");
    assert_eq!(first.app, "app2");

    let second = s.complete_current().expect("app3");
    assert_eq!(second.app, "app3");

    let none = s.complete_current();
    assert!(none.is_none());
    assert!(s.active().is_none());
}

// r[verify operation.lifecycle.completion]
#[test]
fn after_complete_same_app_can_be_requested_again() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    s.complete_current();
    // No active operation; app1 should be accepted again.
    assert_eq!(s.request("app1", "start", None), ScheduleResult::Accepted);
}

// -----------------------------------------------------------------------
// Scheduler — call stack and cycle detection
// -----------------------------------------------------------------------

// r[verify operation.composition]
#[test]
fn push_call_with_no_cycle_succeeds() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    assert!(s.push_call("start").is_ok());
    assert!(s.push_call("setup").is_ok());
    assert!(s.push_call("configure").is_ok());
}

// r[verify operation.composition.cycles]
#[test]
fn push_call_detects_direct_cycle() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    s.push_call("start").unwrap();
    let err = s.push_call("start").unwrap_err();
    assert_eq!(err.action, "start");
    assert_eq!(err.stack, vec!["start"]);
}

// r[verify operation.composition.cycles]
#[test]
fn push_call_detects_transitive_cycle() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    s.push_call("start").unwrap();
    s.push_call("setup").unwrap();
    s.push_call("configure").unwrap();
    let err = s.push_call("start").unwrap_err();
    assert_eq!(err.action, "start");
    assert_eq!(err.stack, vec!["start", "setup", "configure"]);
}

// r[verify operation.composition]
#[test]
fn pop_call_allows_reuse_of_action_name() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    s.push_call("setup").unwrap();
    s.pop_call();
    // After popping "setup", pushing it again must not be a cycle.
    assert!(s.push_call("setup").is_ok());
}

// r[verify operation.lifecycle.completion]
#[test]
fn complete_current_clears_call_stack() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    s.request("app2", "start", None);
    s.push_call("start").unwrap();
    s.push_call("setup").unwrap();

    s.complete_current();
    assert!(s.call_stack().is_empty());
}

// r[verify operation.composition]
#[test]
fn call_stack_is_empty_at_start_of_new_operation() {
    let mut s = Scheduler::new();
    s.request("app1", "start", None);
    // Immediately after the very first request, stack is empty.
    assert!(s.call_stack().is_empty());
}

// -----------------------------------------------------------------------
// Backoff
// -----------------------------------------------------------------------

// r[verify history.operations.rate-limiting]
#[test]
fn no_ops_means_no_backoff() {
    let id = dep_id();
    let result = should_back_off(id, "start_container", &[], SystemTime::now());
    assert!(result.is_none());
}

// r[verify history.operations.rate-limiting]
#[test]
fn single_recent_op_backs_off_for_base_period() {
    let id = dep_id();
    // Op recorded 1 second ago; backoff for n=1 is 5s; remaining = 4s.
    let now_ms: i64 = 1_700_000_000_000;
    let ops = [op_at(id, "start_container", now_ms - 1_000)];
    let result = should_back_off(id, "start_container", &ops, now_from_ms(now_ms));
    assert_eq!(result, Some(Duration::from_secs(4)));
}

// r[verify history.operations.rate-limiting]
#[test]
fn two_recent_ops_back_off_longer() {
    let id = dep_id();
    // n=2 → backoff = 5 * 2 = 10s; elapsed = 1s; remaining = 9s.
    let now_ms: i64 = 1_700_000_000_000;
    let ops = [
        op_at(id, "start_container", now_ms - 2_000),
        op_at(id, "start_container", now_ms - 1_000),
    ];
    let result = should_back_off(id, "start_container", &ops, now_from_ms(now_ms));
    assert_eq!(result, Some(Duration::from_secs(9)));
}

// r[verify history.operations.rate-limiting]
#[test]
fn backoff_duration_increases_with_op_count() {
    let id = dep_id();
    let now_ms: i64 = 1_700_000_000_000;
    let now = now_from_ms(now_ms);

    let make_ops = |n: usize| -> Vec<AutonomousOperation> {
        (0..n)
            .map(|i| op_at(id, "start_container", now_ms - (i as i64 + 1) * 1_000))
            .collect()
    };

    let wait1 = should_back_off(id, "start_container", &make_ops(1), now).expect("should back off");
    let wait2 = should_back_off(id, "start_container", &make_ops(2), now).expect("should back off");
    let wait3 = should_back_off(id, "start_container", &make_ops(3), now).expect("should back off");

    assert!(wait2 > wait1, "backoff should grow: {wait2:?} > {wait1:?}");
    assert!(wait3 > wait2, "backoff should grow: {wait3:?} > {wait2:?}");
}

// r[verify history.operations.rate-limiting]
#[test]
fn backoff_caps_at_maximum() {
    let id = dep_id();
    // With 100 ops, 2^99 overflows — must still cap at MAX_BACKOFF_SECS.
    let now_ms: i64 = 1_700_000_000_000;
    let ops: Vec<_> = (0..100)
        .map(|i| op_at(id, "start_container", now_ms - (i + 1) * 1_000))
        .collect();
    let result = should_back_off(id, "start_container", &ops, now_from_ms(now_ms));
    // Should be capped; remaining = MAX - elapsed(1s) = 299s.
    assert_eq!(result, Some(Duration::from_secs(299)));
}

// r[verify history.operations.rate-limiting]
#[test]
fn waited_full_backoff_period_proceeds() {
    let id = dep_id();
    // n=1, backoff=5s, but 5 seconds have elapsed — should proceed.
    let now_ms: i64 = 1_700_000_000_000;
    let ops = [op_at(id, "start_container", now_ms - 5_000)];
    let result = should_back_off(id, "start_container", &ops, now_from_ms(now_ms));
    assert!(result.is_none());
}

// r[verify history.operations.rate-limiting]
#[test]
fn gap_since_last_op_resets_backoff() {
    let id = dep_id();
    // Many ops, but the last one was 400 seconds ago — gap > MAX_BACKOFF_SECS.
    let now_ms: i64 = 1_700_000_000_000;
    let ops: Vec<_> = (0..10)
        .map(|i| op_at(id, "start_container", now_ms - 400_000 - i * 1_000))
        .collect();
    let result = should_back_off(id, "start_container", &ops, now_from_ms(now_ms));
    assert!(result.is_none(), "backoff should reset after gap");
}

// r[verify history.operations.rate-limiting]
#[test]
fn ops_for_different_resource_are_ignored() {
    let web = dep_id();
    let api = dep_id();
    let now_ms: i64 = 1_700_000_000_000;
    // Only ops for api, not web.
    let ops = [op_at(api, "start_container", now_ms - 1_000)];
    let result = should_back_off(web, "start_container", &ops, now_from_ms(now_ms));
    assert!(result.is_none());
}

// r[verify history.operations.rate-limiting]
#[test]
fn ops_for_different_operation_are_ignored() {
    let id = dep_id();
    let now_ms: i64 = 1_700_000_000_000;
    // Op is "rebuild_proxy", not "start_container".
    let ops = [op_at(id, "rebuild_proxy", now_ms - 1_000)];
    let result = should_back_off(id, "start_container", &ops, now_from_ms(now_ms));
    assert!(result.is_none());
}

// r[verify history.operations.rate-limiting]
#[test]
fn mixed_ops_only_matching_ones_contribute_to_backoff() {
    let web = dep_id();
    let api = dep_id();
    let now_ms: i64 = 1_700_000_000_000;

    // One matching op for web and one non-matching for api.
    // Only the web op should count → n=1, backoff=5s, elapsed=1s → Some(4s).
    let ops = [
        op_at(web, "start_container", now_ms - 1_000),
        op_at(api, "start_container", now_ms - 500),
    ];
    let result = should_back_off(web, "start_container", &ops, now_from_ms(now_ms));
    assert_eq!(result, Some(Duration::from_secs(4)));
}
