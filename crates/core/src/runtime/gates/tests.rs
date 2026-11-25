use std::collections::BTreeSet;

use jiff::Timestamp;
use seedling_protocol::names::{AppName, GateName};

use super::{
    store::{self, GateRecord},
    *,
};
use crate::{
    defs::{
        app::AppDef,
        gate::AfterTarget,
        resource::{ResourceId, ResourceKind},
    },
    runtime::{db::Db, secrets::Cipher},
};

fn app() -> AppName {
    AppName::new("demo").unwrap()
}

fn gate(name: &str) -> GateName {
    GateName::new(name).unwrap()
}

fn id(kind: ResourceKind, name: &str) -> ResourceId {
    ResourceId {
        kind,
        name: std::sync::Arc::new(name.to_owned()),
    }
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

const MIGRATE: &str = r#"
    let migrate = app.gate("migrate").key("1.0.0").exclusive(app);
    app.deployment("api").image("docker.io/library/nginx:1").after(migrate);
    app.deployment("sidecar").image("docker.io/library/nginx:1").after(migrate);
"#;

fn digest(def: &AppDef, cipher: &Cipher, name: &str) -> KeyDigest {
    def.gates[name].key.digest(cipher).unwrap()
}

fn status(db: &Db, def: &AppDef, cipher: &Cipher, name: &str) -> GateStatus {
    evaluate(db, &app(), def, cipher, None, &Remembered::default())
        .gates
        .remove(name)
        .expect("the gate is declared")
}

fn failure(db: &Db, cipher: &Cipher, def: &AppDef, at: Timestamp) {
    store::record_failure(
        db,
        &app(),
        &gate("migrate"),
        &store::FailedRuns::one(digest(def, cipher, "migrate"), at, "the body threw", false),
    )
    .unwrap();
}

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(seconds).unwrap()
}

// spec: GRUN#gate-state
#[test]
fn a_gate_with_no_record_is_unsatisfied() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    assert_eq!(
        status(&db, &def, &cipher, "migrate").state,
        GateState::Unsatisfied
    );
}

// spec: GRUN#gate-state
#[test]
fn a_gate_is_satisfied_for_the_key_of_its_valid_success_and_no_other() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let first = def(MIGRATE);
    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&first, &cipher, "migrate"),
        3,
    )
    .unwrap();
    assert_eq!(
        status(&db, &first, &cipher, "migrate").state,
        GateState::Satisfied
    );

    let moved = def(&MIGRATE.replace("1.0.0", "1.1.0"));
    assert_eq!(
        status(&db, &moved, &cipher, "migrate").state,
        GateState::Unsatisfied,
        "the key moved, so the success is for another key"
    );

    store::invalidate(&db, &app(), &gate("migrate")).unwrap();
    assert_eq!(
        status(&db, &first, &cipher, "migrate").state,
        GateState::Unsatisfied,
        "an invalidated success is not one that stands"
    );
    assert_eq!(
        store::load_records(&db, &app()).unwrap()[&gate("migrate")]
            .as_ref()
            .unwrap()
            .success_generation,
        Some(3),
        "the generation of the last success is kept, so `last` can be rebuilt"
    );
}

// spec: GRUN#gate-state
#[test]
fn a_gate_keyed_the_same_by_another_secret_key_is_unsatisfied() {
    let db = Db::open_in_memory().unwrap();
    let def = def(MIGRATE);
    let written = Cipher::for_tests_with(&[1; 32]);
    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&def, &written, "migrate"),
        1,
    )
    .unwrap();
    assert_eq!(
        status(&db, &def, &Cipher::for_tests_with(&[2; 32]), "migrate").state,
        GateState::Unsatisfied
    );
}

