use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use jiff::Timestamp;
use seedling_protocol::{
    events::OperationTarget,
    names::{AppName, GateName},
};
use serde_json::json;

use super::*;
use crate::{
    defs::resource::{ResourceId, ResourceKind},
    oi::test_support::TestOi,
    runtime::{
        db::Db,
        faults::{self, FaultRecord},
    },
};

fn app() -> AppName {
    AppName::new("demo").unwrap()
}

fn gate(name: &str) -> GateName {
    GateName::new(name).unwrap()
}

fn def(script: &str) -> AppDef {
    let (app, error) = crate::runtime::apps::evaluate_script(
        &app(),
        script,
        &Default::default(),
        &crate::ScriptLimits::default(),
    );
    assert!(error.is_none(), "the definition evaluates: {error:?}");
    (*app.def.load_full()).clone()
}

fn digest(def: &AppDef, cipher: &Cipher, name: &str) -> KeyDigest {
    def.gates[name].key.digest(cipher).unwrap()
}

fn active_faults(db: &Db) -> Vec<(String, String)> {
    let mut faults: Vec<_> = faults::list_active_faults(db, Some(&app()))
        .unwrap()
        .into_iter()
        .map(|FaultRecord { kind, subject, .. }| (kind, subject))
        .collect();
    faults.sort();
    faults
}

fn fail(db: &Db, cipher: &Cipher, def: &AppDef, name: &str, at: Timestamp) {
    store::record_failure(
        db,
        &app(),
        &gate(name),
        &store::FailedRuns::one(digest(def, cipher, name), at, "the body threw", false),
    )
    .unwrap();
}

fn sync(db: &Db, def: &AppDef, cipher: &Cipher, error: Option<&str>) -> Synced {
    sync_db(db, &app(), def, cipher, error, &Remembered::default())
}

fn register(db: &Db) {
    db.conn
        .execute(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation)
             VALUES (?1, 1, 0, 1)",
            [&app()],
        )
        .unwrap();
}

const MIGRATE: &str = r#"
    let api = app.deployment("api").image("docker.io/library/nginx:1");
    let migrate = app.gate("migrate").key("1.0.0").exclusive(api);
    app.deployment("web").image("docker.io/library/nginx:1").after(migrate);
"#;

// spec: FLT#app-faults
#[test]
fn gate_failed_stands_while_the_gate_is_unsatisfied_after_a_failed_run_and_clears_when_it_is_satisfied()
 {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    register(&db);

    sync(&db, &def, &cipher, None);
    assert!(active_faults(&db).is_empty(), "no run has failed");

    fail(
        &db,
        &cipher,
        &def,
        "migrate",
        Timestamp::from_second(100).unwrap(),
    );
    sync(&db, &def, &cipher, None);
    assert_eq!(
        active_faults(&db),
        vec![("gate_failed".to_owned(), "migrate".to_owned())]
    );
    let described = faults::list_active_faults(&db, Some(&app())).unwrap();
    assert!(
        described[0].description.contains("the body threw"),
        "the fault carries how the run ended: {}",
        described[0].description
    );

    // The next run starting ends the fault, as the specification has it.
    store::begin_run(&db, &app(), &gate("migrate")).unwrap();
    sync(&db, &def, &cipher, None);
    assert!(active_faults(&db).is_empty());

    fail(
        &db,
        &cipher,
        &def,
        "migrate",
        Timestamp::from_second(200).unwrap(),
    );
    sync(&db, &def, &cipher, None);
    assert_eq!(active_faults(&db).len(), 1);
    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&def, &cipher, "migrate"),
        1,
    )
    .unwrap();
    sync(&db, &def, &cipher, None);
    assert!(
        active_faults(&db).is_empty(),
        "a satisfied gate has no fault"
    );
}

// spec: FLT#app-faults
// spec: GRUN#gate-state
#[test]
fn a_gate_failed_fault_survives_what_leaves_the_gate_undetermined() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    register(&db);
    fail(
        &db,
        &cipher,
        &def,
        "migrate",
        Timestamp::from_second(100).unwrap(),
    );
    sync(&db, &def, &cipher, None);
    assert_eq!(active_faults(&db).len(), 1);

    // A definition that fails to evaluate is not evidence that the run
    // stopped having failed.
    sync(&db, &def, &cipher, Some("a typo halfway down"));
    assert_eq!(
        active_faults(&db),
        vec![("gate_failed".to_owned(), "migrate".to_owned())],
        "the fault is not cleared for want of knowing"
    );

    // Nor is a record that cannot be read: the fault stands beside the one
    // for the unreadable record.
    db.conn
        .execute(
            "UPDATE gate_records SET key_digest = x'0102' WHERE gate = 'migrate'",
            [],
        )
        .unwrap();
    sync(&db, &def, &cipher, None);
    assert_eq!(
        active_faults(&db),
        vec![
            ("gate_failed".to_owned(), "migrate".to_owned()),
            ("gate_state_unreadable".to_owned(), "migrate".to_owned()),
        ]
    );

    // Once it reads again, what the record says decides.
    db.conn
        .execute("DELETE FROM gate_records WHERE gate = 'migrate'", [])
        .unwrap();
    sync(&db, &def, &cipher, None);
    assert!(active_faults(&db).is_empty());
}

