use std::time::{Duration, Instant};

use seedling_protocol::{
    events::OperationTarget,
    names::{AppName, GateName},
};
use serde_json::json;

use super::*;
use crate::{
    oi::test_support::TestOi,
    runtime::{
        desired::list_dynamic_resources_for_app,
        faults,
        history::insert_observation,
        identity::{InstanceId, InstanceVariant},
        secrets::Cipher,
    },
};

fn app_name() -> AppName {
    AppName::new("demo").unwrap()
}

fn gate(name: &str) -> GateName {
    GateName::new(name).unwrap()
}

const SCRIPT: &str = r#"
    let api = app.deployment("api").image("docker.io/library/nginx:1.29");
    let migrate = app.gate("migrate").key("1.0.0").exclusive(api);
    app.deployment("web").image("docker.io/library/nginx:1.29").after(migrate);
"#;

fn installed(oi: &TestOi, script: &str) {
    oi.call("/apps/create", json!({ "app": "demo", "script": script }))
        .unwrap();
    oi.install("demo");
}

fn key(oi: &TestOi, name: &str) -> KeyDigest {
    let def = oi
        .state
        .registry
        .read()
        .get("demo")
        .unwrap()
        .app
        .def
        .load_full();
    def.gates[name].key.digest(&Cipher::for_tests()).unwrap()
}

fn record(oi: &TestOi, name: &str) -> GateRecord {
    oi.state
        .db
        .call(|db| store::load_records(db, &app_name()).unwrap())
        .remove(&gate(name))
        .expect("the gate has a record")
        .expect("the record reads")
}

fn holds(oi: &TestOi) -> usize {
    oi.state
        .db
        .call(|db| store::load_holds(db, &app_name()).unwrap())
        .len()
}