// spec: GRUN#gate-state
#[test]
fn an_unreadable_record_leaves_the_gate_undetermined_and_not_unsatisfied() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&def, &cipher, "migrate"),
        1,
    )
    .unwrap();
    db.conn
        .execute("UPDATE gate_records SET key_digest = x'0102'", [])
        .unwrap();

    let status = status(&db, &def, &cipher, "migrate");
    assert!(
        matches!(
            status.state,
            GateState::Undetermined(Undetermined::Record(_))
        ),
        "{:?}",
        status.state
    );
    assert!(status.unreadable().is_some());
    assert_eq!(
        decide(&status, Vec::new(), at(1_000_000)),
        Decision::Hold(NotDue::Undetermined),
        "an undetermined gate is not run"
    );
}

// spec: GRUN#gate-state
#[test]
fn a_gate_record_table_that_cannot_be_queried_leaves_every_gate_undetermined() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    db.conn
        .execute("ALTER TABLE gate_records RENAME TO gate_records_away", [])
        .unwrap();
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    assert!(matches!(
        gates.state(&gate("migrate")),
        Some(GateState::Undetermined(Undetermined::Record(_)))
    ));
}

// spec: GRUN#gate-state
#[test]
fn a_record_naming_no_valid_gate_leaves_every_gate_undetermined() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    db.conn
        .execute(
            "INSERT INTO gate_records (app, gate, valid) VALUES (?1, 'not a gate', 1)",
            [app().as_str()],
        )
        .unwrap();
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    assert!(matches!(
        gates.state(&gate("migrate")),
        Some(GateState::Undetermined(Undetermined::Record(_)))
    ));
}

// spec: GRUN#gate-state
#[test]
fn a_definition_that_fails_to_evaluate_leaves_the_gates_undetermined() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    let gates = evaluate(
        &db,
        &app(),
        &def,
        &cipher,
        Some("line 3: oops"),
        &Remembered::default(),
    );
    assert!(matches!(
        gates.state(&gate("migrate")),
        Some(GateState::Undetermined(Undetermined::Definition(_)))
    ));
}

// spec: GRUN#gate-state
#[test]
fn a_valid_success_whose_check_could_not_be_run_is_undetermined() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(r#"
        let probe = app.job("probe").image("docker.io/library/tools:1").deadline("5s");
        let migrate = app.gate("migrate").key("1.0.0").check(probe);
        app.deployment("api").image("docker.io/library/nginx:1").after(migrate);
    "#);
    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&def, &cipher, "migrate"),
        1,
    )
    .unwrap();
    store::record_check_unavailable(&db, &app(), &gate("migrate"), at(10), "no image").unwrap();
    let state = status(&db, &def, &cipher, "migrate");
    assert_eq!(state.check_unavailable(), Some("no image"));
    assert!(matches!(
        state.state,
        GateState::Undetermined(Undetermined::Check(_))
    ));

    store::record_check_outcome(&db, &app(), &gate("migrate"), at(20)).unwrap();
    assert_eq!(
        status(&db, &def, &cipher, "migrate").state,
        GateState::Satisfied
    );
}

// spec: GRUN#gate-state
#[test]
fn a_check_that_could_not_be_run_does_not_leave_a_gate_with_no_check_undetermined() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&def, &cipher, "migrate"),
        1,
    )
    .unwrap();
    store::record_check_unavailable(&db, &app(), &gate("migrate"), at(10), "no image").unwrap();
    assert_eq!(
        status(&db, &def, &cipher, "migrate").state,
        GateState::Satisfied
    );
}

// spec: GRUN#gate-state
#[test]
fn a_failed_check_that_could_not_be_recorded_leaves_the_gate_undetermined() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&def, &cipher, "migrate"),
        1,
    )
    .unwrap();
    let remembered = Remembered {
        unrecorded: [(gate("migrate"), "the database is read-only".to_owned())].into(),
        ..Default::default()
    };
    let gates = evaluate(&db, &app(), &def, &cipher, None, &remembered);
    assert!(matches!(
        gates.state(&gate("migrate")),
        Some(GateState::Undetermined(Undetermined::Unrecorded(_)))
    ));
}

