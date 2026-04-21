use std::time::{Duration, UNIX_EPOCH};

use seedling_protocol::names::AppName;

use super::*;
use crate::defs::resource::ResourceKind;
use crate::runtime::db::{Db, DbHandle};
use crate::runtime::history::{WorldObservation, insert_observation, query_observations};

fn app_name(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

fn dep(app: &str, name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton(app_name(app), ResourceKind::Deployment, name)
}

fn svc(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton(app_name("app"), ResourceKind::Service, name)
}

fn ing(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton(app_name("app"), ResourceKind::Ingress, name)
}

fn vol(name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton(app_name("app"), ResourceKind::Volume, name)
}

/// Build a `WorldObservation` with `recorded_at = 0` for testing the pure
/// derivation function.
fn obs(obs_kind: &str) -> WorldObservation {
    obs_at(obs_kind, 0)
}

/// Build a `WorldObservation` with an explicit timestamp (ms).
fn obs_at(obs_kind: &str, recorded_at: i64) -> WorldObservation {
    WorldObservation {
        id: 0,
        recorded_at,
        resource: dep("app", "web"),
        obs_kind: obs_kind.into(),
        payload: serde_json::Value::Null,
    }
}

// -----------------------------------------------------------------------
// Container derivation
// -----------------------------------------------------------------------

// r[verify lifecycle.derivation]
// r[verify lifecycle.states]
#[test]
fn empty_observations_gives_pending() {
    let resource = dep("app", "web");
    let state = derive_lifecycle_state(&resource, &[]);
    assert_eq!(state, LifecycleState::Pending);
}

// r[verify lifecycle.container]
// r[verify lifecycle.derivation]
#[test]
fn container_created_gives_scheduled() {
    let resource = dep("app", "web");
    let state = derive_lifecycle_state(&resource, &[obs("container_created")]);
    assert_eq!(state, LifecycleState::Scheduled);
}

// r[verify lifecycle.container]
#[test]
fn image_pull_started_gives_scheduled() {
    let resource = dep("app", "web");
    let state = derive_lifecycle_state(&resource, &[obs("image_pull_started")]);
    assert_eq!(state, LifecycleState::Scheduled);
}

// r[verify lifecycle.container]
// r[verify lifecycle.derivation]
#[test]
fn container_running_gives_running() {
    let resource = dep("app", "web");
    let state = derive_lifecycle_state(&resource, &[obs("container_running")]);
    assert_eq!(state, LifecycleState::Running);
}

// r[verify lifecycle.container]
#[test]
fn health_check_pass_gives_ready() {
    let resource = dep("app", "web");
    let state = derive_lifecycle_state(
        &resource,
        &[obs("container_running"), obs("health_check_pass")],
    );
    assert_eq!(state, LifecycleState::Ready);
}

// r[verify lifecycle.container]
// r[verify lifecycle.transitions]
#[test]
fn container_exited_gives_terminated_skipping_terminating() {
    let resource = dep("app", "web");
    let state = derive_lifecycle_state(
        &resource,
        &[obs("container_running"), obs("container_exited")],
    );
    assert_eq!(state, LifecycleState::Terminated);
}

// r[verify lifecycle.container]
// r[verify lifecycle.transitions]
#[test]
fn stop_sent_then_exited_gives_terminated_via_terminating() {
    let resource = dep("app", "web");
    let state = derive_lifecycle_state(
        &resource,
        &[
            obs("container_running"),
            obs("stop_sent"),
            obs("container_exited"),
        ],
    );
    assert_eq!(state, LifecycleState::Terminated);
}

// r[verify lifecycle.container]
// r[verify lifecycle.transitions]
#[test]
fn container_removed_gives_unscheduled() {
    let resource = dep("app", "web");
    let state = derive_lifecycle_state(
        &resource,
        &[
            obs("container_running"),
            obs("container_exited"),
            obs("container_removed"),
        ],
    );
    assert_eq!(state, LifecycleState::Unscheduled);
}

// r[verify lifecycle.derivation]
// r[verify reconciliation.idempotency]
#[test]
fn duplicate_running_observations_do_not_regress() {
    let resource = dep("app", "web");
    let state = derive_lifecycle_state(
        &resource,
        &[obs("container_running"), obs("container_running")],
    );
    assert_eq!(state, LifecycleState::Running);
}

// r[verify lifecycle.derivation]
#[test]
fn unknown_obs_kind_is_ignored() {
    let resource = dep("app", "web");
    let state = derive_lifecycle_state(
        &resource,
        &[obs("container_running"), obs("some_unknown_event")],
    );
    assert_eq!(state, LifecycleState::Running);
}

// -----------------------------------------------------------------------
// Service derivation
// -----------------------------------------------------------------------

// r[verify lifecycle.service]
#[test]
fn service_no_observations_gives_pending() {
    assert_eq!(
        derive_lifecycle_state(&svc("lb"), &[]),
        LifecycleState::Pending
    );
}

// r[verify lifecycle.service]
#[test]
fn service_network_created_gives_scheduled() {
    assert_eq!(
        derive_lifecycle_state(&svc("lb"), &[obs("network_created")]),
        LifecycleState::Scheduled
    );
}

// r[verify lifecycle.service]
#[test]
fn service_backend_healthy_gives_ready() {
    assert_eq!(
        derive_lifecycle_state(
            &svc("lb"),
            &[obs("network_created"), obs("backend_healthy")]
        ),
        LifecycleState::Ready
    );
}

// r[verify lifecycle.service]
// r[verify lifecycle.transitions]
#[test]
fn service_backend_healthy_without_network_created_skips_to_ready() {
    assert_eq!(
        derive_lifecycle_state(&svc("lb"), &[obs("backend_healthy")]),
        LifecycleState::Ready
    );
}

// r[verify lifecycle.service]
// r[verify lifecycle.transitions]
#[test]
fn service_stop_then_network_removed_gives_terminated() {
    assert_eq!(
        derive_lifecycle_state(
            &svc("lb"),
            &[
                obs("network_created"),
                obs("backend_healthy"),
                obs("stop_sent"),
                obs("network_removed"),
            ]
        ),
        LifecycleState::Terminated
    );
}

// r[verify lifecycle.service]
// r[verify lifecycle.transitions]
#[test]
fn service_network_removed_without_stop_skips_terminating() {
    assert_eq!(
        derive_lifecycle_state(
            &svc("lb"),
            &[
                obs("network_created"),
                obs("backend_healthy"),
                obs("network_removed"),
            ]
        ),
        LifecycleState::Terminated
    );
}

// r[verify lifecycle.service]
#[test]
fn service_network_cleaned_up_gives_unscheduled() {
    assert_eq!(
        derive_lifecycle_state(
            &svc("lb"),
            &[
                obs("network_created"),
                obs("network_removed"),
                obs("network_cleaned_up"),
            ]
        ),
        LifecycleState::Unscheduled
    );
}

// r[verify lifecycle.service]
#[test]
fn service_container_obs_kinds_are_ignored() {
    assert_eq!(
        derive_lifecycle_state(&svc("lb"), &[obs("container_running")]),
        LifecycleState::Pending
    );
}

// -----------------------------------------------------------------------
// Ingress derivation
// -----------------------------------------------------------------------

// r[verify lifecycle.ingress]
#[test]
fn ingress_no_observations_gives_pending() {
    assert_eq!(
        derive_lifecycle_state(&ing("main"), &[]),
        LifecycleState::Pending
    );
}

// r[verify lifecycle.ingress]
#[test]
fn ingress_configured_gives_scheduled() {
    assert_eq!(
        derive_lifecycle_state(&ing("main"), &[obs("ingress_configured")]),
        LifecycleState::Scheduled
    );
}

// r[verify lifecycle.ingress]
#[test]
fn ingress_ready_gives_ready() {
    assert_eq!(
        derive_lifecycle_state(
            &ing("main"),
            &[obs("ingress_configured"), obs("ingress_ready")]
        ),
        LifecycleState::Ready
    );
}

// r[verify lifecycle.ingress]
// r[verify lifecycle.transitions]
#[test]
fn ingress_stop_then_removed_gives_terminated() {
    assert_eq!(
        derive_lifecycle_state(
            &ing("main"),
            &[
                obs("ingress_configured"),
                obs("ingress_ready"),
                obs("stop_sent"),
                obs("ingress_removed"),
            ]
        ),
        LifecycleState::Terminated
    );
}

// r[verify lifecycle.ingress]
// r[verify lifecycle.transitions]
#[test]
fn ingress_removed_without_stop_skips_terminating() {
    assert_eq!(
        derive_lifecycle_state(
            &ing("main"),
            &[
                obs("ingress_configured"),
                obs("ingress_ready"),
                obs("ingress_removed")
            ]
        ),
        LifecycleState::Terminated
    );
}

// r[verify lifecycle.ingress]
#[test]
fn ingress_cleaned_up_gives_unscheduled() {
    assert_eq!(
        derive_lifecycle_state(
            &ing("main"),
            &[
                obs("ingress_configured"),
                obs("ingress_removed"),
                obs("ingress_cleaned_up"),
            ]
        ),
        LifecycleState::Unscheduled
    );
}

// -----------------------------------------------------------------------
// Volume derivation
// -----------------------------------------------------------------------

// r[verify lifecycle.volume]
#[test]
fn volume_no_observations_gives_pending() {
    assert_eq!(
        derive_lifecycle_state(&vol("data"), &[]),
        LifecycleState::Pending
    );
}

// r[verify lifecycle.volume]
#[test]
fn volume_created_gives_scheduled() {
    assert_eq!(
        derive_lifecycle_state(&vol("data"), &[obs("volume_created")]),
        LifecycleState::Scheduled
    );
}

// r[verify lifecycle.volume]
#[test]
fn volume_ready_gives_ready() {
    assert_eq!(
        derive_lifecycle_state(&vol("data"), &[obs("volume_created"), obs("volume_ready")]),
        LifecycleState::Ready
    );
}

// r[verify lifecycle.volume]
// r[verify lifecycle.transitions]
#[test]
fn volume_stop_then_removed_gives_terminated() {
    assert_eq!(
        derive_lifecycle_state(
            &vol("data"),
            &[
                obs("volume_created"),
                obs("volume_ready"),
                obs("stop_sent"),
                obs("volume_removed"),
            ]
        ),
        LifecycleState::Terminated
    );
}

// r[verify lifecycle.volume]
// r[verify lifecycle.transitions]
#[test]
fn volume_removed_without_stop_skips_terminating() {
    assert_eq!(
        derive_lifecycle_state(
            &vol("data"),
            &[
                obs("volume_created"),
                obs("volume_ready"),
                obs("volume_removed")
            ]
        ),
        LifecycleState::Terminated
    );
}

// r[verify lifecycle.volume]
#[test]
fn volume_cleaned_up_gives_unscheduled() {
    assert_eq!(
        derive_lifecycle_state(
            &vol("data"),
            &[
                obs("volume_created"),
                obs("volume_removed"),
                obs("volume_cleaned_up"),
            ]
        ),
        LifecycleState::Unscheduled
    );
}

// -----------------------------------------------------------------------
// derive_state_with_transition_time
// -----------------------------------------------------------------------

// r[verify lifecycle.derivation]
#[test]
fn transition_time_is_none_when_pending() {
    let resource = dep("app", "web");
    let (state, time) = derive_state_with_transition_time(&resource, &[]);
    assert_eq!(state, LifecycleState::Pending);
    assert!(time.is_none());
}

// r[verify lifecycle.derivation]
#[test]
fn transition_time_matches_observation_recorded_at() {
    let resource = dep("app", "web");
    let ts_ms: i64 = 1_700_000_000_000;
    let observations = [obs_at("container_running", ts_ms)];
    let (state, time) = derive_state_with_transition_time(&resource, &observations);
    assert_eq!(state, LifecycleState::Running);
    let expected = UNIX_EPOCH + Duration::from_millis(ts_ms as u64);
    assert_eq!(time, Some(expected));
}

// r[verify lifecycle.derivation]
#[test]
fn transition_time_reflects_last_transition() {
    let resource = dep("app", "web");
    let t1: i64 = 1_000;
    let t2: i64 = 2_000;
    let observations = [
        obs_at("container_running", t1),
        obs_at("health_check_pass", t2),
    ];
    let (state, time) = derive_state_with_transition_time(&resource, &observations);
    assert_eq!(state, LifecycleState::Ready);
    let expected = UNIX_EPOCH + Duration::from_millis(t2 as u64);
    assert_eq!(time, Some(expected));
}

// r[verify lifecycle.derivation]
// r[verify reconciliation.idempotency]
#[test]
fn transition_time_not_updated_by_duplicate_observation() {
    let resource = dep("app", "web");
    let t1: i64 = 1_000;
    let t2: i64 = 5_000;
    let observations = [
        obs_at("container_running", t1),
        obs_at("container_running", t2),
    ];
    let (state, time) = derive_state_with_transition_time(&resource, &observations);
    assert_eq!(state, LifecycleState::Running);
    let expected = UNIX_EPOCH + Duration::from_millis(t1 as u64);
    assert_eq!(time, Some(expected));
}

// r[verify lifecycle.service]
// r[verify lifecycle.derivation]
#[test]
fn transition_time_works_for_service() {
    let ts_ms: i64 = 9_999_000;
    let resource = svc("lb");
    let observations = [obs_at("network_created", ts_ms)];
    let (state, time) = derive_state_with_transition_time(&resource, &observations);
    assert_eq!(state, LifecycleState::Scheduled);
    assert_eq!(time, Some(UNIX_EPOCH + Duration::from_millis(ts_ms as u64)));
}

// -----------------------------------------------------------------------
// DbWorldOracle (DB-backed)
// -----------------------------------------------------------------------

// r[verify history.world.state-derivation]
#[test]
fn db_oracle_empty_gives_pending() {
    let db = Db::open_in_memory().expect("open");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    let resource = dep("app", "web");
    assert_eq!(oracle.lifecycle_state(&resource), LifecycleState::Pending);
}

// r[verify history.world.state-derivation]
// r[verify lifecycle.derivation]
#[test]
fn db_oracle_derives_from_observations() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "web");

    insert_observation(&db, &resource, "container_created", &serde_json::json!({}))
        .expect("insert");
    insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
        .expect("insert");

    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.lifecycle_state(&resource), LifecycleState::Running);
}

