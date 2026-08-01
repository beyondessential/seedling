use super::*;
use crate::{
    defs::resource::ResourceKind,
    runtime::restarts::{self, Initiator},
    system::types::{UnitExit, UnitExitKind},
};

fn app() -> AppName {
    AppName::new("myapp").unwrap()
}

fn instance() -> ResourceInstance {
    ResourceInstance::new_singleton(app(), ResourceKind::Deployment, "web")
}

fn counter(count: u32, exit: Option<UnitExit>) -> pods::RestartCounter {
    pods::RestartCounter { count, exit }
}

/// One observe tick: the counter the supervisor reports for `inst`.
fn tick(db: &Db, inst: &ResourceInstance, count: u32, exit: Option<UnitExit>) -> Vec<CrashLoop> {
    reconcile_counters(db, &app(), &[(inst.clone(), counter(count, exit))], &[])
}

fn records(db: &Db, inst: &ResourceInstance) -> Vec<restarts::RestartRecord> {
    restarts::list(db, None, Some(&inst.id.to_hex()), 1000).expect("list")
}

// r[verify autonomous.restart.record]
#[test]
fn first_sighting_baselines_without_recording() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();

    // The daemon has just come up against a unit that has already restarted
    // three times. Those happened at times seedling cannot know, so they are
    // adopted as the baseline rather than invented into the history.
    tick(&db, &inst, 3, None);
    assert!(records(&db, &inst).is_empty());
    assert_eq!(
        restarts::baseline(&db, &inst.id.to_hex()).expect("baseline"),
        Some(3)
    );
}

// r[verify autonomous.restart.record]
#[test]
fn counter_delta_across_a_restart_is_recorded_with_its_exit() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    tick(&db, &inst, 0, None);

    // The container restarted and came back between two ticks: the unit looks
    // active at both ends and only the counter moved.
    tick(
        &db,
        &inst,
        1,
        Some(UnitExit {
            kind: UnitExitKind::Exited,
            code: 137,
        }),
    );

    let rows = records(&db, &inst);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].initiator, Initiator::Supervisor);
    assert_eq!(rows[0].exit_code, Some(137));
    assert_eq!(rows[0].exit_kind, Some(restarts::ExitKind::Exited));
    assert_eq!(rows[0].resource_name.as_deref(), Some("web"));
}

// r[verify autonomous.restart.record]
#[test]
fn a_burst_between_ticks_records_every_restart() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    tick(&db, &inst, 0, None);
    tick(
        &db,
        &inst,
        4,
        Some(UnitExit {
            kind: UnitExitKind::Signalled,
            code: 9,
        }),
    );

    let rows = records(&db, &inst);
    assert_eq!(rows.len(), 4);
    // Only the most recent run's exit is known.
    assert_eq!(rows[0].exit_code, Some(9));
    assert_eq!(rows[0].exit_kind, Some(restarts::ExitKind::Signalled));
    assert!(rows[1..].iter().all(|r| r.exit_code.is_none()));
}

// r[verify autonomous.restart.record]
#[test]
fn an_unmoved_counter_records_nothing() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    tick(&db, &inst, 0, None);
    tick(&db, &inst, 2, None);
    tick(&db, &inst, 2, None);
    tick(&db, &inst, 2, None);

    assert_eq!(records(&db, &inst).len(), 2);
}

// r[verify autonomous.restart.record]
#[test]
fn a_counter_reset_rebaselines_instead_of_recording_a_negative() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    tick(&db, &inst, 0, None);
    tick(&db, &inst, 5, None);
    assert_eq!(records(&db, &inst).len(), 5);

    // `systemctl reset-failed` (or a recreated unit) zeroes the counter. The
    // drop is a reset, not five restarts unhappening.
    tick(&db, &inst, 0, None);
    assert_eq!(records(&db, &inst).len(), 5);
    assert_eq!(
        restarts::baseline(&db, &inst.id.to_hex()).expect("baseline"),
        Some(0)
    );

    // Counting resumes from the new baseline.
    tick(&db, &inst, 1, None);
    assert_eq!(records(&db, &inst).len(), 6);
}

// r[verify autonomous.restart.record]
#[test]
fn restarts_after_a_reset_are_recorded_not_dropped() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    tick(&db, &inst, 0, None);
    tick(&db, &inst, 5, None);

    // The reset and two further restarts both land between the same pair of
    // ticks, so the counter comes back lower than the baseline but non-zero.
    // Those two restarts really happened.
    tick(&db, &inst, 2, None);
    assert_eq!(records(&db, &inst).len(), 7);
}