// spec: GRUN#retries
#[test]
fn the_delay_between_failed_runs_doubles_from_thirty_seconds_to_an_hour() {
    let secs = |n| retry_delay(n).as_secs();
    assert_eq!(secs(0), 0);
    assert_eq!(secs(1), 30);
    assert_eq!(secs(2), 60);
    assert_eq!(secs(3), 120);
    assert_eq!(secs(7), 1920);
    assert_eq!(secs(8), 3600);
    assert_eq!(secs(500), 3600, "however long the runs have failed");
}

// spec: GRUN#retries
#[test]
fn a_failed_run_is_retried_once_its_delay_has_passed_and_not_before() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    failure(&db, &cipher, &def, at(1_000));
    let status = status(&db, &def, &cipher, "migrate");
    assert_eq!(status.failed_run(), Some("the body threw"));
    assert_eq!(
        decide(&status, Vec::new(), at(1_029)),
        Decision::Hold(NotDue::BackOff(at(1_030)))
    );
    assert_eq!(decide(&status, Vec::new(), at(1_030)), Decision::Run);

    failure(&db, &cipher, &def, at(2_000));
    let status = self::status(&db, &def, &cipher, "migrate");
    assert_eq!(
        decide(&status, Vec::new(), at(2_059)),
        Decision::Hold(NotDue::BackOff(at(2_060))),
        "a second failure doubles the delay"
    );
    assert_eq!(decide(&status, Vec::new(), at(2_060)), Decision::Run);
}

// spec: GRUN#retries
#[test]
fn a_gate_that_has_failed_for_days_is_run_again_after_the_cap() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    for n in 0..40 {
        failure(&db, &cipher, &def, at(1_000 + n));
    }
    let status = status(&db, &def, &cipher, "migrate");
    assert_eq!(status.record.as_ref().unwrap().consecutive_failures, 40);
    assert_eq!(
        decide(&status, Vec::new(), at(1_039 + 3_600)),
        Decision::Run,
        "there is no terminal give-up"
    );
}

// spec: GRUN#retries
#[test]
fn a_move_of_the_key_makes_the_next_run_due_at_once_and_restarts_the_delay() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let first = def(MIGRATE);
    for n in 0..5 {
        failure(&db, &cipher, &first, at(1_000 + n));
    }
    let moved = def(&MIGRATE.replace("1.0.0", "2.0.0"));
    let status = status(&db, &moved, &cipher, "migrate");
    assert_eq!(decide(&status, Vec::new(), at(1_005)), Decision::Run);

    failure(&db, &cipher, &moved, at(1_005));
    assert_eq!(
        store::load_records(&db, &app()).unwrap()[&gate("migrate")]
            .as_ref()
            .unwrap()
            .consecutive_failures,
        1,
        "the first failure for a key is the first of its run"
    );
}

// spec: GRUN#retries
// spec: GRUN#gate-state
#[test]
fn back_off_holds_across_a_restart_of_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seedling.db");
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    {
        let db = Db::open(&path).unwrap();
        for n in 0..3 {
            failure(&db, &cipher, &def, at(5_000 + n));
        }
    }

    let db = Db::open(&path).unwrap();
    let status = status(&db, &def, &cipher, "migrate");
    assert_eq!(
        decide(&status, Vec::new(), at(5_002 + 119)),
        Decision::Hold(NotDue::BackOff(at(5_002 + 120)))
    );
    assert_eq!(decide(&status, Vec::new(), at(5_002 + 120)), Decision::Run);
}

