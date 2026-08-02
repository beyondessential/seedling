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

// r[verify fault.crash-loop]
// r[verify autonomous.restart.rate]
#[test]
fn the_rate_trigger_files_the_fault_and_observed_healthy_clears_it() {
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

    // The instance comes back healthy on a later tick.
    apply_crash_loop_faults(&db, &app(), &[], std::slice::from_ref(&inst));
    assert!(active(&db).is_empty());
}

// r[verify fault.crash-loop]
// r[verify autonomous.restart.start-limit-hit]
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

// r[verify fault.crash-loop]
#[test]
fn a_persisting_crash_loop_is_not_filed_twice() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();

    apply_crash_loop_faults(&db, &app(), &[rate_loop(&inst, 5)], &[]);
    apply_crash_loop_faults(&db, &app(), &[rate_loop(&inst, 6)], &[]);
    apply_crash_loop_faults(&db, &app(), &[start_limit_loop(&inst)], &[]);

    assert_eq!(active(&db).len(), 1);
}

// r[verify fault.crash-loop]
#[test]
fn clearing_is_scoped_to_the_instance_that_recovered() {
    let db = Db::open_in_memory().expect("open");
    let one = instance();
    let two = instance();

    apply_crash_loop_faults(&db, &app(), &[rate_loop(&one, 5), rate_loop(&two, 5)], &[]);
    assert_eq!(active(&db).len(), 2);

    apply_crash_loop_faults(&db, &app(), &[], std::slice::from_ref(&one));

    let remaining = active(&db);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].instance_id.as_deref(), Some(&*two.id.to_hex()));
}