// spec: FLT#app-faults
// spec: GRUN#checks
#[test]
fn gate_check_unavailable_stands_until_a_check_runs_to_an_outcome() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(r#"
        let probe = app.job("probe").image("docker.io/library/tools:1").deadline("5s");
        let ready = app.gate("ready").key(1).check(probe);
        app.deployment("web").image("docker.io/library/nginx:1").after(ready);
        "#);
    register(&db);
    store::record_success(
        &db,
        &app(),
        &gate("ready"),
        &digest(&def, &cipher, "ready"),
        1,
    )
    .unwrap();
    store::record_check_unavailable(
        &db,
        &app(),
        &gate("ready"),
        Timestamp::from_second(100).unwrap(),
        "no podman",
    )
    .unwrap();

    let synced = sync(&db, &def, &cipher, None);
    assert!(matches!(
        synced.gates.state(&gate("ready")),
        Some(GateState::Undetermined(Undetermined::Check(_)))
    ));
    assert_eq!(
        active_faults(&db),
        vec![("gate_check_unavailable".to_owned(), "ready".to_owned())]
    );

    store::record_check_outcome(
        &db,
        &app(),
        &gate("ready"),
        Timestamp::from_second(200).unwrap(),
    )
    .unwrap();
    sync(&db, &def, &cipher, None);
    assert!(active_faults(&db).is_empty());
}

// spec: FLT#app-faults
// spec: GRUN#checks
#[test]
fn a_check_that_could_not_be_run_is_cleared_once_the_definition_no_longer_declares_the_check() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let checked = def(r#"
        let probe = app.job("probe").image("docker.io/library/tools:1").deadline("5s");
        let ready = app.gate("ready").key(1).check(probe);
        app.deployment("web").image("docker.io/library/nginx:1").after(ready);
        "#);
    let unchecked = def(r#"
        let ready = app.gate("ready").key(1);
        app.deployment("web").image("docker.io/library/nginx:1").after(ready);
        "#);
    register(&db);
    store::record_success(
        &db,
        &app(),
        &gate("ready"),
        &digest(&checked, &cipher, "ready"),
        1,
    )
    .unwrap();
    store::record_check_unavailable(
        &db,
        &app(),
        &gate("ready"),
        Timestamp::from_second(100).unwrap(),
        "no podman",
    )
    .unwrap();
    sync(&db, &checked, &cipher, None);
    assert_eq!(
        active_faults(&db),
        vec![("gate_check_unavailable".to_owned(), "ready".to_owned())]
    );

    sync(&db, &unchecked, &cipher, Some("the script threw"));
    let records = store::load_records(&db, &app()).unwrap();
    assert!(
        records[&gate("ready")]
            .as_ref()
            .unwrap()
            .check_unavailable
            .is_some(),
        "a definition that fails to evaluate does not say the check is gone"
    );

    let synced = sync(&db, &unchecked, &cipher, None);
    assert_eq!(
        synced.gates.state(&gate("ready")),
        Some(&GateState::Satisfied)
    );
    let records = store::load_records(&db, &app()).unwrap();
    assert!(
        records[&gate("ready")]
            .as_ref()
            .unwrap()
            .check_unavailable
            .is_none()
    );
    assert!(active_faults(&db).is_empty());
}

// spec: FLT#app-faults
// spec: GRUN#gates-no-longer-declared
#[test]
fn a_gate_the_definition_no_longer_declares_loses_its_faults_and_its_hold_but_keeps_its_record() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let with_gate = def(MIGRATE);
    let without = def(r#"app.deployment("api").image("docker.io/library/nginx:1");"#);
    register(&db);
    fail(
        &db,
        &cipher,
        &with_gate,
        "migrate",
        Timestamp::from_second(100).unwrap(),
    );
    let api = ResourceId {
        kind: ResourceKind::Deployment,
        name: std::sync::Arc::new("api".to_owned()),
    };
    store::set_hold(&db, &app(), &gate("migrate"), &BTreeSet::from([api])).unwrap();
    sync(&db, &with_gate, &cipher, None);
    assert_eq!(active_faults(&db).len(), 1);

    // While the definition cannot be evaluated nothing is learned about it.
    sync(&db, &with_gate, &cipher, Some("broken"));
    assert_eq!(store::load_holds(&db, &app()).unwrap().len(), 1);

    sync(&db, &without, &cipher, None);
    assert!(active_faults(&db).is_empty());
    assert!(store::load_holds(&db, &app()).unwrap().is_empty());
    assert!(
        store::load_records(&db, &app())
            .unwrap()
            .contains_key(&gate("migrate")),
        "the record is kept until uninstall, so a gate declared again resumes from it"
    );
}

// spec: RCN#gate-holds
#[test]
fn a_hold_is_cleared_once_its_gate_is_satisfied_and_not_before() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    register(&db);
    let api = ResourceId {
        kind: ResourceKind::Deployment,
        name: std::sync::Arc::new("api".to_owned()),
    };
    store::set_hold(&db, &app(), &gate("migrate"), &BTreeSet::from([api])).unwrap();

    let synced = sync(&db, &def, &cipher, None);
    assert_eq!(synced.world.unwrap().held.len(), 1);
    assert_eq!(store::load_holds(&db, &app()).unwrap().len(), 1);

    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&def, &cipher, "migrate"),
        1,
    )
    .unwrap();
    let synced = sync(&db, &def, &cipher, None);
    assert!(synced.world.unwrap().held.is_empty());
    assert!(store::load_holds(&db, &app()).unwrap().is_empty());
}