// r[verify history.world.state-derivation]
// r[verify lifecycle.container]
// r[verify lifecycle.transitions]
#[test]
fn db_oracle_full_sequence_to_terminated() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "web");

    for kind in &["container_running", "stop_sent", "container_exited"] {
        insert_observation(&db, &resource, kind, &serde_json::json!({})).expect("insert");
    }

    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(
        oracle.lifecycle_state(&resource),
        LifecycleState::Terminated
    );
}

// r[verify history.world.state-derivation]
// r[verify lifecycle.derivation]
#[test]
fn db_oracle_uses_query_observations_from_history() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "api");

    insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
        .expect("insert");
    insert_observation(&db, &resource, "health_check_pass", &serde_json::json!({}))
        .expect("insert");

    let obs_vec = query_observations(&db, &resource).expect("query");
    let direct_state = derive_lifecycle_state(&resource, &obs_vec);
    assert_eq!(direct_state, LifecycleState::Ready);

    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.lifecycle_state(&resource), LifecycleState::Ready);
}

// -----------------------------------------------------------------------
// termination_success
// -----------------------------------------------------------------------

// l[verify rt.termination.ensure-success]
#[test]
fn db_oracle_termination_success_none_before_exit() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "job");
    insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
        .expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), None);
}

// l[verify rt.termination.ensure-success]
#[test]
fn db_oracle_termination_success_true_on_exit_zero() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "job");
    insert_observation(
        &db,
        &resource,
        "container_exited",
        &serde_json::json!({ "exit_code": 0 }),
    )
    .expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), Some(true));
}

