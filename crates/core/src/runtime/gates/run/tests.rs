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

fn gate_input(oi: &TestOi, name: &str, cancelled: bool) -> GateRunInput {
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
    GateRunInput {
        app,
        gate: gate(name),
        op_ctx,
        active_progress,
        tick_notify,
        cancel_token,
        db_path: oi.state.db_path.clone(),
    }
}

fn kinds_of(
    events: &mut tokio::sync::broadcast::Receiver<seedling_protocol::events::OiEvent>,
) -> Vec<String> {
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

/// Run `name` once, as the dispatcher does, and report the kinds of the
/// events it emitted.
fn run_once(oi: &TestOi, name: &str, cancelled: bool) -> Vec<String> {
    run_at(oi, name, cancelled, 1)
}

/// As [`run_once`], for a run against `generation`.
fn run_at(oi: &TestOi, name: &str, cancelled: bool, generation: u64) -> Vec<String> {
    let mut events = oi.state.event_tx.subscribe();
    let mut input = gate_input(oi, name, cancelled);
    input.op_ctx.operation_generation = generation;
    oi.block_on(run_gate(&oi.state, input));
    kinds_of(&mut events)
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
    assert!(
        kinds.contains(&"OperationCompleted".to_owned()),
        "{kinds:?}"
    );

    let record = record(&oi, "migrate");
    assert!(record.valid);
    assert_eq!(record.key_digest, Some(key(&oi, "migrate")));
    assert_eq!(record.success_generation, Some(1));
    assert_eq!(record.consecutive_failures, 0);
    assert_eq!(
        holds(&oi),
        0,
        "the hold is cleared once the gate is satisfied"
    );
    assert_eq!(
        persisted_operations(&oi),
        0,
        "the operation record is cleared"
    );
    assert!(faults_of(&oi).is_empty());
}

// spec: FLT#app-faults
#[test]
fn a_run_that_succeeds_clears_the_faults_of_operations_that_failed_or_were_cancelled_before_it() {
    let oi = TestOi::new();
    installed(&oi, SCRIPT);
    oi.state.db.call(|db| {
        for kind in ["operation_failed", "operation_cancelled"] {
            faults::file_fault(db, &app_name(), None, None, None, kind, "an earlier action")
                .unwrap();
        }
    });
    assert_eq!(faults_of(&oi), ["operation_cancelled", "operation_failed"]);

    let kinds = run_once(&oi, "migrate", false);
    assert!(
        kinds.contains(&"OperationCompleted".to_owned()),
        "{kinds:?}"
    );
    assert!(faults_of(&oi).is_empty(), "{:?}", faults_of(&oi));
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
    assert!(
        !record.valid,
        "the gate is unsatisfied until a run succeeds"
    );
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
    assert_eq!(
        holds(&oi),
        0,
        "the selection was never held, so nothing was stopped"
    );
    assert!(
        record.paused_key_digest.is_none(),
        "a failed run does not pause the gate"
    );
    assert_eq!(faults_of(&oi), vec!["gate_failed"]);
    assert_eq!(persisted_operations(&oi), 0);
}

// spec: GRUN#preparation
// spec: OPS#interrupted-operations
#[test]
fn a_replayed_run_whose_body_has_begun_is_not_prepared_for_again() {
    use crate::runtime::{
        barrier::{ActionLogEntry, CallKind, OperationId},
        history::insert_action_log_entry,
    };

    let oi = TestOi::with_unreadable_containers();
    installed(&oi, SCRIPT);
    oi.state.db.call(|db| {
        insert_action_log_entry(
            db,
            &OperationId("op-1".to_owned()),
            &app_name(),
            "migrate",
            &ActionLogEntry {
                call_index: 0,
                call_kind: CallKind::Query,
                resources: Vec::new(),
                barriers: Vec::new(),
                extra: None,
                begun: false,
            },
        )
        .unwrap();
    });

    let kinds = run_once(&oi, "migrate", false);
    assert!(
        kinds.contains(&"OperationCompleted".to_owned()),
        "a pull that would fail is not attempted again for a body that has begun: {kinds:?}"
    );
    assert!(record(&oi, "migrate").valid);
    assert_eq!(persisted_operations(&oi), 0);
}

// spec: GRUN#retries
#[test]
fn a_run_that_fails_before_it_has_its_generations_key_is_recorded_against_the_current_key() {
    let oi = TestOi::new();
    installed(&oi, SCRIPT);
    oi.state.db.call(|db| {
        db.conn
            .execute(
                "UPDATE registered_apps SET current_generation = 5 WHERE name = 'demo'",
                [],
            )
            .unwrap();
    });

    let kinds = run_at(&oi, "migrate", false, 9999);
    assert!(kinds.contains(&"OperationFailed".to_owned()), "{kinds:?}");

    let record = record(&oi, "migrate");
    assert_eq!(record.consecutive_failures, 1);
    assert_eq!(
        record.failed_key_digest,
        Some(key(&oi, "migrate")),
        "the failure is on record, so the retry waits out its delay"
    );
    assert!(
        record
            .last_failure
            .as_deref()
            .is_some_and(|how| how.contains("cannot be rebuilt")),
        "{:?}",
        record.last_failure
    );
    assert_eq!(faults_of(&oi), vec!["gate_failed"]);
    assert_eq!(holds(&oi), 0, "nothing was stopped");
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
    assert!(
        kinds.contains(&"OperationCancelled".to_owned()),
        "{kinds:?}"
    );
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
    assert_eq!(
        holds(&oi),
        1,
        "an unsatisfied gate keeps holding its selection"
    );
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

const IMAGES: &str = r#"
    let probe = app.job("probe").image("docker.io/library/tools:1").deadline(60);
    let api = app.deployment("api").image("docker.io/library/api:1");
    app.deployment("other").image("docker.io/library/other:1");
    let migrate = app.gate("migrate").key(1).check(probe).exclusive(api).run(|rt, last| {
        rt.warm_images(app.job().image("docker.io/library/body:1")).ready();
    });
    app.deployment("web").image("docker.io/library/web:1").after(migrate);
"#;

// spec: GRUN#preparation
#[test]
fn a_run_pulls_the_images_of_its_body_its_check_and_its_dependants_and_no_others() {
    let oi = TestOi::new();
    installed(&oi, IMAGES);
    let input = gate_input(&oi, "migrate", false);
    let def = input.app.def.load_full();
    let run = Run {
        state: &oi.state,
        app_name: app_name(),
        gate: gate("migrate"),
        op_ctx: input.op_ctx.clone(),
        active_progress: Arc::clone(&input.active_progress),
        tick_notify: Arc::clone(&input.tick_notify),
        cancel: Arc::clone(&input.cancel_token),
        db_path: input.db_path.clone(),
    };
    let images = oi
        .block_on(run.images(&input.app, &def, &def.gates["migrate"], None))
        .unwrap_or_else(|_| panic!("the images are worked out"));
    assert_eq!(
        images.into_iter().collect::<Vec<_>>(),
        [
            "docker.io/library/body:1",
            "docker.io/library/tools:1",
            "docker.io/library/web:1"
        ]
    );
}

// spec: GRUN#preparation
#[test]
fn an_instance_that_has_started_and_not_ended_is_still_running_and_one_that_never_started_is_not() {
    use crate::runtime::history::{insert_instance, insert_observation};
    let oi = TestOi::new();
    let instance = ResourceInstance::new_singleton(app_name(), ResourceKind::Deployment, "api");
    let selection = BTreeSet::from([ResourceId {
        kind: ResourceKind::Deployment,
        name: Arc::new("api".to_owned()),
    }]);
    let running = |oi: &TestOi| {
        let selection = selection.clone();
        oi.state
            .db
            .call(move |db| still_running(db, &app_name(), &selection).unwrap())
    };
    let i = instance.clone();
    oi.state.db.call(move |db| insert_instance(db, &i).unwrap());
    assert_eq!(
        running(&oi),
        None,
        "an instance that never started is as stopped as one that ended"
    );

    let i = instance.clone();
    oi.state.db.call(move |db| {
        insert_observation(db, &i, "container_running", &json!({})).unwrap();
    });
    assert!(
        running(&oi).is_some(),
        "a started instance holds up the run"
    );

    oi.state.db.call(move |db| {
        insert_observation(
            db,
            &instance,
            "container_exited",
            &json!({ "exit_code": 0 }),
        )
        .unwrap();
    });
    assert_eq!(running(&oi), None, "an instance that ended no longer does");
}

// spec: GRUN#preparation
#[test]
fn an_instance_with_a_start_that_has_not_ended_is_still_running_until_the_start_fails() {
    use crate::runtime::history::{
        Provenance, complete_autonomous_operation, insert_autonomous_operation, insert_instance,
    };
    let oi = TestOi::new();
    let instance = ResourceInstance::new_singleton(app_name(), ResourceKind::Deployment, "api");
    let selection = BTreeSet::from([ResourceId {
        kind: ResourceKind::Deployment,
        name: Arc::new("api".to_owned()),
    }]);
    let running = |oi: &TestOi| {
        let selection = selection.clone();
        oi.state
            .db
            .call(move |db| still_running(db, &app_name(), &selection).unwrap())
    };
    let i = instance.clone();
    oi.state.db.call(move |db| insert_instance(db, &i).unwrap());
    let i = instance.clone();
    let start = oi.state.db.call(move |db| {
        insert_autonomous_operation(
            db,
            i.id,
            "start",
            &Provenance {
                observation_ids: Vec::new(),
                rule: "test".to_owned(),
            },
        )
        .unwrap()
    });
    assert!(
        running(&oi).is_some(),
        "an instance whose start is under way is not yet stopped"
    );

    oi.state.db.call(move |db| {
        complete_autonomous_operation(db, start, "error: image_unavailable").unwrap();
    });
    assert_eq!(
        running(&oi),
        None,
        "a start that failed left nothing to wait for"
    );
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
    let def = oi
        .state
        .registry
        .read()
        .get("demo")
        .unwrap()
        .app
        .def
        .load_full();
    let (_, job) = def
        .resources
        .iter()
        .find(|(id, _)| id.kind == ResourceKind::Job)
        .unwrap();
    job.clone()
}

/// Wait for the check's instance to be started, and report the host's
/// observations of it as `observed` says.
async fn feed_check(
    state: &Arc<OiState>,
    observed: impl Fn(&Db, &ResourceInstance) + Send + 'static,
) {
    let until = Instant::now() + Duration::from_secs(60);
    let instance = loop {
        // The check that is waiting is the one whose instance has not been
        // observed yet.
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
            feed_check(&state, observed).await;
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
fn a_running_check_is_not_a_shell_session() {
    let oi = TestOi::new();
    installed(&oi, CHECKED);
    let job = probe_job(&oi);
    let state = Arc::clone(&oi.state);
    oi.block_on(async move {
        let runner = tokio::spawn({
            let state = Arc::clone(&state);
            async move { run_check(&state, &app_name(), &gate("ready"), job, None).await }
        });
        let until = Instant::now() + Duration::from_secs(60);
        let record = loop {
            let found = state.db.call(|db| {
                list_dynamic_resources_for_app(db, &app_name())
                    .unwrap()
                    .pop()
            });
            if let Some(record) = found {
                break record;
            }
            assert!(Instant::now() < until, "the check never recorded its Job");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(record.creator, DynamicCreator::GateCheck);
        {
            let registry = state.registry.read();
            let entry = registry.get("demo").unwrap();
            assert!(
                entry.sessions.is_empty(),
                "a check opened a shell session slot"
            );
            let checks: Vec<String> = entry
                .checks
                .owners()
                .iter()
                .map(ToString::to_string)
                .collect();
            assert_eq!(checks, std::slice::from_ref(&record.operation_id));
            assert!(registry.live_slot_owners().contains(&record.operation_id));
        }
        feed_check(&state, exits(0)).await;
        assert_eq!(runner.await.unwrap(), CheckOutcome::Passed);
    });
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
    assert!(
        matches!(outcome, CheckOutcome::Unavailable(_)),
        "{outcome:?}"
    );
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

/// Run `ready`, the gate of `CHECKED`, with the host observing its check's
/// instance as `observed` says.
fn run_ready(
    oi: &TestOi,
    observed: impl Fn(&Db, &ResourceInstance) + Send + 'static,
) -> Vec<String> {
    let mut events = oi.state.event_tx.subscribe();
    let state = Arc::clone(&oi.state);
    let input = gate_input(oi, "ready", false);
    oi.block_on(async move {
        let feeder = tokio::spawn({
            let state = Arc::clone(&state);
            async move { feed_check(&state, observed).await }
        });
        run_gate(&state, input).await;
        feeder.await.unwrap();
    });
    kinds_of(&mut events)
}

// spec: GRUN#outcome
#[test]
fn a_run_succeeds_only_once_its_check_passes() {
    let oi = TestOi::new();
    installed(&oi, CHECKED);

    let kinds = run_ready(&oi, exits(0));
    assert!(
        kinds.contains(&"OperationCompleted".to_owned()),
        "{kinds:?}"
    );
    assert!(record(&oi, "ready").valid);

    let oi = TestOi::new();
    installed(&oi, CHECKED);
    let kinds = run_ready(&oi, exits(1));
    assert!(kinds.contains(&"OperationFailed".to_owned()), "{kinds:?}");
    let after = record(&oi, "ready");
    assert!(
        !after.valid,
        "a run whose check fails does not satisfy the gate"
    );
    assert!(
        after
            .last_failure
            .as_deref()
            .is_some_and(|how| how.contains("its check failed")),
        "{:?}",
        after.last_failure
    );
    assert_eq!(faults_of(&oi), vec!["gate_failed"]);
}

// spec: GRUN#outcome
#[test]
fn a_run_whose_check_cannot_be_run_does_not_succeed_and_files_gate_check_unavailable() {
    let oi = TestOi::new();
    installed(&oi, CHECKED);
    oi.state.db.call(|db| {
        db.conn
            .execute_batch("DROP TABLE dynamic_resources;")
            .unwrap();
    });

    let kinds = run_once(&oi, "ready", false);
    assert!(kinds.contains(&"OperationFailed".to_owned()), "{kinds:?}");
    let after = record(&oi, "ready");
    assert!(!after.valid);
    assert_eq!(after.check_failures, 1);
    assert_eq!(
        faults_of(&oi),
        vec!["gate_check_unavailable", "gate_failed"]
    );
}

// spec: GRUN#retries
#[test]
fn a_run_gives_up_once_its_exclusive_selection_has_been_unreadable_for_long_enough() {
    let since = Instant::now();
    assert!(!selection_unreadable_for_too_long(since, since));
    assert!(!selection_unreadable_for_too_long(
        since,
        since + SELECTION_READ_WAIT - Duration::from_secs(1)
    ));
    assert!(selection_unreadable_for_too_long(
        since,
        since + SELECTION_READ_WAIT
    ));
}

// spec: GRUN#preparation
// spec: RCN#gate-holds
#[test]
fn a_run_does_not_wait_for_a_resource_a_change_removed_from_its_hold() {
    use crate::runtime::history::{insert_instance, insert_observation};
    let oi = TestOi::new();
    installed(&oi, SCRIPT);
    let api = ResourceInstance::new_singleton(app_name(), ResourceKind::Deployment, "api");
    oi.state.db.call(move |db| {
        insert_instance(db, &api).unwrap();
        insert_observation(db, &api, "container_running", &json!({})).unwrap();
    });

    let input = gate_input(&oi, "migrate", false);
    let state = Arc::clone(&oi.state);
    let ended = oi.block_on(async move {
        let run = tokio::spawn({
            let state = Arc::clone(&state);
            async move { run_gate(&state, input).await }
        });
        let until = Instant::now() + Duration::from_secs(60);
        while state
            .db
            .call(|db| store::load_holds(db, &app_name()).unwrap())
            .is_empty()
        {
            assert!(Instant::now() < until, "the run never held its selection");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!run.is_finished(), "the run waits for api to stop");
        state.db.call(|db| {
            store::set_hold(db, &app_name(), &gate("migrate"), &BTreeSet::new()).unwrap();
        });
        tokio::time::timeout(Duration::from_secs(10), run).await
    });
    assert!(ended.is_ok(), "the run went on waiting for a released api");
    assert!(record(&oi, "migrate").valid);
}
