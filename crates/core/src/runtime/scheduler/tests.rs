use std::time::{Duration, UNIX_EPOCH};

use seedling_protocol::names::{ActionName, AppName};

use super::*;
use crate::runtime::identity::InstanceId;

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

fn act(s: &str) -> ActionName {
    ActionName::new_unchecked(s)
}

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

fn phase(p: AppPhase) -> Mutex<AppPhase> {
    Mutex::new(p)
}

// -----------------------------------------------------------------------
// Scheduler — admission
// -----------------------------------------------------------------------

// r[verify app.uninstall.exclusive]
// The gate used to live in the handlers, one copy each, and only some of them
// had it: an operator action, a param change's handler, a scheduled firing and
// an install all admit operations, and a rule applied per caller holds only
// for the callers that know it. Deciding it here is what makes it hold for all
// of them, so this is the test that the decision is actually made here.
#[test]
fn an_app_being_uninstalled_is_refused() {
    let mut s = Scheduler::new();
    assert_eq!(
        s.admit(
            &app("app1"),
            &act("backup"),
            serde_json::Map::new(),
            1,
            1,
            "operator",
            &phase(AppPhase::Uninstalling),
        ),
        ScheduleResult::Rejected(RejectReason::AppUninstalling)
    );
    assert!(
        s.active().is_none(),
        "a refused request takes no slot, so the teardown is not held up behind it"
    );
}

// r[verify app.uninstall.exclusive]
#[test]
fn an_installed_app_is_admitted() {
    let mut s = Scheduler::new();
    assert_eq!(
        s.admit(
            &app("app1"),
            &act("backup"),
            serde_json::Map::new(),
            1,
            1,
            "operator",
            &phase(AppPhase::Installed),
        ),
        ScheduleResult::Accepted
    );
}

// -----------------------------------------------------------------------
// Scheduler — single-app behaviour
// -----------------------------------------------------------------------

// r[verify operation.lifecycle.single]
#[test]
fn empty_scheduler_accepts_first_request() {
    let mut s = Scheduler::new();
    assert_eq!(
        s.request(
            &app("app1"),
            &act("start"),
            serde_json::Map::new(),
            1,
            1,
            "operator"
        ),
        ScheduleResult::Accepted
    );
    let active = s.active().expect("should be active");
    assert_eq!(active.app, "app1");
    assert_eq!(active.action, "start");
}

// r[verify operation.lifecycle.single.intra-app]
#[test]
fn same_app_second_request_rejected() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    assert_eq!(
        s.request(
            &app("app1"),
            &act("start"),
            serde_json::Map::new(),
            1,
            1,
            "operator"
        ),
        ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress)
    );
}

// r[verify operation.lifecycle.single.intra-app]
#[test]
fn same_app_different_action_still_rejected() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    assert_eq!(
        s.request(
            &app("app1"),
            &act("deploy"),
            serde_json::Map::new(),
            1,
            1,
            "operator"
        ),
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
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    assert_eq!(
        s.request(
            &app("app2"),
            &act("start"),
            serde_json::Map::new(),
            1,
            1,
            "operator"
        ),
        ScheduleResult::Queued
    );
    // app2 is queued, not yet active.
    assert_eq!(s.active().unwrap().app, "app1");
    assert_eq!(s.queue.len(), 1);
    assert_eq!(s.queue[0].app, "app2");
}

// r[verify operation.lifecycle.single.inter-app]
#[test]
fn already_queued_app_is_rejected() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.request(
        &app("app2"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    ); // queued
    assert_eq!(
        s.request(
            &app("app2"),
            &act("start"),
            serde_json::Map::new(),
            1,
            1,
            "operator"
        ),
        ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued)
    );
}

// r[verify operation.lifecycle.single.inter-app]
#[test]
fn two_different_apps_can_both_queue() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    assert_eq!(
        s.request(
            &app("app2"),
            &act("start"),
            serde_json::Map::new(),
            1,
            1,
            "operator"
        ),
        ScheduleResult::Queued
    );
    assert_eq!(
        s.request(
            &app("app3"),
            &act("start"),
            serde_json::Map::new(),
            1,
            1,
            "operator"
        ),
        ScheduleResult::Queued
    );
    assert_eq!(s.queue.len(), 2);
}