// l[verify rt.termination.ensure-success]
#[test]
fn db_oracle_termination_success_false_on_nonzero_exit() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "job");
    insert_observation(
        &db,
        &resource,
        "container_exited",
        &serde_json::json!({ "exit_code": 1 }),
    )
    .expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), Some(false));
}

// l[verify rt.termination.ensure-success]
#[test]
fn db_oracle_termination_success_uses_latest_exit() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "job");
    // First run fails, second run succeeds (container restarted).
    insert_observation(
        &db,
        &resource,
        "container_exited",
        &serde_json::json!({ "exit_code": 42 }),
    )
    .expect("insert");
    insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
        .expect("insert");
    insert_observation(
        &db,
        &resource,
        "container_exited",
        &serde_json::json!({ "exit_code": 0 }),
    )
    .expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), Some(true));
}

// l[verify rt.termination.ensure-success]
#[test]
fn db_oracle_termination_success_services_terminate_successfully() {
    let db = Db::open_in_memory().expect("open");
    let resource = svc("api");
    insert_observation(&db, &resource, "stop_sent", &serde_json::json!({})).expect("insert");
    insert_observation(&db, &resource, "network_removed", &serde_json::json!({})).expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), Some(true));
}

// l[verify rt.termination.ensure-success]
#[test]
fn db_oracle_termination_success_non_terminal_service_returns_none() {
    let db = Db::open_in_memory().expect("open");
    let resource = ing("api");
    insert_observation(&db, &resource, "ingress_configured", &serde_json::json!({}))
        .expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), None);
}