// spec: RCN#gate-holds
#[test]
fn a_hold_is_brought_up_to_what_the_definition_now_selects() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(r#"
        app.deployment("api").image("docker.io/library/nginx:1");
        app.deployment("tasks").image("docker.io/library/nginx:1");
        app.gate("migrate").key(1).exclusive(app);
        "#);
    register(&db);
    let api = ResourceId {
        kind: ResourceKind::Deployment,
        name: std::sync::Arc::new("api".to_owned()),
    };
    store::set_hold(&db, &app(), &gate("migrate"), &BTreeSet::from([api])).unwrap();
    sync(&db, &def, &cipher, None);
    let holds = store::load_holds(&db, &app()).unwrap();
    assert_eq!(
        holds[&gate("migrate")].len(),
        2,
        "a resource added to the selection while the gate holds it is held with the rest"
    );
}

// spec: GRUN#app-setup
#[test]
fn an_app_is_marked_set_up_the_first_time_every_gate_is_satisfied_and_stays_so() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    register(&db);

    sync(&db, &def, &cipher, None);
    db.conn
        .execute("UPDATE registered_apps SET set_up = 0", [])
        .unwrap();
    sync(&db, &def, &cipher, None);
    assert!(!store::is_set_up(&db, &app()).unwrap());

    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&def, &cipher, "migrate"),
        1,
    )
    .unwrap();
    sync(&db, &def, &cipher, None);
    assert!(store::is_set_up(&db, &app()).unwrap());

    // A key that moves afterwards does not undo it.
    let moved = self::def(&MIGRATE.replace("1.0.0", "2.0.0"));
    sync(&db, &moved, &cipher, None);
    assert!(store::is_set_up(&db, &app()).unwrap());
}

// spec: GRUN#app-setup
#[test]
fn an_app_that_cannot_be_judged_is_not_marked_set_up() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    register(&db);
    db.conn
        .execute("UPDATE registered_apps SET set_up = 0", [])
        .unwrap();
    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&def, &cipher, "migrate"),
        1,
    )
    .unwrap();
    sync(&db, &def, &cipher, Some("a typo"));
    assert!(
        !store::is_set_up(&db, &app()).unwrap(),
        "undetermined gates are not satisfied"
    );
}

// ---------------------------------------------------------------------------
// Initiation
// ---------------------------------------------------------------------------

const TWO_GATES: &str = r#"
    let first = app.gate("first").key(1);
    let second = app.gate("second").key(1);
    app.deployment("web").image("docker.io/library/nginx:1.29").after(second);
"#;

fn installed(script: &str) -> TestOi {
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "demo", "script": script }))
        .unwrap();
    oi.install("demo");
    oi
}

fn records(oi: &TestOi) -> store::Records {
    oi.state
        .db
        .call(|db| store::load_records(db, &app()).unwrap())
}

fn valid(oi: &TestOi, name: &str) -> bool {
    records(oi)
        .get(&gate(name))
        .and_then(|r| r.as_ref().ok())
        .is_some_and(|r| r.valid)
}

