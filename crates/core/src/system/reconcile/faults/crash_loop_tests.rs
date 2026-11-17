use super::*;
use crate::{
    defs::resource::ResourceKind,
    system::reconcile::pods::{CrashLoop, CrashLoopCause},
};

fn app() -> AppName {
    AppName::new("myapp").unwrap()
}

fn instance() -> ResourceInstance {
    ResourceInstance::new_singleton(app(), ResourceKind::Deployment, "web")
}

fn active(db: &Db) -> Vec<faults::FaultRecord> {
    faults::list_active_faults(db, Some(&app()))
        .expect("list")
        .into_iter()
        .filter(|f| f.kind == "crash_loop")
        .collect()
}

fn rate_loop(inst: &ResourceInstance, count: i64) -> CrashLoop {
    CrashLoop {
        instance: inst.clone(),
        cause: CrashLoopCause::RestartRate {
            count,
            window_secs: 1800,
        },
    }
}

fn start_limit_loop(inst: &ResourceInstance) -> CrashLoop {
    CrashLoop {
        instance: inst.clone(),
        cause: CrashLoopCause::StartLimitHit,
    }
}

/// Record that `inst` began running `secs` seconds ago.
fn running_for(db: &Db, inst: &ResourceInstance, secs: i64) {
    crate::runtime::history::insert_observation(
        db,
        inst,
        "container_running",
        &serde_json::json!({}),
    )
    .expect("observe");
    db.conn
        .execute(
            "UPDATE world_observations SET recorded_at = recorded_at - ?1 WHERE instance_id = ?2",
            rusqlite::params![secs * 1000, inst.id.to_hex()],
        )
        .expect("backdate");
}

fn recovery_restart(db: &Db, inst: &ResourceInstance, secs_ago: i64) {
    let subject = crate::runtime::restarts::RestartSubject {
        instance_id: inst.id.to_hex(),
        app: app(),
        resource_type: Some("deployment".to_owned()),
        resource_name: Some("web".to_owned()),
        generation: None,
    };
    crate::runtime::restarts::record(
        db,
        &subject,
        crate::runtime::restarts::Cause::Recovery,
        None,
        jiff::Timestamp::now().as_millisecond() - secs_ago * 1000,
        50,
    )
    .expect("record");
}

// spec: FLT#workload-faults
// spec: AUTO#crash-loop-detection
#[test]
fn the_fault_clears_only_once_the_instance_has_stayed_running_for_the_window() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    apply_crash_loop_faults(&db, &app(), &[rate_loop(&inst, 5)], &[]);
    assert_eq!(active(&db).len(), 1);

    // Up now, but not for the 30 minutes of the default window.
    running_for(&db, &inst, 60);
    apply_crash_loop_faults(&db, &app(), &[], std::slice::from_ref(&inst));
    assert_eq!(active(&db).len(), 1, "running for a minute is not enough");
}

// spec: FLT#workload-faults
// spec: AUTO#crash-loop-detection
#[test]
fn a_recovery_restart_inside_the_window_keeps_the_fault() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    apply_crash_loop_faults(&db, &app(), &[rate_loop(&inst, 5)], &[]);
    running_for(&db, &inst, 3600);
    recovery_restart(&db, &inst, 600);

    apply_crash_loop_faults(&db, &app(), &[], std::slice::from_ref(&inst));
    assert_eq!(
        active(&db).len(),
        1,
        "the unit was back up between crashes, and restarted within the window"
    );
}

// spec: FLT#workload-faults
// spec: AUTO#crash-loop-detection
#[test]
fn an_instance_that_restarted_after_it_began_running_has_not_stayed_running() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    apply_crash_loop_faults(&db, &app(), &[rate_loop(&inst, 5)], &[]);
    running_for(&db, &inst, 3600);
    // It exited and came up again a minute ago.
    for (kind, secs) in [("container_exited", 120), ("container_running", 60)] {
        crate::runtime::history::insert_observation(&db, &inst, kind, &serde_json::json!({}))
            .expect("observe");
        db.conn
            .execute(
                "UPDATE world_observations SET recorded_at = recorded_at - ?1
                 WHERE instance_id = ?2 AND obs_kind = ?3",
                rusqlite::params![secs * 1000, inst.id.to_hex(), kind],
            )
            .expect("backdate");
    }

    apply_crash_loop_faults(&db, &app(), &[], std::slice::from_ref(&inst));
    assert_eq!(active(&db).len(), 1);
}