// spec: GRUN#pauses
#[test]
fn a_pause_holds_across_a_restart_until_an_operator_runs_the_gate_or_the_key_moves() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seedling.db");
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    {
        let db = Db::open(&path).unwrap();
        let key = digest(&def, &cipher, "migrate");
        store::record_failure(
            &db,
            &app(),
            &gate("migrate"),
            &store::FailedRuns::one(key, at(100), "cancelled", true),
        )
        .unwrap();
    }

    let db = Db::open(&path).unwrap();
    let status = status(&db, &def, &cipher, "migrate");
    assert!(status.paused);
    assert_eq!(
        decide(&status, Vec::new(), at(10_000_000)),
        Decision::Hold(NotDue::Paused),
        "no retry runs a paused gate, however long it has been"
    );

    let moved = def_with_key("2.0.0");
    assert!(!self::status(&db, &moved, &cipher, "migrate").paused);

    store::begin_run(&db, &app(), &gate("migrate")).unwrap();
    assert!(
        !self::status(&db, &def, &cipher, "migrate").paused,
        "a run, whoever starts it, ends the pause"
    );
}

// spec: GRUN#pauses
#[test]
fn a_pause_the_key_moved_away_from_does_not_come_back_with_the_key() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    let moved = def_with_key("2.0.0");
    let (key, other) = (
        digest(&def, &cipher, "migrate"),
        digest(&moved, &cipher, "migrate"),
    );
    store::record_failure(
        &db,
        &app(),
        &gate("migrate"),
        &store::FailedRuns::one(key, at(100), "cancelled", true),
    )
    .unwrap();
    store::record_failure(
        &db,
        &app(),
        &gate("migrate"),
        &store::FailedRuns::one(other, at(200), "failed", false),
    )
    .unwrap();

    assert!(
        !status(&db, &def, &cipher, "migrate").paused,
        "the pause ended when the key moved"
    );
}

fn def_with_key(key: &str) -> AppDef {
    def(&MIGRATE.replace("1.0.0", key))
}

// spec: GRUN#initiating-runs
#[test]
fn a_gate_is_not_run_while_a_target_is_not_ready() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(r#"
        let one = app.gate("one").key(1);
        app.gate("two").key(2).after(one);
        "#);
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    let two = &def.gates["two"];
    let unready = unready_targets(&two.after, &gates, &Readiness::default());
    assert_eq!(unready, vec![AfterTarget::Gate(gate("one"))]);
    assert_eq!(
        decide(&gates.gates[&gate("two")], unready.clone(), at(1)),
        Decision::Hold(NotDue::WaitingOn(unready))
    );

    store::record_success(&db, &app(), &gate("one"), &digest(&def, &cipher, "one"), 1).unwrap();
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    assert!(unready_targets(&two.after, &gates, &Readiness::default()).is_empty());
}

