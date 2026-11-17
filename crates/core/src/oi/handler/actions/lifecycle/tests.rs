use std::{collections::HashMap, sync::Arc};

use parking_lot::RwLock;
use seedling_protocol::{
    events::{OiEvent, new_event_channel},
    names::{ActionName, AppName},
};
use tokio::sync::Notify;

use super::{BoundParams, run_operation_loop};
use crate::runtime::{
    barrier::{CancelToken, OperationId, replay::ActionLog as _},
    db::DbHandle,
    faults, generations,
    secrets::Cipher,
};

const SCRIPT: &str = r#"
    app.deployment("web").image("docker.io/library/nginx:1");
    app.on_action("poke", |rt, _p| { rt.start(app); });
    app.on_action("bounce", |rt, _p| { rt.restart(app.deployment("web")); });
"#;

struct Run {
    db: DbHandle,
    events: tokio::sync::broadcast::Receiver<OiEvent>,
    ok: bool,
    operation_id: String,
}

/// Run `poke` for an app whose generation 1 is attached to the operation,
/// with `cancel` deciding whether the operation is cancelled before it starts.
fn run_poke(cancel: bool, break_persistence: bool) -> Run {
    let name = AppName::new("demo").unwrap();
    let (app, err) = crate::runtime::apps::evaluate_script(
        &name,
        SCRIPT,
        &Default::default(),
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none());
    let db = DbHandle::open_in_memory().unwrap();
    let operation_id = OperationId::new();
    {
        let (id, name) = (operation_id.0.clone(), name.clone());
        db.call(move |db| {
            let generation = generations::register_script(db, &name, SCRIPT).unwrap();
            generations::attach_operation(db, &name, generation, &id).unwrap();
            if break_persistence {
                db.conn
                    .execute_batch("DROP TABLE current_operation;")
                    .unwrap();
            }
        });
    }
    let tx = new_event_channel();
    let events = tx.subscribe();
    let op_ctx = tx.operation(
        name,
        ActionName::new("poke").unwrap(),
        operation_id.0.clone(),
        1,
        1,
        None,
    );
    let cancel_token = Arc::new(CancelToken::new());
    if cancel {
        cancel_token.request();
    }
    let ok = run_operation_loop(
        &app,
        db.clone(),
        Ok(BoundParams {
            params: serde_json::Map::new(),
            bindings: HashMap::new(),
        }),
        Arc::new(RwLock::new(None)),
        Arc::new(Notify::new()),
        &op_ctx,
        &crate::ScriptLimits::default(),
        Arc::new(Cipher::for_tests()),
        cancel_token,
        None,
        None,
        None,
    );
    Run {
        db,
        events,
        ok,
        operation_id: operation_id.0,
    }
}

fn outcome_of(db: &DbHandle) -> Option<String> {
    db.call(|db| {
        db.conn
            .query_row(
                "SELECT outcome FROM generations WHERE generation = 1",
                [],
                |r| r.get(0),
            )
            .unwrap()
    })
}