// spec: FLT#workload-faults
// spec: AUTO#crash-loop-detection
#[test]
fn the_rate_trigger_files_the_fault_and_a_full_window_running_clears_it() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();

    apply_crash_loop_faults(&db, &app(), &[rate_loop(&inst, 5)], &[]);

    let filed = active(&db);
    assert_eq!(filed.len(), 1);
    assert_eq!(filed[0].instance_id.as_deref(), Some(&*inst.id.to_hex()));
    assert_eq!(filed[0].resource_type.as_deref(), Some("deployment"));
    // The description says which trigger fired, and in the operator's units.
    assert!(
        filed[0]
            .description
            .contains("restarted 5 times in the last 30 minutes"),
        "{}",
        filed[0].description
    );

    // The instance has run, with no recovery restart, for longer than the window.
    running_for(&db, &inst, 1801);
    apply_crash_loop_faults(&db, &app(), &[], std::slice::from_ref(&inst));
    assert!(active(&db).is_empty());
}

// spec: FLT#workload-faults
// spec: AUTO#restart-pacing
#[test]
fn start_limit_hit_files_the_fault_below_the_rate_threshold() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();

    // No rate-derived loop is present at all: systemd gave up on its own
    // accounting before the recorded rate reached the threshold.
    apply_crash_loop_faults(&db, &app(), &[start_limit_loop(&inst)], &[]);

    let filed = active(&db);
    assert_eq!(filed.len(), 1);
    assert!(
        filed[0].description.contains("start-limit"),
        "{}",
        filed[0].description
    );
}

// spec: FLT#workload-faults
#[test]
fn a_persisting_crash_loop_is_not_filed_twice() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();

    apply_crash_loop_faults(&db, &app(), &[rate_loop(&inst, 5)], &[]);
    apply_crash_loop_faults(&db, &app(), &[rate_loop(&inst, 6)], &[]);
    apply_crash_loop_faults(&db, &app(), &[start_limit_loop(&inst)], &[]);

    assert_eq!(active(&db).len(), 1);
}

// spec: FLT#workload-faults
#[test]
fn clearing_is_scoped_to_the_instance_that_recovered() {
    let db = Db::open_in_memory().expect("open");
    let one = instance();
    let two = instance();

    apply_crash_loop_faults(&db, &app(), &[rate_loop(&one, 5), rate_loop(&two, 5)], &[]);
    assert_eq!(active(&db).len(), 2);

    running_for(&db, &one, 3600);
    running_for(&db, &two, 3600);
    apply_crash_loop_faults(&db, &app(), &[], std::slice::from_ref(&one));

    let remaining = active(&db);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].instance_id.as_deref(), Some(&*two.id.to_hex()));
}

// spec: OBS#failed-observation
// spec: FLT#filing-and-clearing
#[test]
fn an_observe_failed_fault_clears_on_the_next_successful_observation() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    let other = ResourceInstance::new_singleton(app(), ResourceKind::Volume, "data");
    for i in [&inst, &other] {
        crate::system::reconcile::Reconciler::file_instance_faults(
            &db,
            &app(),
            &[(i.clone(), "inspect failed".to_owned())],
            "observe_failed",
        );
    }
    let active = |db: &Db| {
        faults::list_active_faults(db, Some(&app()))
            .unwrap()
            .into_iter()
            .filter(|f| f.kind == "observe_failed")
            .count()
    };
    assert_eq!(active(&db), 2);

    crate::system::reconcile::Reconciler::clear_observe_failed(
        &db,
        &app(),
        std::slice::from_ref(&inst),
    );

    assert_eq!(
        active(&db),
        1,
        "only the instance that observed cleanly is cleared"
    );
}