// spec: GRUN#initiating-runs
#[test]
fn gates_come_in_the_order_the_script_declared_them() {
    let def = def(r#"
        app.gate("zebra").key(1);
        app.gate("apple").key(1);
        app.gate("mango").key(1);
        "#);
    let names: Vec<_> = declared_order(&def)
        .into_iter()
        .map(|(name, _)| name.as_str().to_owned())
        .collect();
    assert_eq!(names, ["zebra", "apple", "mango"]);
}

// spec: GATE#start-order
#[test]
fn a_job_target_is_ready_by_its_most_recent_instances_outcome() {
    use crate::runtime::{history::insert_observation, identity::ResourceInstance};
    let db = Db::open_in_memory().unwrap();
    let def = def(r#"
        let tool = app.job("tool").image("docker.io/library/tools:1").deadline("60s");
        app.gate("tooled").key(1).after(tool);
        app.deployment("web").image("docker.io/library/nginx:1").after(tool);
        "#);
    let tool = id(ResourceKind::Job, "tool");
    let instance = ResourceInstance::new_singleton(app(), ResourceKind::Job, "tool");
    let ready = |db: &Db| {
        read_readiness(db, &app(), &def, &BTreeSet::new(), &Default::default())
            .ready
            .contains(&tool)
    };
    assert!(!ready(&db), "a job that has not run is not ready");

    insert_observation(&db, &instance, "container_running", &serde_json::json!({})).unwrap();
    assert!(!ready(&db), "a job that is running is not ready");

    insert_observation(
        &db,
        &instance,
        "container_exited",
        &serde_json::json!({ "exit_code": 1 }),
    )
    .unwrap();
    assert!(!ready(&db), "a job that failed is not ready");

    insert_observation(&db, &instance, "container_running", &serde_json::json!({})).unwrap();
    insert_observation(
        &db,
        &instance,
        "container_exited",
        &serde_json::json!({ "exit_code": 0 }),
    )
    .unwrap();
    assert!(ready(&db), "a job whose latest run succeeded is ready");
}

// spec: RT#resources-a-gate-holds
#[test]
fn what_a_runtime_handle_leaves_out_is_what_gates_hold_and_the_workloads_waiting_on_them() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(r#"
        let api = app.deployment("api").image("docker.io/library/nginx:1");
        let migrate = app.gate("migrate").key(1).exclusive(api);
        let seed = app.gate("seed").key(1).after(migrate);
        app.deployment("web").image("docker.io/library/nginx:1").after(seed);
        app.deployment("plain").image("docker.io/library/nginx:1");
        "#);
    let api = id(ResourceKind::Deployment, "api");
    let web = id(ResourceKind::Deployment, "web");
    let plain = id(ResourceKind::Deployment, "plain");

    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    let left_out = rt_exclusions(&def, &gates, None).unwrap();
    assert!(left_out.contains(&web), "web waits through seed on migrate");
    assert!(!left_out.contains(&plain));

    store::set_hold(
        &db,
        &app(),
        &gate("migrate"),
        &BTreeSet::from([api.clone()]),
    )
    .unwrap();
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    let left_out = rt_exclusions(&def, &gates, None).unwrap();
    assert!(left_out.contains(&api), "migrate holds api");

    let own = rt_exclusions(&def, &gates, Some(&gate("migrate"))).unwrap();
    assert!(
        !own.contains(&api),
        "the gate's own body is not kept from its hold"
    );
    assert!(
        own.contains(&web),
        "web still waits on seed, which is unsatisfied"
    );
    let own_seed = rt_exclusions(&def, &gates, Some(&gate("seed"))).unwrap();
    assert!(
        own_seed.contains(&api),
        "another gate's hold is kept from a body"
    );
    assert!(own_seed.contains(&web), "web waits on migrate through seed");

    for name in ["migrate", "seed"] {
        store::record_success(&db, &app(), &gate(name), &digest(&def, &cipher, name), 1).unwrap();
    }
    store::clear_hold(&db, &app(), &gate("migrate")).unwrap();
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    assert!(rt_exclusions(&def, &gates, None).unwrap().is_empty());
}

// spec: RT#resources-a-gate-holds
// spec: RCN#gate-holds
#[test]
fn holds_that_cannot_be_read_are_an_error_and_not_an_empty_set() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    db.conn
        .execute("ALTER TABLE gate_holds RENAME TO gate_holds_away", [])
        .unwrap();
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    assert!(gates.holds.is_err());
    assert!(rt_exclusions(&def, &gates, None).is_err());

    let stopped = Default::default();
    let view = view(
        &db,
        &app(),
        &def,
        &gates,
        &stopped,
        &Remembered::default(),
        None,
    );
    assert!(
        view.held.is_empty(),
        "nothing is stopped for what cannot be read"
    );
    for name in ["api", "sidecar"] {
        assert!(
            view.waiting
                .contains_key(&id(ResourceKind::Deployment, name)),
            "{name} starts nothing while its hold cannot be read"
        );
    }
}

// spec: GATE#body
#[test]
fn what_a_running_body_started_waits_on_nothing_but_the_other_targets_it_is_ordered_after() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(r#"
        let config = app.gate("config").key(1);
        let api = app.deployment("api").image("docker.io/library/nginx:1");
        let migrate = app.gate("migrate").key(1).exclusive(api);
        api.after([config, migrate]);
        app.deployment("worker").image("docker.io/library/nginx:1").after(migrate);
        app.deployment("web").image("docker.io/library/nginx:1").after(migrate);
        "#);
    let (api, worker, web) = (
        id(ResourceKind::Deployment, "api"),
        id(ResourceKind::Deployment, "worker"),
        id(ResourceKind::Deployment, "web"),
    );
    let body = BodyStarts {
        gate: gate("migrate"),
        started: BTreeSet::from([api.clone(), worker.clone()]),
    };
    let waiting = |db: &Db| {
        let gates = evaluate(db, &app(), &def, &cipher, None, &Remembered::default());
        view(
            db,
            &app(),
            &def,
            &gates,
            &Default::default(),
            &Remembered::default(),
            Some(&body),
        )
        .waiting
    };

    let view = waiting(&db);
    assert_eq!(
        view.get(&api),
        Some(&vec![Wait::Target(AfterTarget::Gate(gate("config")))]),
        "api still waits on the other gate it is ordered after"
    );
    assert!(
        !view.contains_key(&worker),
        "what the body started does not wait on the gate"
    );
    assert_eq!(
        view.get(&web),
        Some(&vec![Wait::Target(AfterTarget::Gate(gate("migrate")))]),
        "what the body did not start still waits"
    );

    store::record_success(
        &db,
        &app(),
        &gate("config"),
        &digest(&def, &cipher, "config"),
        1,
    )
    .unwrap();
    assert!(!waiting(&db).contains_key(&api));

    db.conn
        .execute("ALTER TABLE gate_holds RENAME TO gate_holds_away", [])
        .unwrap();
    assert!(
        waiting(&db).contains_key(&api),
        "the selection still waits while its hold cannot be read"
    );
}

// spec: RCN#gate-holds
#[test]
fn a_hold_covers_the_selection_the_definition_gives_beyond_what_the_record_names() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    let api = id(ResourceKind::Deployment, "api");
    let sidecar = id(ResourceKind::Deployment, "sidecar");
    let gone = id(ResourceKind::Deployment, "gone");
    store::set_hold(
        &db,
        &app(),
        &gate("migrate"),
        &BTreeSet::from([api.clone(), gone.clone()]),
    )
    .unwrap();
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    let held = held_resources(&def, gates.holds.as_ref().unwrap());
    assert_eq!(
        held.keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([api, sidecar, gone]),
        "what a change added is held at once, and what it removed stays held until the record drops it"
    );
}