// r[verify operation.lifecycle.single.inter-app]
#[test]
fn queued_app_rejected_regardless_of_action_name() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.request(
        &app("app2"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    assert_eq!(
        s.request(
            &app("app2"),
            &act("deploy"),
            serde_json::Map::new(),
            1,
            1,
            "operator"
        ),
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
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    let next = s.complete_current();
    assert!(next.is_none());
    assert!(s.active().is_none());
}

// r[verify operation.lifecycle.completion]
// r[verify operation.lifecycle.single.inter-app]
#[test]
fn complete_current_dequeues_next_and_makes_it_active() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.request(
        &app("app2"),
        &act("deploy"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );

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
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.request(
        &app("app2"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.request(
        &app("app3"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );

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
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.complete_current();
    // No active operation; app1 should be accepted again.
    assert_eq!(
        s.request(
            &app("app1"),
            &act("start"),
            serde_json::Map::new(),
            1,
            1,
            "operator"
        ),
        ScheduleResult::Accepted
    );
}

// -----------------------------------------------------------------------
// Scheduler — call stack and cycle detection
// -----------------------------------------------------------------------

// r[verify operation.composition]
#[test]
fn push_call_with_no_cycle_succeeds() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    assert!(s.push_call(&act("start")).is_ok());
    assert!(s.push_call(&act("setup")).is_ok());
    assert!(s.push_call(&act("configure")).is_ok());
}

// r[verify operation.composition.cycles]
#[test]
fn push_call_detects_direct_cycle() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.push_call(&act("start")).unwrap();
    let err = s.push_call(&act("start")).unwrap_err();
    assert_eq!(err.action, "start");
    assert_eq!(err.stack, vec!["start"]);
}

// r[verify operation.composition.cycles]
#[test]
fn push_call_detects_transitive_cycle() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.push_call(&act("start")).unwrap();
    s.push_call(&act("setup")).unwrap();
    s.push_call(&act("configure")).unwrap();
    let err = s.push_call(&act("start")).unwrap_err();
    assert_eq!(err.action, "start");
    assert_eq!(err.stack, vec!["start", "setup", "configure"]);
}

// r[verify operation.composition]
#[test]
fn pop_call_allows_reuse_of_action_name() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.push_call(&act("setup")).unwrap();
    s.pop_call();
    // After popping "setup", pushing it again must not be a cycle.
    assert!(s.push_call(&act("setup")).is_ok());
}

// r[verify operation.lifecycle.completion]
#[test]
fn complete_current_clears_call_stack() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.request(
        &app("app2"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
    s.push_call(&act("start")).unwrap();
    s.push_call(&act("setup")).unwrap();

    s.complete_current();
    assert!(s.call_stack().is_empty());
}

// r[verify operation.composition]
#[test]
fn call_stack_is_empty_at_start_of_new_operation() {
    let mut s = Scheduler::new();
    s.request(
        &app("app1"),
        &act("start"),
        serde_json::Map::new(),
        1,
        1,
        "operator",
    );
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

// -----------------------------------------------------------------------
// AppHold
// -----------------------------------------------------------------------

fn shared() -> Arc<Mutex<Scheduler>> {
    Arc::new(Mutex::new(Scheduler::new()))
}

fn fire(s: &Arc<Mutex<Scheduler>>, name: &str) -> ScheduleResult {
    s.lock().admit(
        &app(name),
        &act("nightly"),
        serde_json::Map::new(),
        1,
        1,
        "schedule",
        &phase(AppPhase::Installed),
    )
}

// r[verify operation.lifecycle.param-change]
// A param change checked that the app was free, committed, and then admitted
// its handler. A scheduled firing admitted in between had the handler
// refused after the value was already stored.
#[test]
fn a_held_app_admits_nothing_until_its_handler_takes_the_hold_s_place() {
    let s = shared();
    let hold = AppHold::take(&s, &app("app1")).expect("a free app can be held");
    assert_eq!(s.lock().blocker_for(&app("app1")), Some(AppBlocker::Change));
    assert_eq!(
        fire(&s, "app1"),
        ScheduleResult::Rejected(RejectReason::ChangeBeingApplied)
    );
    assert_eq!(
        fire(&s, "app2"),
        ScheduleResult::Accepted,
        "other apps are not held"
    );
    s.lock().complete_current();

    let (result, op) = hold.admit(
        &act("apikey"),
        1,
        2,
        "param_change",
        &phase(AppPhase::Installed),
    );
    assert_eq!(result, ScheduleResult::Accepted);
    assert_eq!(op, s.lock().active().map(|a| a.operation_id.clone()));
    assert_eq!(s.lock().active().unwrap().action, act("apikey"));
}

// r[verify operation.lifecycle.param-change]
#[test]
fn a_handler_queued_in_the_hold_s_place_reports_its_id() {
    let s = shared();
    assert_eq!(fire(&s, "app2"), ScheduleResult::Accepted);
    let hold = AppHold::take(&s, &app("app1")).expect("a free app can be held");
    let (result, op) = hold.admit(
        &act("apikey"),
        1,
        2,
        "param_change",
        &phase(AppPhase::Installed),
    );
    assert_eq!(result, ScheduleResult::Queued);
    assert_eq!(
        op,
        s.lock()
            .queue_iter()
            .find(|q| q.app == app("app1"))
            .map(|q| q.operation_id.clone())
    );
    assert!(op.is_some());
}

// r[verify operation.lifecycle.param-change]
#[test]
fn an_app_with_an_operation_or_a_hold_cannot_be_held() {
    let s = shared();
    assert_eq!(fire(&s, "app1"), ScheduleResult::Accepted);
    assert_eq!(
        AppHold::take(&s, &app("app1")).err(),
        Some(AppBlocker::Active(act("nightly")))
    );
    s.lock().complete_current();

    let _hold = AppHold::take(&s, &app("app1")).expect("free again");
    assert_eq!(
        AppHold::take(&s, &app("app1")).err(),
        Some(AppBlocker::Change),
        "one change at a time"
    );
}

// r[verify operation.lifecycle.param-change]
#[test]
fn a_dropped_hold_frees_the_app() {
    let s = shared();
    drop(AppHold::take(&s, &app("app1")).expect("a free app can be held"));
    assert_eq!(s.lock().blocker_for(&app("app1")), None);
    assert_eq!(fire(&s, "app1"), ScheduleResult::Accepted);
}

// r[verify operation.lifecycle.param-change]
#[test]
fn a_refused_handler_frees_the_app() {
    let s = shared();
    let first = AppHold::take(&s, &app("app1")).unwrap();
    let (result, _) = first.admit(
        &act("apikey"),
        1,
        2,
        "param_change",
        &phase(AppPhase::Uninstalling),
    );
    assert_eq!(
        result,
        ScheduleResult::Rejected(RejectReason::AppUninstalling)
    );

    let _second = AppHold::take(&s, &app("app1")).expect("free after the refusal");
    assert_eq!(
        fire(&s, "app1"),
        ScheduleResult::Rejected(RejectReason::ChangeBeingApplied),
        "the new hold stands"
    );
}
