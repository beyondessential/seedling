use std::{collections::HashMap, sync::Arc};

use parking_lot::RwLock;
use seedling_protocol::{
    events::{OiEvent, new_event_channel},
    names::{ActionName, AppName},
};
use tokio::sync::Notify;

use super::run_operation_loop;
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
        serde_json::Map::new(),
        Arc::new(RwLock::new(None)),
        Arc::new(Notify::new()),
        &op_ctx,
        &crate::ScriptLimits::default(),
        Arc::new(Cipher::for_tests()),
        HashMap::new(),
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
            serde_json::Map::new(),
            Arc::new(RwLock::new(None)),
            Arc::new(Notify::new()),
            &op_ctx,
            &crate::ScriptLimits::default(),
            Arc::new(Cipher::for_tests()),
            HashMap::new(),
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
        serde_json::Map::new(),
        Arc::new(RwLock::new(None)),
        Arc::new(Notify::new()),
        &op_ctx,
        &crate::ScriptLimits::default(),
        Arc::new(Cipher::for_tests()),
        HashMap::new(),
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