// spec: RCN#gate-holds
#[test]
fn a_hold_stays_held_across_a_restart_of_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seedling.db");
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    let api = id(ResourceKind::Deployment, "api");
    {
        let db = Db::open(&path).unwrap();
        store::set_hold(
            &db,
            &app(),
            &gate("migrate"),
            &BTreeSet::from([api.clone()]),
        )
        .unwrap();
    }

    let db = Db::open(&path).unwrap();
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    let view = view(
        &db,
        &app(),
        &def,
        &gates,
        &Default::default(),
        &Remembered::default(),
        None,
    );
    assert_eq!(
        view.held.get(&api),
        Some(&BTreeSet::from([gate("migrate")])),
        "a restart does not release what a gate holds"
    );
}

// spec: GRUN#app-setup
#[test]
fn an_app_is_set_up_once_every_gate_is_satisfied_at_once() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(r#"
        app.gate("one").key(1);
        app.gate("two").key(2);
        "#);
    store::record_success(&db, &app(), &gate("one"), &digest(&def, &cipher, "one"), 1).unwrap();
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    assert!(!gates.all_satisfied());
    store::record_success(&db, &app(), &gate("two"), &digest(&def, &cipher, "two"), 1).unwrap();
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    assert!(gates.all_satisfied());
}

// spec: GATE#start-order
#[test]
fn a_gate_with_a_check_clears_a_dependant_to_start_from_stopped_only_once_a_check_passed() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(r#"
        let probe = app.job("probe").image("docker.io/library/tools:1").deadline("30s");
        let ready = app.gate("ready").key(1).check(probe);
        app.deployment("web").image("docker.io/library/nginx:1").after(ready);
        "#);
    store::record_success(
        &db,
        &app(),
        &gate("ready"),
        &digest(&def, &cipher, "ready"),
        1,
    )
    .unwrap();
    let web = id(ResourceKind::Deployment, "web");
    let waits = |remembered: &Remembered| {
        let gates = evaluate(&db, &app(), &def, &cipher, None, remembered);
        view(
            &db,
            &app(),
            &def,
            &gates,
            &Default::default(),
            remembered,
            None,
        )
        .waiting
        .get(&web)
        .cloned()
    };
    let on_check = Some(vec![Wait::Check(gate("ready"))]);
    assert_eq!(
        waits(&Remembered::default()),
        on_check,
        "web is down and no check has passed, and it waits on the check rather than the satisfied gate"
    );
    let cleared = Remembered {
        cleared: [(gate("ready"), digest(&def, &cipher, "ready"))].into(),
        ..Default::default()
    };
    assert_eq!(
        waits(&cleared),
        None,
        "a check passed for the gate's current key"
    );
    let stale = Remembered {
        cleared: [(
            gate("ready"),
            Cipher::for_tests_with(&[9; 32]).key_digest(b"1").unwrap(),
        )]
        .into(),
        ..Default::default()
    };
    assert_eq!(
        waits(&stale),
        on_check,
        "a check that passed for another key clears nothing"
    );
}