// l[verify rt.termination.ensure-success]
#[test]
fn db_oracle_termination_success_volume_terminated_is_success() {
    let db = Db::open_in_memory().expect("open");
    let resource = vol("data");
    insert_observation(&db, &resource, "stop_sent", &serde_json::json!({})).expect("insert");
    insert_observation(&db, &resource, "volume_removed", &serde_json::json!({})).expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), Some(true));
}

// l[verify rt.termination.ensure-success]
// Jobs run with `podman --rm` auto-remove on exit. The observer then sees
// ContainerMissing rather than ContainerExited, so no container_exited
// observation is ever written. A healthy unit exit must still report
// success — otherwise ensure_success() fails on every such job.
#[test]
fn db_oracle_termination_success_rm_clean_exit_is_success() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "job");
    insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
        .expect("insert");
    insert_observation(&db, &resource, "container_removed", &serde_json::json!({}))
        .expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), Some(true));
}

// l[verify rt.termination.ensure-success]
// If systemd reports the unit failed, treat as failure even when the
// container's exit code was unobservable (podman --rm raced the
// inspect).
#[test]
fn db_oracle_termination_success_unit_failed_is_failure() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "job");
    insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
        .expect("insert");
    insert_observation(&db, &resource, "unit_failed", &serde_json::json!({})).expect("insert");
    insert_observation(&db, &resource, "container_removed", &serde_json::json!({}))
        .expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), Some(false));
}