/// Run `name` once, as the dispatcher does, and report the kinds of the
/// events it emitted.
fn run_once(oi: &TestOi, name: &str, cancelled: bool) -> Vec<String> {
    let mut events = oi.state.event_tx.subscribe();
    let (app, active_progress, tick_notify) = {
        let reg = oi.state.registry.read();
        let entry = reg.get("demo").unwrap();
        (
            entry.app.clone(),
            Arc::clone(&entry.active_progress),
            Arc::clone(&entry.tick_notify),
        )
    };
    let cancel_token = Arc::new(CancelToken::new());
    if cancelled {
        cancel_token.request();
    }
    let op_ctx = oi.state.event_tx.operation(
        app_name(),
        OperationTarget::Gate(gate(name)),
        "op-1",
        1,
        None,
    );
    oi.block_on(run_gate(
        &oi.state,
        GateRunInput {
            app,
            gate: gate(name),
            op_ctx,
            active_progress,
            tick_notify,
            cancel_token,
            db_path: oi.state.db_path.clone(),
        },
    ));
    let mut kinds = Vec::new();
    while let Ok(event) = events.try_recv() {
        kinds.push(
            serde_json::to_value(&event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    kinds
}

fn faults_of(oi: &TestOi) -> Vec<String> {
    oi.state.db.call(|db| {
        let mut kinds: Vec<_> = faults::list_active_faults(db, Some(&app_name()))
            .unwrap()
            .into_iter()
            .map(|f| f.kind)
            .collect();
        kinds.sort();
        kinds
    })
}

fn persisted_operations(oi: &TestOi) -> i64 {
    oi.state.db.call(|db| {
        db.conn
            .query_row("SELECT COUNT(*) FROM current_operation", [], |r| r.get(0))
            .unwrap()
    })
}

// spec: GRUN#outcome
#[test]
fn a_run_that_succeeds_records_the_digest_of_its_key_and_its_generation_and_releases_its_hold() {
    let oi = TestOi::new();
    installed(&oi, SCRIPT);

    let kinds = run_once(&oi, "migrate", false);
    assert!(kinds.contains(&"OperationCompleted".to_owned()), "{kinds:?}");

    let record = record(&oi, "migrate");
    assert!(record.valid);
    assert_eq!(record.key_digest, Some(key(&oi, "migrate")));
    assert_eq!(record.success_generation, Some(1));
    assert_eq!(record.consecutive_failures, 0);
    assert_eq!(holds(&oi), 0, "the hold is cleared once the gate is satisfied");
    assert_eq!(persisted_operations(&oi), 0, "the operation record is cleared");
    assert!(faults_of(&oi).is_empty());
}

// spec: GRUN#preparation
#[test]
fn a_run_starts_by_invalidating_the_last_success_and_keeps_its_generation_for_last() {
    let oi = TestOi::with_unreadable_containers();
    installed(&oi, SCRIPT);
    let digest = key(&oi, "migrate");
    oi.state.db.call(move |db| {
        store::record_success(db, &app_name(), &gate("migrate"), &digest, 1).unwrap();
    });

    let kinds = run_once(&oi, "migrate", false);
    assert!(kinds.contains(&"OperationFailed".to_owned()), "{kinds:?}");

    let record = record(&oi, "migrate");
    assert!(!record.valid, "the gate is unsatisfied until a run succeeds");
    assert_eq!(
        record.success_generation,
        Some(1),
        "the generation of the last success is kept, so `last` can be rebuilt"
    );
}

// spec: GRUN#preparation
#[test]
fn a_run_whose_images_cannot_be_pulled_fails_with_nothing_stopped() {
    let oi = TestOi::with_unreadable_containers();
    installed(&oi, SCRIPT);

    let kinds = run_once(&oi, "migrate", false);
    assert!(kinds.contains(&"OperationFailed".to_owned()), "{kinds:?}");
    assert!(!kinds.contains(&"OperationCompleted".to_owned()));

    let record = record(&oi, "migrate");
    assert!(!record.valid);
    assert_eq!(record.consecutive_failures, 1);
    assert!(
        record
            .last_failure
            .as_deref()
            .is_some_and(|how| how.contains("could not be pulled")),
        "{:?}",
        record.last_failure
    );
    assert!(record.last_failure_at.is_some());
    assert_eq!(record.failed_key_digest, Some(key(&oi, "migrate")));
    assert_eq!(holds(&oi), 0, "the selection was never held, so nothing was stopped");
    assert!(record.paused_key_digest.is_none(), "a failed run does not pause the gate");
    assert_eq!(faults_of(&oi), vec!["gate_failed"]);
    assert_eq!(persisted_operations(&oi), 0);
}

// spec: GRUN#retries
#[test]
fn consecutive_failed_runs_for_one_key_count_up_and_a_success_resets_them() {
    let oi = TestOi::with_unreadable_containers();
    installed(&oi, SCRIPT);
    run_once(&oi, "migrate", false);
    run_once(&oi, "migrate", false);
    assert_eq!(record(&oi, "migrate").consecutive_failures, 2);

    let digest = key(&oi, "migrate");
    oi.state.db.call(move |db| {
        store::record_success(db, &app_name(), &gate("migrate"), &digest, 1).unwrap();
    });
    let after = record(&oi, "migrate");
    assert_eq!(after.consecutive_failures, 0);
    assert!(after.last_failure.is_none());
}

// spec: GRUN#pauses
#[test]
fn a_cancelled_run_pauses_the_gate_for_its_key_and_files_gate_failed() {
    let oi = TestOi::new();
    installed(&oi, SCRIPT);

    let kinds = run_once(&oi, "migrate", true);
    assert!(kinds.contains(&"OperationCancelled".to_owned()), "{kinds:?}");
    assert!(!kinds.contains(&"OperationFailed".to_owned()));

    let after = record(&oi, "migrate");
    assert!(!after.valid);
    assert_eq!(after.paused_key_digest, Some(key(&oi, "migrate")));
    assert_eq!(
        faults_of(&oi),
        vec!["gate_failed"],
        "and no operation_cancelled fault beside it"
    );

    // The next run, by whoever starts it, ends the pause.
    oi.state.db.call(|db| {
        store::begin_run(db, &app_name(), &gate("migrate")).unwrap();
    });
    assert!(record(&oi, "migrate").paused_key_digest.is_none());
}

// spec: GRUN#outcome
#[test]
fn a_run_whose_success_cannot_be_recorded_counts_as_failed_and_keeps_its_hold() {
    let oi = TestOi::new();
    installed(&oi, SCRIPT);
    oi.state.db.call(|db| {
        db.conn
            .execute_batch(
                "CREATE TRIGGER refuse_success BEFORE UPDATE ON gate_records
                 WHEN NEW.valid = 1
                 BEGIN SELECT RAISE(ABORT, 'no room'); END;
                 CREATE TRIGGER refuse_first_success BEFORE INSERT ON gate_records
                 WHEN NEW.valid = 1
                 BEGIN SELECT RAISE(ABORT, 'no room'); END;",
            )
            .unwrap();
    });

    let kinds = run_once(&oi, "migrate", false);
    assert!(kinds.contains(&"OperationFailed".to_owned()), "{kinds:?}");
    assert!(!kinds.contains(&"OperationCompleted".to_owned()));

    let record = record(&oi, "migrate");
    assert!(!record.valid, "the gate is not counted as satisfied");
    assert!(
        record
            .last_failure
            .as_deref()
            .is_some_and(|how| how.contains("could not be recorded")),
        "{:?}",
        record.last_failure
    );
    assert_eq!(holds(&oi), 1, "an unsatisfied gate keeps holding its selection");
    assert_eq!(faults_of(&oi), vec!["gate_failed"]);
}

// spec: GRUN#preparation
#[test]
fn a_run_that_cannot_invalidate_the_last_success_fails_before_anything_else_is_done() {
    let oi = TestOi::new();
    installed(&oi, SCRIPT);
    oi.state.db.call(|db| {
        db.conn.execute_batch("DROP TABLE gate_holds;").unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER refuse_invalidation BEFORE INSERT ON gate_records
                 BEGIN SELECT RAISE(ABORT, 'read only'); END;",
            )
            .unwrap();
    });

    let kinds = run_once(&oi, "migrate", false);
    assert!(kinds.contains(&"OperationFailed".to_owned()), "{kinds:?}");
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

const CHECKED: &str = r#"
    let probe = app.job("probe").image("docker.io/library/tools:1").deadline(60);
    let ready = app.gate("ready").key(1).check(probe);
    app.deployment("web").image("docker.io/library/nginx:1.29").after(ready);
"#;

fn probe_job(oi: &TestOi) -> Resource {
    let def = oi.state.registry.read().get("demo").unwrap().app.def.load_full();
    let (_, job) = def
        .resources
        .iter()
        .find(|(id, _)| id.kind == ResourceKind::Job)
        .unwrap();
    job.clone()
}

/// Run the check of `ready` while the host reports the check's instance as
/// `observed` says, and return how it came out.
fn check_with(
    oi: &TestOi,
    cancel: Option<Arc<CancelToken>>,
    observed: impl Fn(&Db, &ResourceInstance) + Send + 'static,
) -> CheckOutcome {
    let job = probe_job(oi);
    let state = Arc::clone(&oi.state);
    oi.block_on(async move {
        let runner = tokio::spawn({
            let state = Arc::clone(&state);
            let cancel = cancel.clone();
            async move { run_check(&state, &app_name(), &gate("ready"), job, cancel.as_ref()).await }
        });
        if cancel.is_none() {
            let until = Instant::now() + Duration::from_secs(60);
            let instance = loop {
                // The check that is waiting is the one whose instance has
                // not been observed yet.
                let found = state.db.call(|db| {
                    list_dynamic_resources_for_app(db, &app_name())
                        .unwrap()
                        .into_iter()
                        .map(|record| ResourceInstance {
                            id: InstanceId::from_hex(&record.instance_id).unwrap(),
                            app: app_name(),
                            kind: ResourceKind::Job,
                            name: None,
                            variant: InstanceVariant::Singleton,
                            display_name: record.display_name,
                        })
                        .find(|instance| {
                            crate::runtime::history::query_observations(db, instance)
                                .unwrap()
                                .is_empty()
                        })
                });
                if let Some(instance) = found {
                    break instance;
                }
                assert!(Instant::now() < until, "the check never started");
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            state.db.call(move |db| observed(db, &instance));
        }
        runner.await.unwrap()
    })
}

fn exits(code: i64) -> impl Fn(&Db, &ResourceInstance) + Send + 'static {
    move |db, instance| {
        insert_observation(db, instance, "container_running", &json!({})).unwrap();
        insert_observation(
            db,
            instance,
            "container_exited",
            &json!({ "exit_code": code }),
        )
        .unwrap();
    }
}

// spec: GATE#check
#[test]
fn a_check_passes_when_its_job_exits_zero_and_fails_otherwise() {
    let oi = TestOi::new();
    installed(&oi, CHECKED);
    assert_eq!(check_with(&oi, None, exits(0)), CheckOutcome::Passed);
    assert!(matches!(
        check_with(&oi, None, exits(3)),
        CheckOutcome::Failed(_)
    ));
}

// spec: GRUN#checks
#[test]
fn a_check_whose_job_cannot_be_recorded_could_not_be_run() {
    let oi = TestOi::new();
    installed(&oi, CHECKED);
    oi.state.db.call(|db| {
        db.conn
            .execute_batch("DROP TABLE dynamic_resources;")
            .unwrap();
    });
    let outcome = oi.block_on(run_check(
        &oi.state,
        &app_name(),
        &gate("ready"),
        probe_job(&oi),
        None,
    ));
    assert!(matches!(outcome, CheckOutcome::Unavailable(_)), "{outcome:?}");
}

// spec: GRUN#cancellation
#[test]
fn cancelling_while_a_check_runs_ends_it_at_once() {
    let oi = TestOi::new();
    installed(&oi, CHECKED);
    let cancel = Arc::new(CancelToken::new());
    cancel.request();
    let outcome = check_with(&oi, Some(cancel), |_, _| {});
    assert_eq!(outcome, CheckOutcome::Cancelled);
}

// spec: GATE#check
#[test]
fn a_check_is_judged_by_how_its_job_exited_and_by_its_deadline() {
    use crate::runtime::history::WorldObservation;
    let instance = ResourceInstance::new_singleton(app_name(), ResourceKind::Job, "probe");
    let observation = |kind: &str, payload: serde_json::Value| WorldObservation {
        id: 0,
        recorded_at: 0,
        resource: instance.clone(),
        obs_kind: kind.to_owned(),
        payload,
    };
    let exited = |code: i64| {
        [
            observation("container_running", json!({})),
            observation("container_exited", json!({ "exit_code": code })),
        ]
    };
    let deadline = Some(Duration::from_secs(10));
    assert_eq!(
        judge(&exited(0), deadline, Duration::from_secs(1)),
        Some(CheckOutcome::Passed)
    );
    assert!(matches!(
        judge(&exited(1), deadline, Duration::from_secs(1)),
        Some(CheckOutcome::Failed(_))
    ));
    assert!(matches!(
        judge(&exited(137), deadline, Duration::from_secs(11)),
        Some(CheckOutcome::Failed(why)) if why.contains("deadline")
    ));
    assert_eq!(
        judge(
            &[observation("container_running", json!({}))],
            deadline,
            Duration::from_secs(1)
        ),
        None,
        "a check that has not ended has no outcome yet"
    );
}