// spec: GATE#start-order
#[test]
fn a_dependant_waits_on_the_targets_that_are_not_ready_and_is_told_which() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    let gates = evaluate(&db, &app(), &def, &cipher, None, &Remembered::default());
    let view = view(
        &db,
        &app(),
        &def,
        &gates,
        &Default::default(),
        &Remembered::default(),
        None,
    );
    assert_eq!(
        view.waiting.get(&id(ResourceKind::Deployment, "api")),
        Some(&vec![Wait::Target(AfterTarget::Gate(gate("migrate")))])
    );
}

// spec: GATE#start-order
#[test]
fn a_gate_record_is_unaffected_by_the_resources_of_other_apps() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    store::record_success(
        &db,
        &AppName::new("other").unwrap(),
        &gate("migrate"),
        &digest(&def, &cipher, "migrate"),
        1,
    )
    .unwrap();
    assert_eq!(
        status(&db, &def, &cipher, "migrate").state,
        GateState::Unsatisfied
    );
}

// spec: GRUN#uninstall
#[test]
fn uninstall_deletes_the_records_the_holds_and_the_setup_of_the_app_alone() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let def = def(MIGRATE);
    for name in ["demo", "other"] {
        let name = AppName::new(name).unwrap();
        db.conn
            .execute(
                "INSERT INTO registered_apps (name, installed, uninstalling, current_generation) \
                 VALUES (?1, 1, 0, 1)",
                [name.as_str()],
            )
            .unwrap();
        store::record_success(
            &db,
            &name,
            &gate("migrate"),
            &digest(&def, &cipher, "migrate"),
            1,
        )
        .unwrap();
        store::set_hold(
            &db,
            &name,
            &gate("migrate"),
            &BTreeSet::from([id(ResourceKind::Deployment, "api")]),
        )
        .unwrap();
        store::mark_set_up(&db, &name).unwrap();
    }
    store::delete_app_state(&db, &app()).unwrap();
    assert!(store::load_records(&db, &app()).unwrap().is_empty());
    assert!(store::load_holds(&db, &app()).unwrap().is_empty());
    assert!(!store::is_set_up(&db, &app()).unwrap());
    let other = AppName::new("other").unwrap();
    assert_eq!(store::load_records(&db, &other).unwrap().len(), 1);
    assert_eq!(store::load_holds(&db, &other).unwrap().len(), 1);
    assert!(store::is_set_up(&db, &other).unwrap());
}

fn left_out(exclusions: &Exclusions, name: &str) -> bool {
    exclusions
        .leaves_out(ResourceKind::Deployment, name)
        .unwrap()
}