fn kinds_of(events: &mut tokio::sync::broadcast::Receiver<OiEvent>) -> Vec<String> {
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

// spec: GEN#generation-numbers
#[test]
fn a_completed_operation_records_its_outcome_against_its_generation() {
    let mut run = run_poke(false, false);
    assert!(run.ok);
    assert_eq!(outcome_of(&run.db).as_deref(), Some("succeeded"));
    assert!(kinds_of(&mut run.events).contains(&"OperationCompleted".to_owned()));
}

// spec: OPS#cancellation
#[test]
fn a_cancelled_operation_ends_cancelled_rather_than_failed() {
    let mut run = run_poke(true, false);
    assert!(!run.ok);

    let kinds = kinds_of(&mut run.events);
    assert!(
        kinds.contains(&"OperationCancelled".to_owned()),
        "{kinds:?}"
    );
    assert!(!kinds.contains(&"OperationFailed".to_owned()), "{kinds:?}");
    assert_eq!(outcome_of(&run.db).as_deref(), Some("cancelled"));
    let filed: Vec<String> = run.db.call(|db| {
        faults::list_active_faults(db, Some(&AppName::new("demo").unwrap()))
            .unwrap()
            .into_iter()
            .map(|f| f.kind)
            .collect()
    });
    assert_eq!(filed, vec!["operation_cancelled"]);
}

// spec: OPS#persistence
#[test]
fn an_operation_that_cannot_be_persisted_does_not_run_its_closure() {
    let mut run = run_poke(false, true);
    assert!(!run.ok);

    let kinds = kinds_of(&mut run.events);
    assert!(kinds.contains(&"OperationFailed".to_owned()), "{kinds:?}");
    let id = OperationId(run.operation_id.clone());
    let log = crate::runtime::barrier::replay::DbActionLog::new(
        run.db.clone(),
        id,
        AppName::new("demo").unwrap(),
        ActionName::new("poke").unwrap(),
    );
    assert!(log.load().unwrap().is_empty(), "the closure made no call");
    assert_eq!(outcome_of(&run.db).as_deref(), Some("failed"));
}

// spec: RT#deployment-restart
#[test]
fn a_replayed_closure_restarts_the_deployment_only_once() {
    let name = AppName::new("demo").unwrap();
    let (app, _) = crate::runtime::apps::evaluate_script(
        &name,
        SCRIPT,
        &Default::default(),
        &crate::ScriptLimits::default(),
    );
    let db = DbHandle::open_in_memory().unwrap();
    let operation_id = OperationId::new();
    let tx = new_event_channel();
    let run = |db: &DbHandle| {
        let op_ctx = tx.operation(
            name.clone(),
            ActionName::new("bounce").unwrap(),
            operation_id.0.clone(),
            1,
            1,
            None,
        );
        run_operation_loop(
            &app,
            db.clone(),
            Ok(BoundParams {
                params: serde_json::Map::new(),
                bindings: HashMap::new(),
            }),
            Arc::new(RwLock::new(None)),
            Arc::new(Notify::new()),
            &op_ctx,
            &crate::ScriptLimits::default(),
            Arc::new(Cipher::for_tests()),
            Arc::new(CancelToken::new()),
            None,
            None,
            None,
        )
    };
    let generation = |db: &DbHandle| {
        let name = name.clone();
        db.call(move |db| crate::runtime::restart_gens::load_restart_gen(db, &name, "web").unwrap())
    };

    assert!(run(&db));
    assert_eq!(generation(&db), 1);
    // The closure runs again from its beginning, as after a restart.
    assert!(run(&db));
    assert_eq!(generation(&db), 1, "the call is not made a second time");
}

// spec: OPS#initiating-events
#[test]
fn an_operation_runs_against_its_target_generation_not_the_current_one() {
    let name = AppName::new("demo").unwrap();
    // Generation 1's `poke` restarts the deployment; generation 2's does not.
    let gen1 = r#"
        app.deployment("web").image("docker.io/library/nginx:1");
        app.on_action("poke", |rt, _p| { rt.restart(app.deployment("web")); });
    "#;
    let gen2 = r#"
        app.deployment("web").image("docker.io/library/nginx:1");
        app.on_action("poke", |rt, _p| {});
    "#;
    let db = DbHandle::open_in_memory().unwrap();
    {
        let name = name.clone();
        db.call(move |db| {
            generations::register_script(db, &name, gen1).unwrap();
            generations::update_script(db, &name, gen2).unwrap();
            db.conn
                .execute(
                    "INSERT INTO registered_apps (name, installed, uninstalling, installing, \
                     current_generation) VALUES (?1, 1, 0, 0, 2)",
                    [&name],
                )
                .unwrap();
        });
    }
    // The app as the registry holds it: the current generation's.
    let (current, _) = crate::runtime::apps::evaluate_script(
        &name,
        gen2,
        &Default::default(),
        &crate::ScriptLimits::default(),
    );
    let tx = new_event_channel();
    let op_ctx = tx.operation(
        name.clone(),
        ActionName::new("poke").unwrap(),
        "op-1",
        1,
        1,
        None,
    );
    let ok = run_operation_loop(
        &current,
        db.clone(),
        Ok(BoundParams {
            params: serde_json::Map::new(),
            bindings: HashMap::new(),
        }),
        Arc::new(RwLock::new(None)),
        Arc::new(Notify::new()),
        &op_ctx,
        &crate::ScriptLimits::default(),
        Arc::new(Cipher::for_tests()),
        Arc::new(CancelToken::new()),
        None,
        None,
        None,
    );
    assert!(ok);
    let restarted = db
        .call(move |db| crate::runtime::restart_gens::load_restart_gen(db, &name, "web").unwrap());
    assert_eq!(
        restarted, 1,
        "generation 1's closure ran, not generation 2's"
    );
}

// spec: ACN#volume-params
#[test]
fn a_volume_param_that_cannot_be_bound_fails_the_dispatch_rather_than_being_skipped() {
    use crate::defs::install::{ParamDef, ParamKind};
    use crate::oi::test_support::TestOi;
    use seedling_protocol::names::ParamName;

    let oi = TestOi::new();
    let schema = std::collections::BTreeMap::from([(
        ParamName::new("data").unwrap(),
        ParamDef {
            kind: ParamKind::Volume,
            required: true,
            default_value: None,
            description: None,
            secret: None,
        },
    )]);
    let mut params = serde_json::Map::new();
    params.insert("data".into(), serde_json::json!("vanished"));

    let err = super::bind_volume_params(&oi.state, &schema, params, "op-1")
        .err()
        .expect("a volume that is not there cannot be bound");
    assert!(err.contains("no longer exists"), "{err}");

    // A param that was not supplied binds nothing and is not an error.
    let none = super::bind_volume_params(&oi.state, &schema, serde_json::Map::new(), "op-1")
        .expect("an absent optional volume param is not a failure");
    assert!(none.bindings.is_empty());

    // An empty one is an absent one, not a volume named "".
    let mut params = serde_json::Map::new();
    params.insert("data".into(), serde_json::json!(""));
    let empty = super::bind_volume_params(&oi.state, &schema, params, "op-1")
        .expect("an empty optional volume param is not a failure");
    assert!(empty.bindings.is_empty());
    assert!(!empty.params.contains_key("data"));
}

// spec: OPS#operation-scoped-volumes
#[test]
fn a_replayed_operation_binds_its_volumes_under_their_persisted_names() {
    use crate::oi::test_support::TestOi;
    use crate::runtime::scheduler::ScheduleResult;

    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        serde_json::json!({ "app": "demo", "script": r#"
            app.deployment("web").image("docker.io/library/nginx:1.29");
            app.on_action("fill", |rt, p| {
                rt.write(app.external_volume(p["data_volume"]), "/marker", "replayed");
            }, #{ params: #{ data: #{ kind: "volume", required: true } } });
        "# }),
    )
    .unwrap();
    oi.install("demo");

    // As persisted by the interrupted run: the param names the binding, and
    // no longer the site volume it was made from.
    let dir = tempfile::tempdir().unwrap();
    let binding = "seedling-op-interrupted-0".to_owned();
    let mut params = serde_json::Map::new();
    params.insert("data_volume".into(), serde_json::json!(binding));
    let bindings = HashMap::from([(
        binding,
        crate::defs::volume::OperationVolumeBinding {
            host_path: dir.path().to_owned(),
            read_only: false,
        },
    )]);

    let app = AppName::new("demo").unwrap();
    let action = ActionName::new("fill").unwrap();
    let operation_id = OperationId::new();
    let admitted = oi.state.scheduler.lock().request_with_id(
        &app,
        &action,
        params.clone(),
        1,
        1,
        "replay",
        operation_id.clone(),
        None,
    );
    assert!(matches!(admitted, ScheduleResult::Accepted));
    oi.block_on(async {
        super::spawn_accepted_operation(
            Arc::clone(&oi.state),
            app,
            action,
            operation_id,
            super::OperationParams::Replayed { params, bindings },
            1,
            1,
            "replay".to_owned(),
            None,
        );
        let ended = async {
            while oi.state.scheduler.lock().active().is_some() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), ended)
            .await
            .expect("the replay ends");
    });

    assert_eq!(
        std::fs::read_to_string(dir.path().join("marker"))
            .expect("the closure wrote through the binding"),
        "replayed"
    );
}

/// An operation's anonymous Job, recorded as a dynamic resource of it, whose
/// container has run and been removed on exit.
fn recorded_dynamic_job(
    oi: &crate::oi::test_support::TestOi,
    operation_id: &str,
) -> crate::runtime::identity::ResourceInstance {
    let job = crate::runtime::identity::ResourceInstance::new_anonymous(
        AppName::new("app").unwrap(),
        crate::defs::resource::ResourceKind::Job,
    );
    let (recorded, operation_id) = (job.clone(), operation_id.to_owned());
    oi.state.db.call(move |db| {
        crate::runtime::desired::insert_dynamic_resource(db, &recorded, &operation_id, None)
            .unwrap();
        for kind in ["container_running", "container_exited", "container_removed"] {
            crate::runtime::history::insert_observation(
                db,
                &recorded,
                kind,
                &serde_json::json!({}),
            )
            .unwrap();
        }
    });
    job
}

fn dynamic_records(oi: &crate::oi::test_support::TestOi, operation_id: &str) -> usize {
    let operation_id = operation_id.to_owned();
    oi.state.db.call(move |db| {
        crate::runtime::desired::list_dynamic_resources_for_operation(db, &operation_id)
            .unwrap()
            .len()
    })
}

fn rename_table(oi: &crate::oi::test_support::TestOi, from: &'static str, to: &'static str) {
    oi.state.db.call(move |db| {
        db.conn
            .execute(&format!("ALTER TABLE {from} RENAME TO {to}"), [])
            .unwrap();
    });
}

/// Run `body` on a runtime whose clock moves only when every task is idle,
/// so the cleanup's waits pass at once.
fn with_paused_clock<F: std::future::Future>(body: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(body)
}

const TEN_MINUTES: std::time::Duration = std::time::Duration::from_secs(600);

// spec: APP#dynamic-resource-lifetime
// The records were read with `unwrap_or_default`, so a read that failed was
// nothing to clean up: the cleanup ended at once and the resources stayed
// with nothing tearing them down.
#[test]
fn a_dynamic_resource_list_that_cannot_be_read_is_not_nothing_to_clean_up() {
    let oi = crate::oi::test_support::TestOi::new();
    recorded_dynamic_job(&oi, "op-1");
    rename_table(&oi, "dynamic_resources", "hidden_dynamic_resources");
    let progress = RwLock::new(None);
    let notify = Notify::new();

    with_paused_clock(async {
        let cleanup = super::cleanup_dynamic_resources(&oi.state, "op-1", &progress, &notify);
        tokio::pin!(cleanup);
        tokio::select! {
            () = &mut cleanup => panic!("the cleanup ended without reading its records"),
            () = tokio::time::sleep(TEN_MINUTES) => {}
        }

        rename_table(&oi, "hidden_dynamic_resources", "dynamic_resources");
        tokio::time::timeout(TEN_MINUTES, cleanup)
            .await
            .expect("the cleanup ends once its records are read and torn down");
    });
    assert_eq!(dynamic_records(&oi, "op-1"), 0);
}

// spec: APP#dynamic-resource-lifetime
// The cleanup gave up after a minute and forgot the records of resources it
// had not seen torn down.
#[test]
fn the_cleanup_does_not_give_up_on_a_resource_that_is_slow_to_go() {
    let oi = crate::oi::test_support::TestOi::new();
    let job = recorded_dynamic_job(&oi, "op-1");
    let network = format!("seedling-{}", job.display_name);
    oi.block_on(oi.state.driver.container.create_network(
        &network,
        "fd00:0:0:1::/64".parse().unwrap(),
        None,
    ))
    .unwrap();
    let progress = RwLock::new(None);
    let notify = Notify::new();

    with_paused_clock(async {
        let cleanup = super::cleanup_dynamic_resources(&oi.state, "op-1", &progress, &notify);
        tokio::pin!(cleanup);
        tokio::select! {
            () = &mut cleanup => panic!("the cleanup gave up while the network stood"),
            () = tokio::time::sleep(TEN_MINUTES) => {}
        }
        assert_eq!(dynamic_records(&oi, "op-1"), 1, "the record is kept");
        assert!(
            progress
                .read()
                .as_ref()
                .is_some_and(|p| p.dynamic_defs.contains_key(&job)),
            "the job is still being torn down"
        );

        oi.state
            .driver
            .container
            .remove_network(&network)
            .await
            .unwrap();
        tokio::time::timeout(TEN_MINUTES, cleanup)
            .await
            .expect("the cleanup ends once the network is gone");
    });
    assert_eq!(dynamic_records(&oi, "op-1"), 0);
}