// l[verify rt.termination.ensure-success]
// When we DO capture a known exit code, trust it over the fallback —
// a healthy unit exit with a non-zero container exit must still be
// reported as a failure.
#[test]
fn db_oracle_termination_success_exit_code_wins_over_unit_success() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "job");
    insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
        .expect("insert");
    insert_observation(
        &db,
        &resource,
        "container_exited",
        &serde_json::json!({ "exit_code": 7 }),
    )
    .expect("insert");
    insert_observation(&db, &resource, "container_removed", &serde_json::json!({}))
        .expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), Some(false));
}

// l[verify rt.termination.ensure-success]
// A container_exited with exit_code = -1 (podman didn't tell us) is an
// unknown outcome. If no unit_failed is observed, fall through to the
// terminal-seen fallback and succeed.
#[test]
fn db_oracle_termination_success_exit_code_unknown_falls_through() {
    let db = Db::open_in_memory().expect("open");
    let resource = dep("app", "job");
    insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
        .expect("insert");
    insert_observation(
        &db,
        &resource,
        "container_exited",
        &serde_json::json!({ "exit_code": -1 }),
    )
    .expect("insert");
    let oracle = DbWorldOracle::new(DbHandle::from_db(db));
    assert_eq!(oracle.termination_success(&resource), Some(true));
}