// spec: RT#resources-a-gate-holds
// spec: GRUN#gate-state
#[test]
fn a_closure_is_kept_from_every_gated_workload_while_the_definition_fails_to_evaluate() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let current = std::sync::Arc::new(def(MIGRATE));
    store::record_success(
        &db,
        &app(),
        &gate("migrate"),
        &digest(&current, &cipher, "migrate"),
        1,
    )
    .unwrap();
    let exclusions = |def: &AppDef, error: Option<&str>| {
        let current = CurrentDefinition {
            def: std::sync::Arc::clone(&current),
            error: error.map(str::to_owned),
        };
        Exclusions::for_closure(
            &db,
            &app(),
            def,
            Some(&current),
            &cipher,
            &Remembered::default(),
            None,
        )
    };

    let satisfied = exclusions(&current, None);
    assert!(!left_out(&satisfied, "api"), "the gate is satisfied");

    let generation = def(MIGRATE);
    for def in [&*current, &generation] {
        let failing = exclusions(def, Some("the script threw"));
        for name in ["api", "sidecar"] {
            assert!(
                left_out(&failing, name),
                "{name} waits on a gate whose state cannot be told"
            );
        }
    }
}

// spec: RT#resources-a-gate-holds
#[test]
fn a_closure_is_kept_from_what_the_apps_current_definition_gates() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let generation = def(r#"
        app.deployment("api").image("docker.io/library/nginx:1");
        app.deployment("web").image("docker.io/library/nginx:1");
        "#);
    let current = CurrentDefinition {
        def: std::sync::Arc::new(def(r#"
            let api = app.deployment("api").image("docker.io/library/nginx:1");
            let migrate = app.gate("migrate").key(1).exclusive(api);
            app.deployment("web").image("docker.io/library/nginx:1").after(migrate);
            "#)),
        error: None,
    };
    store::set_hold(
        &db,
        &app(),
        &gate("migrate"),
        &BTreeSet::from([id(ResourceKind::Deployment, "api")]),
    )
    .unwrap();
    let exclusions = |current: Option<&CurrentDefinition>| {
        Exclusions::for_closure(
            &db,
            &app(),
            &generation,
            current,
            &cipher,
            &Remembered::default(),
            None,
        )
    };

    let alone = exclusions(None);
    assert!(
        !left_out(&alone, "web"),
        "the operation's generation orders web after nothing"
    );

    let both = exclusions(Some(&current));
    assert!(
        left_out(&both, "web"),
        "the current definition orders web after an unsatisfied gate"
    );
    assert!(left_out(&both, "api"), "what the gate holds is left out");
}

// spec: GRUN#checks
#[test]
fn the_last_check_outlasts_a_restart_and_the_next_interval_check_is_counted_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seedling.db");
    {
        let db = Db::open(&path).unwrap();
        store::record_check_outcome(&db, &app(), &gate("ready"), at(1_000)).unwrap();
    }

    let db = Db::open(&path).unwrap();
    let record = store::load_records(&db, &app())
        .unwrap()
        .remove(&gate("ready"))
        .unwrap()
        .unwrap();
    assert_eq!(record.last_checked_at, Some(at(1_000)));
    assert!(!record.valid, "a check alone makes no success");
    assert_eq!(
        interval_check_due(record.last_checked_at.unwrap(), Duration::from_secs(60)),
        at(1_060)
    );
}

// spec: APPS#gates
#[test]
fn a_paused_gate_is_reported_paused_whatever_state_the_runtime_determined() {
    let status = |state, paused| GateStatus {
        state,
        key_digest: None,
        record: None,
        paused,
        record_error: None,
    };
    let undetermined = GateState::Undetermined(Undetermined::Definition("it throws".to_owned()));
    assert_eq!(status(undetermined.clone(), true).reported(false), "paused");
    assert_eq!(
        status(GateState::Unsatisfied, true).reported(false),
        "paused"
    );
    assert_eq!(status(undetermined.clone(), true).reported(true), "running");
    assert_eq!(status(undetermined, false).reported(false), "undetermined");
}