// r[verify autonomous.restart.record]
#[test]
fn a_runtime_start_is_recorded_as_runtime_initiated_and_rebaselines() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    tick(&db, &inst, 0, None);
    tick(&db, &inst, 3, None);

    // The reconciler tore the unit down and started a fresh one. systemd's
    // counter for the new transient unit begins at zero.
    reconcile_counters(&db, &app(), &[], std::slice::from_ref(&inst));

    let rows = records(&db, &inst);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].initiator, Initiator::Runtime);
    assert_eq!(
        restarts::baseline(&db, &inst.id.to_hex()).expect("baseline"),
        Some(0)
    );

    // The zeroed counter on the next tick is the expected state, not a reset
    // to record against.
    tick(&db, &inst, 0, None);
    assert_eq!(records(&db, &inst).len(), 4);
}

// r[verify autonomous.restart.record]
#[test]
fn a_first_start_is_not_a_restart() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();

    // No baseline yet: the reconciler has never seen this instance's unit.
    reconcile_counters(&db, &app(), &[], std::slice::from_ref(&inst));
    assert!(records(&db, &inst).is_empty());
}

// r[verify autonomous.restart.rate]
#[test]
fn crossing_the_rate_threshold_reports_a_crash_loop() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    tick(&db, &inst, 0, None);

    for n in 1..5 {
        assert!(
            tick(&db, &inst, n, None).is_empty(),
            "{n} restarts is below the default threshold of 5"
        );
    }

    let loops = tick(&db, &inst, 5, None);
    assert_eq!(loops.len(), 1);
    assert_eq!(loops[0].instance.id, inst.id);
    assert_eq!(
        loops[0].cause,
        CrashLoopCause::RestartRate {
            count: 5,
            window_secs: 1800
        }
    );
}

// r[verify autonomous.restart.rate]
// r[verify autonomous.restart.rate.settings]
#[test]
fn the_threshold_follows_the_operator_setting() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    restarts::set_settings(&db, Some(3), None).expect("set");
    tick(&db, &inst, 0, None);

    assert!(tick(&db, &inst, 2, None).is_empty());
    let loops = tick(&db, &inst, 3, None);
    assert_eq!(loops.len(), 1);
}

// r[verify autonomous.restart.rate]
#[test]
fn a_rolling_update_does_not_read_as_a_crash_burst() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    tick(&db, &inst, 0, None);

    // Ten reconciler-driven restarts in a row — well past the threshold if
    // they counted. They are recorded, but not against the rate.
    for _ in 0..10 {
        reconcile_counters(&db, &app(), &[], std::slice::from_ref(&inst));
        assert!(tick(&db, &inst, 0, None).is_empty());
    }

    assert_eq!(records(&db, &inst).len(), 10);
    assert_eq!(
        restarts::recent_supervisor_count(&db, &inst.id.to_hex(), 1800).expect("count"),
        0
    );
}

// r[verify autonomous.restart.rate]
#[test]
fn instances_are_accounted_for_independently() {
    let db = Db::open_in_memory().expect("open");
    let one = instance();
    let two = instance();
    let counters = |a: u32, b: u32| {
        vec![
            (one.clone(), counter(a, None)),
            (two.clone(), counter(b, None)),
        ]
    };

    reconcile_counters(&db, &app(), &counters(0, 0), &[]);
    let loops = reconcile_counters(&db, &app(), &counters(6, 1), &[]);

    assert_eq!(loops.len(), 1);
    assert_eq!(loops[0].instance.id, one.id);
    assert_eq!(records(&db, &one).len(), 6);
    assert_eq!(records(&db, &two).len(), 1);
}

// r[verify gc.restarts]
#[test]
fn gc_drops_bookkeeping_for_instances_that_no_longer_exist() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    tick(&db, &inst, 0, None);
    tick(&db, &inst, 2, None);
    assert_eq!(records(&db, &inst).len(), 2);

    // The instance was never written to the registry, so from GC's point of
    // view it has been retired.
    let removed = gc(&db).expect("gc");
    assert!(removed >= 2);
    assert!(records(&db, &inst).is_empty());
    assert_eq!(
        restarts::baseline(&db, &inst.id.to_hex()).expect("baseline"),
        None
    );
}