fn idle(oi: &TestOi) -> bool {
    oi.state.scheduler.lock().active().is_none()
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn drive_once(oi: &TestOi) {
    oi.block_on(drive(&oi.state.engine));
}

// spec: GRUN#initiating-runs
#[test]
fn an_unsatisfied_gate_is_run_by_the_loop_and_the_app_comes_to_be_set_up() {
    let oi = installed(r#"app.gate("seed").key(1);"#);
    assert!(!valid(&oi, "seed"));

    drive_once(&oi);
    wait_until("the run to end", || valid(&oi, "seed") && idle(&oi));

    let set_up = oi.state.db.call(|db| store::is_set_up(db, &app()).unwrap());
    assert!(set_up);
    drive_once(&oi);
    assert!(idle(&oi), "a satisfied gate is not run again");
}

// spec: GRUN#initiating-runs
#[test]
fn due_gates_run_one_at_a_time_in_the_order_the_script_declared_them() {
    let oi = installed(TWO_GATES);

    drive_once(&oi);
    // The first run holds the app, so a second drive starts nothing more.
    drive_once(&oi);
    wait_until("the first run to end", || valid(&oi, "first") && idle(&oi));
    assert!(
        !valid(&oi, "second"),
        "the second was not run while the first held the app"
    );

    drive_once(&oi);
    wait_until("the second run to end", || {
        valid(&oi, "second") && idle(&oi)
    });
}

// spec: GRUN#initiating-runs
#[test]
fn a_run_the_concurrency_rules_do_not_admit_is_retried_and_has_not_failed() {
    let oi = installed(r#"app.gate("seed").key(1);"#);
    // Another operation of the app holds it.
    let phase = parking_lot::Mutex::new(AppPhase::Installed);
    oi.state.scheduler.lock().admit(
        &app(),
        &OperationTarget::Action(seedling_protocol::names::ActionName::new("busy").unwrap()),
        crate::runtime::param_map::ParamMap::default(),
        1,
        "test",
        &phase,
        None,
    );

    drive_once(&oi);
    assert!(
        records(&oi).get(&gate("seed")).is_none_or(|record| {
            let record = record.as_ref().unwrap();
            record.last_failure.is_none() && record.consecutive_failures == 0
        }),
        "a refused admission records no failure"
    );
    let faults = oi.state.db.call(active_faults);
    assert!(faults.is_empty(), "and files no fault: {faults:?}");

    oi.state.scheduler.lock().complete_current(|_| None);
    drive_once(&oi);
    wait_until("the run to end", || valid(&oi, "seed") && idle(&oi));
}

// spec: GRUN#initiating-runs
#[test]
fn a_gate_waiting_out_its_back_off_a_paused_gate_and_a_gate_after_one_are_not_run() {
    let oi = installed(
        r#"
        let alpha = app.gate("alpha").key(1);
        app.gate("bravo").key(1).after(alpha);
        app.gate("paused").key(1);
        "#,
    );
    let (cipher, def) = {
        let reg = oi.state.registry.read();
        (
            Arc::clone(&oi.state.cipher),
            reg.get("demo").unwrap().app.def.load_full(),
        )
    };
    oi.state.db.call(move |db| {
        // `a` failed just now, so its retry is not due for thirty seconds.
        fail(db, &cipher, &def, "alpha", Timestamp::now());
        let key = digest(&def, &cipher, "paused");
        store::record_failure(
            db,
            &app(),
            &gate("paused"),
            &store::FailedRuns::one(
                key,
                Timestamp::from_second(1).unwrap(),
                "cancelled by an operator",
                true,
            ),
        )
        .unwrap();
    });

    drive_once(&oi);
    assert!(idle(&oi), "nothing was initiated");
    assert!(!valid(&oi, "alpha") && !valid(&oi, "bravo") && !valid(&oi, "paused"));
}

// spec: GRUN#gate-state
#[test]
fn a_gate_whose_record_cannot_be_read_is_not_run_and_a_fault_says_so() {
    let oi = installed(r#"app.gate("seed").key(1);"#);
    oi.state.db.call(|db| {
        db.conn
            .execute(
                "INSERT INTO gate_records (app, gate, key_digest, valid) VALUES (?1, 'seed', x'0102', 1)",
                [&app()],
            )
            .unwrap();
    });

    drive_once(&oi);
    assert!(idle(&oi), "an undetermined gate is not run");
    let faults = oi.state.db.call(active_faults);
    assert_eq!(
        faults,
        vec![("gate_state_unreadable".to_owned(), "seed".to_owned())]
    );

    // As soon as the record reads again the gate is judged, and runs.
    oi.state.db.call(|db| {
        db.conn.execute("DELETE FROM gate_records", []).unwrap();
    });
    drive_once(&oi);
    wait_until("the run to end", || valid(&oi, "seed") && idle(&oi));
    let faults = oi.state.db.call(active_faults);
    assert!(faults.is_empty(), "{faults:?}");
}

// spec: GRUN#initiating-runs
#[test]
fn no_gate_is_run_while_what_is_held_or_what_is_stopped_cannot_be_read() {
    for table in ["gate_holds", "stopped_resources"] {
        let oi = installed(r#"app.gate("seed").key(1);"#);
        let rename = |from: &str, to: &str| {
            let sql = format!("ALTER TABLE {from} RENAME TO {to}");
            oi.state
                .db
                .call(move |db| db.conn.execute_batch(&sql).unwrap());
        };
        rename(table, "unreadable");
        drive_once(&oi);
        assert!(
            idle(&oi),
            "nothing is initiated while {table} cannot be read"
        );
        assert!(!valid(&oi, "seed"));

        rename("unreadable", table);
        drive_once(&oi);
        wait_until("the run to end", || valid(&oi, "seed") && idle(&oi));
    }
}

// spec: GRUN#checks
#[test]
fn nothing_is_decided_from_a_hold_set_or_stopped_set_that_cannot_be_read() {
    for table in ["gate_holds", "stopped_resources"] {
        let db = Db::open_in_memory().unwrap();
        let cipher = Cipher::for_tests();
        let def = def(MIGRATE);
        register(&db);
        assert!(sync(&db, &def, &cipher, None).world.is_some());

        db.conn
            .execute_batch(&format!("ALTER TABLE {table} RENAME TO unreadable"))
            .unwrap();
        assert!(
            sync(&db, &def, &cipher, None).world.is_none(),
            "{table} that cannot be read is not read as empty"
        );
    }
}

// spec: GRUN#gates-no-longer-declared
#[test]
fn a_queued_run_of_a_gate_the_definition_no_longer_declares_is_withdrawn() {
    let oi = installed(r#"app.gate("seed").key(1);"#);
    // Another app's operation holds the scheduler, so the run queues.
    let phase = parking_lot::Mutex::new(AppPhase::Installed);
    let other = AppName::new("other").unwrap();
    oi.state.scheduler.lock().admit(
        &other,
        &OperationTarget::Action(seedling_protocol::names::ActionName::new("busy").unwrap()),
        crate::runtime::param_map::ParamMap::default(),
        1,
        "test",
        &phase,
        None,
    );
    let result = oi.state.scheduler.lock().admit(
        &app(),
        &OperationTarget::Gate(gate("gone")),
        crate::runtime::param_map::ParamMap::default(),
        1,
        "gate",
        &phase,
        None,
    );
    assert!(matches!(
        result,
        crate::runtime::scheduler::ScheduleResult::Queued
    ));
    let id = oi
        .state
        .scheduler
        .lock()
        .queue_iter()
        .next()
        .unwrap()
        .operation_id
        .clone();
    crate::runtime::operations::admission::persist_queued(&oi.state.engine, &id).unwrap();
    let persisted = |oi: &TestOi, gate: &'static str| -> i64 {
        oi.state.db.call(move |db| {
            db.conn
                .query_row(
                    "SELECT COUNT(*) FROM queued_operations WHERE gate = ?1",
                    [gate],
                    |r| r.get(0),
                )
                .unwrap()
        })
    };
    assert_eq!(persisted(&oi, "gone"), 1);

    drive_once(&oi);
    assert!(
        oi.state
            .scheduler
            .lock()
            .queue_iter()
            .all(|queued| queued.target.gate() != Some(&gate("gone"))),
        "the run of the undeclared gate is no longer queued"
    );
    assert_eq!(persisted(&oi, "gone"), 0, "its record goes with it");
    // The gate that is declared is due, and takes its place in the queue.
    assert_eq!(persisted(&oi, "seed"), 1);
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

const CHECKED: &str = r#"
    let probe = app.job("probe").image("docker.io/library/tools:1").deadline("60s");
    let ready = app.gate("ready").key(1).check(probe);
    app.deployment("web").image("docker.io/library/nginx:1.29").after(ready);
"#;

/// An installed app whose gate `ready` holds a valid success for its key.
fn satisfied_and_checked() -> TestOi {
    let oi = installed(CHECKED);
    let def = oi
        .state
        .registry
        .read()
        .get("demo")
        .unwrap()
        .app
        .def
        .load_full();
    let digest = digest(&def, &oi.state.cipher, "ready");
    oi.state.db.call(move |db| {
        store::record_success(db, &app(), &gate("ready"), &digest, 1).unwrap();
    });
    oi
}

fn ready_record(oi: &TestOi) -> store::GateRecord {
    records(oi)
        .remove(&gate("ready"))
        .expect("the gate has a record")
        .expect("the record reads")
}

/// Report every check instance that starts as having exited with `code`,
/// as the host would, until the returned flag is raised.
fn exit_checks_with(oi: &TestOi, code: i64) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use crate::runtime::{
        desired::list_dynamic_resources_for_app,
        history::{insert_observation, query_observations},
        identity::{InstanceId, InstanceVariant, ResourceInstance},
    };
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (db, flag) = (oi.state.db.clone(), std::sync::Arc::clone(&stop));
    std::thread::spawn(move || {
        while !flag.load(std::sync::atomic::Ordering::Relaxed) {
            db.call(move |db| {
                for record in list_dynamic_resources_for_app(db, &app()).unwrap() {
                    let instance = ResourceInstance {
                        id: InstanceId::from_hex(&record.instance_id).unwrap(),
                        app: app(),
                        kind: ResourceKind::Job,
                        name: None,
                        variant: InstanceVariant::Singleton,
                        display_name: record.display_name,
                    };
                    if query_observations(db, &instance).unwrap().is_empty() {
                        insert_observation(db, &instance, "container_running", &json!({})).unwrap();
                        insert_observation(
                            db,
                            &instance,
                            "container_exited",
                            &json!({ "exit_code": code }),
                        )
                        .unwrap();
                    }
                }
            });
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    stop
}

// spec: GRUN#checks
#[test]
fn a_check_that_passes_leaves_the_gate_satisfied_and_clears_its_dependants_to_start() {
    let oi = satisfied_and_checked();
    let stop = exit_checks_with(&oi, 0);

    drive_once(&oi);
    wait_until("the check to pass", || {
        oi.state
            .gate_runtime
            .remembered(&app())
            .cleared
            .contains_key(&gate("ready"))
    });
    stop.store(true, std::sync::atomic::Ordering::Relaxed);

    assert!(
        ready_record(&oi).valid,
        "a passing check leaves the record as it was"
    );
    assert!(oi.state.db.call(active_faults).is_empty());
}

// spec: GRUN#checks
#[test]
fn a_check_that_fails_invalidates_the_last_success_so_the_gate_is_run_again() {
    let oi = satisfied_and_checked();
    let stop = exit_checks_with(&oi, 1);

    drive_once(&oi);
    wait_until("the check to fail", || !ready_record(&oi).valid);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    wait_until("the checking to end", || {
        !oi.state.gate_runtime.is_checking(&app(), &gate("ready"))
    });

    let (def, cipher) = {
        let reg = oi.state.registry.read();
        (
            reg.get("demo").unwrap().app.def.load_full(),
            Arc::clone(&oi.state.cipher),
        )
    };
    let state = oi.state.db.call(move |db| {
        evaluate(db, &app(), &def, &cipher, None, &Remembered::default())
            .gates
            .remove("ready")
            .unwrap()
            .state
    });
    assert_eq!(state, GateState::Unsatisfied);
    assert!(
        !oi.state
            .gate_runtime
            .remembered(&app())
            .cleared
            .contains_key(&gate("ready")),
        "a failed check clears nothing for the dependants"
    );
}

// spec: GRUN#checks
#[test]
fn a_check_with_an_interval_runs_every_interval_while_no_run_is_in_progress() {
    let oi = installed(&CHECKED.replace("check(probe)", "check(probe, #{ interval: \"1s\" })"));
    let (def, cipher) = {
        let reg = oi.state.registry.read();
        (
            reg.get("demo").unwrap().app.def.load_full(),
            Arc::clone(&oi.state.cipher),
        )
    };
    let key = digest(&def, &cipher, "ready");
    oi.state.db.call(move |db| {
        store::record_success(db, &app(), &gate("ready"), &key, 1).unwrap();
    });
    // A check has already cleared the dependants to start, so only the
    // interval can make one due.
    oi.state
        .gate_runtime
        .clear_for_start(&app(), &gate("ready"), key);
    let stop = exit_checks_with(&oi, 1);

    drive_once(&oi);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        ready_record(&oi).valid,
        "no check is made before the interval has passed"
    );

    std::thread::sleep(Duration::from_millis(1000));
    drive_once(&oi);
    wait_until("the check to fail", || !ready_record(&oi).valid);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

// spec: GRUN#checks
#[test]
fn a_check_with_an_interval_runs_for_an_unsatisfied_gate_whose_run_is_queued() {
    let oi = installed(&CHECKED.replace("check(probe)", "check(probe, #{ interval: \"1s\" })"));
    let (def, cipher) = {
        let reg = oi.state.registry.read();
        (
            reg.get("demo").unwrap().app.def.load_full(),
            Arc::clone(&oi.state.cipher),
        )
    };
    oi.state
        .db
        .call(move |db| fail(db, &cipher, &def, "ready", Timestamp::now()));
    // Another app's operation holds the scheduler, so the gate's run queues.
    let phase = parking_lot::Mutex::new(AppPhase::Installed);
    for (app, target) in [
        (
            AppName::new("other").unwrap(),
            OperationTarget::Action(seedling_protocol::names::ActionName::new("busy").unwrap()),
        ),
        (app(), OperationTarget::Gate(gate("ready"))),
    ] {
        oi.state.scheduler.lock().admit(
            &app,
            &target,
            crate::runtime::param_map::ParamMap::default(),
            1,
            "test",
            &phase,
            None,
        );
    }
    let stop = exit_checks_with(&oi, 0);

    drive_once(&oi);
    std::thread::sleep(Duration::from_millis(1100));
    drive_once(&oi);
    wait_until("the check to pass", || {
        oi.state
            .gate_runtime
            .remembered(&app())
            .cleared
            .contains_key(&gate("ready"))
    });
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(
        !ready_record(&oi).valid,
        "a passing check leaves the gate unsatisfied, as its record says"
    );
}

// spec: GRUN#checks
#[test]
fn an_interval_check_is_due_from_the_last_check_the_record_holds_after_a_restart() {
    for (checked_ago, due) in [(120, true), (0, false)] {
        let oi =
            installed(&CHECKED.replace("check(probe)", "check(probe, #{ interval: \"60s\" })"));
        let key = {
            let reg = oi.state.registry.read();
            digest(
                &reg.get("demo").unwrap().app.def.load_full(),
                &oi.state.cipher,
                "ready",
            )
        };
        oi.state.db.call(move |db| {
            store::record_success(db, &app(), &gate("ready"), &key, 1).unwrap();
        });
        let at = Timestamp::from_second(Timestamp::now().as_second() - checked_ago).unwrap();
        oi.state.db.call(move |db| {
            store::record_check_outcome(db, &app(), &gate("ready"), at).unwrap();
        });
        // The runtime has restarted: nothing in memory remembers the check,
        // and a check has already cleared the dependants to start, so only
        // the interval can make one due.
        oi.state.gate_runtime.forget_app(&app());
        oi.state
            .gate_runtime
            .clear_for_start(&app(), &gate("ready"), key);
        let stop = exit_checks_with(&oi, 1);

        drive_once(&oi);
        if due {
            wait_until("the check to fail", || !ready_record(&oi).valid);
        } else {
            std::thread::sleep(Duration::from_millis(400));
            assert!(
                ready_record(&oi).valid,
                "no check is made before the interval has passed"
            );
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

// spec: GRUN#checks
#[test]
fn a_check_that_cannot_be_run_leaves_the_gate_undetermined_and_is_retried_after_its_delay_across_a_restart()
 {
    let oi = satisfied_and_checked();
    oi.state.db.call(|db| {
        db.conn
            .execute_batch("DROP TABLE dynamic_resources;")
            .unwrap();
    });

    drive_once(&oi);
    wait_until("the check to be tried", || {
        ready_record(&oi).check_unavailable.is_some()
    });
    wait_until("the checking to end", || {
        !oi.state.gate_runtime.is_checking(&app(), &gate("ready"))
    });
    assert_eq!(ready_record(&oi).check_failures, 1);
    assert!(
        ready_record(&oi).valid,
        "an unrun check does not invalidate"
    );
    let faults = oi.state.db.call(active_faults);
    assert_eq!(
        faults,
        vec![("gate_check_unavailable".to_owned(), "ready".to_owned())]
    );

    // The runtime restarts: nothing in memory remembers the attempt, and the
    // record alone keeps the next one from being made at once.
    oi.state.gate_runtime.forget_app(&app());
    drive_once(&oi);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        ready_record(&oi).check_failures,
        1,
        "no second attempt before the delay has passed"
    );

    // Once the delay has passed the check is tried again.
    oi.state.db.call(|db| {
        db.conn
            .execute(
                "UPDATE gate_records SET check_unavailable_at = '2000-01-01T00:00:00Z'",
                [],
            )
            .unwrap();
    });
    oi.state.gate_runtime.forget_app(&app());
    drive_once(&oi);
    wait_until("the check to be tried again", || {
        ready_record(&oi).check_failures == 2
    });
}

// spec: GRUN#checks
#[test]
fn a_check_that_could_not_be_run_backs_off_from_the_count_the_record_holds_when_it_cannot_be_written()
 {
    let db = Db::open_in_memory().unwrap();
    register(&db);
    let at = Timestamp::from_second(100).unwrap();
    let ready = gate("ready");
    for expected in 1..=3 {
        assert_eq!(
            record_unavailable_check(&db, &app(), &ready, at, "no podman"),
            expected
        );
    }

    db.conn
        .execute_batch(
            "CREATE TRIGGER read_only BEFORE UPDATE ON gate_records
             BEGIN SELECT RAISE(ABORT, 'read-only'); END;",
        )
        .unwrap();
    assert_eq!(
        record_unavailable_check(&db, &app(), &ready, at, "no podman"),
        4,
        "the attempt counts after the three the record holds"
    );
    let held = store::load_records(&db, &app()).unwrap()[&ready]
        .as_ref()
        .unwrap()
        .check_failures;
    assert_eq!(held, 3, "the record was not written");

    db.conn
        .execute_batch("ALTER TABLE gate_records RENAME TO unreadable")
        .unwrap();
    assert_eq!(
        record_unavailable_check(&db, &app(), &ready, at, "no podman"),
        1
    );
}

/// The instances of check Jobs the app has recorded.
fn check_instances(db: &Db) -> Vec<crate::runtime::identity::ResourceInstance> {
    use crate::runtime::{
        desired::list_dynamic_resources_for_app,
        identity::{InstanceId, InstanceVariant, ResourceInstance},
    };
    list_dynamic_resources_for_app(db, &app())
        .unwrap()
        .into_iter()
        .map(|record| ResourceInstance {
            id: InstanceId::from_hex(&record.instance_id).unwrap(),
            app: app(),
            kind: ResourceKind::Job,
            name: None,
            variant: InstanceVariant::Singleton,
            display_name: record.display_name,
        })
        .collect()
}

fn exit(db: &Db, instance: &crate::runtime::identity::ResourceInstance, code: i64) {
    use crate::runtime::history::insert_observation;
    insert_observation(db, instance, "container_running", &json!({})).unwrap();
    insert_observation(
        db,
        instance,
        "container_exited",
        &json!({ "exit_code": code }),
    )
    .unwrap();
}

// spec: GRUN#checks
#[test]
fn a_run_cancels_the_check_in_flight_so_its_failure_cannot_land_after_the_runs_success() {
    use crate::runtime::{
        barrier::CancelToken,
        gates::run::{GateRunInput, run_gate},
    };
    let oi = satisfied_and_checked();
    drive_once(&oi);
    wait_until("the loop's check to start", || {
        oi.state.db.call(check_instances).len() == 1
    });
    let background = oi.state.db.call(check_instances).remove(0);

    let input = {
        let reg = oi.state.registry.read();
        let entry = reg.get("demo").unwrap();
        GateRunInput {
            app: entry.app.clone(),
            gate: gate("ready"),
            op_ctx: oi.state.event_tx.operation(
                app(),
                OperationTarget::Gate(gate("ready")),
                "op-1",
                1,
                None,
            ),
            active_progress: Arc::clone(&entry.active_progress),
            tick_notify: Arc::clone(&entry.tick_notify),
            cancel_token: Arc::new(CancelToken::new()),
            db_path: oi.state.db_path.clone(),
        }
    };
    let state = Arc::clone(&oi.state.engine);
    oi.block_on(async move {
        let run = tokio::spawn({
            let state = Arc::clone(&state);
            async move { run_gate(&state, input).await }
        });
        let until = Instant::now() + Duration::from_secs(60);
        let ending = loop {
            let id = background.id;
            let found = state.db.call(move |db| {
                check_instances(db)
                    .into_iter()
                    .find(|instance| instance.id != id)
            });
            if let Some(found) = found {
                break found;
            }
            assert!(Instant::now() < until, "the run's check never started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        state.db.call(move |db| exit(db, &ending, 0));
        run.await.unwrap();
    });

    assert!(ready_record(&oi).valid, "the run succeeded");
    assert!(
        !oi.state.gate_runtime.is_checking(&app(), &gate("ready")),
        "the loop's check ended before the run did"
    );
    oi.state.db.call(move |db| exit(db, &background, 1));
    std::thread::sleep(Duration::from_millis(1200));
    assert!(
        ready_record(&oi).valid,
        "the loop's check, cancelled, invalidates nothing"
    );
}
