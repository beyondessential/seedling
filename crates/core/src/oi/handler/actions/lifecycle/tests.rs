use std::{collections::HashMap, sync::Arc};

use parking_lot::RwLock;
use seedling_protocol::{
    events::{OiEvent, OperationTarget, new_event_channel},
    names::{ActionName, AppName},
};
use tokio::sync::Notify;

use super::{BoundParams, OperationEnd, Persisted, end_action};
use crate::runtime::{
    barrier::{CancelToken, OperationId, replay::ActionLog as _},
    db::DbHandle,
    faults, generations,
    secrets::Cipher,
};

/// Run an action's closure to its end and report it as the dispatcher does,
/// saying whether it completed.
#[expect(
    clippy::too_many_arguments,
    reason = "the arguments are those of the loop it runs"
)]
fn run_operation_loop(
    app: &crate::defs::app::App,
    db: DbHandle,
    bound: Result<BoundParams, String>,
    active_progress: Arc<RwLock<Option<crate::runtime::OperationProgress>>>,
    tick_notify: Arc<Notify>,
    op_ctx: &seedling_protocol::events::OperationEventCtx,
    limits: &crate::ScriptLimits,
    cipher: Arc<Cipher>,
    cancel_token: Arc<CancelToken>,
    signaler: Option<Arc<dyn crate::runtime::barrier::ContainerSignaler>>,
    writer: Option<Arc<dyn crate::runtime::barrier::VolumeWriter>>,
    executor: Option<Arc<dyn crate::runtime::barrier::Executor>>,
) -> bool {
    let end = super::run_operation_loop(
        app,
        db.clone(),
        bound,
        active_progress,
        tick_notify,
        op_ctx,
        limits,
        cipher,
        &crate::runtime::gates::GateRuntime::new(),
        &RwLock::new(crate::runtime::apps::AppRegistry::new()),
        cancel_token,
        signaler,
        writer,
        executor,
        None,
        Persisted::No,
    );
    let completed = matches!(end, OperationEnd::Completed);
    end_action(&db, op_ctx, end);
    completed
}

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

/// Run `poke` for an app at generation 1, with `cancel` deciding whether the operation is cancelled before it starts.
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
        let name = name.clone();
        db.call(move |db| {
            generations::register_script(db, &name, SCRIPT).unwrap();
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
        ActionName::new("poke").unwrap().into(),
        operation_id.0.clone(),
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

// spec: OPS#completion
#[test]
fn a_completed_operation_ends_completed() {
    let mut run = run_poke(false, false);
    assert!(run.ok);
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
        ActionName::new("poke").unwrap().into(),
    );
    assert!(log.load().unwrap().is_empty(), "the closure made no call");
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
            ActionName::new("bounce").unwrap().into(),
            operation_id.0.clone(),
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
fn an_operation_runs_against_its_generation_not_the_current_one() {
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
                    "INSERT INTO registered_apps (name, installed, uninstalling, \
                     current_generation) VALUES (?1, 1, 0, 2)",
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
        ActionName::new("poke").unwrap().into(),
        "op-1",
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
    use crate::defs::param_def::{ParamDef, ParamKind};
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
    let action = OperationTarget::Action(ActionName::new("fill").unwrap());
    let operation_id = OperationId::new();
    let admitted = oi.state.scheduler.lock().request_with_id(
        &app,
        &action,
        params.clone(),
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
        crate::runtime::desired::insert_dynamic_resource(
            db,
            &recorded,
            &operation_id,
            crate::runtime::desired::DynamicCreator::Operation,
            None,
        )
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

fn app() -> AppName {
    AppName::new("app").unwrap()
}

/// How many `operation_cleanup_stalled` faults are active for `operation_id`.
fn cleanup_stalled(oi: &crate::oi::test_support::TestOi, operation_id: &str) -> usize {
    let operation_id = operation_id.to_owned();
    oi.state.db.call(move |db| {
        faults::list_active_faults(db, Some(&app()))
            .unwrap()
            .into_iter()
            .filter(|f| f.kind == faults::OPERATION_CLEANUP_STALLED && f.subject == operation_id)
            .count()
    })
}

// spec: APP#dynamic-resource-lifetime
// spec: OPS#completion
// spec: FLT#app-faults
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

    let app = app();
    with_paused_clock(async {
        let cleanup = crate::runtime::dynamic_cleanup::cleanup_dynamic_resources(
            &oi.state.db,
            &oi.state.driver,
            &app,
            "op-1",
            &progress,
            &notify,
        );
        tokio::pin!(cleanup);
        tokio::select! {
            () = &mut cleanup => panic!("the cleanup ended without reading its records"),
            () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
        assert_eq!(
            cleanup_stalled(&oi, "op-1"),
            1,
            "held while the records cannot be read, a minute or not"
        );
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
    assert_eq!(cleanup_stalled(&oi, "op-1"), 0, "cleared once torn down");
}

// spec: APP#dynamic-resource-lifetime
// spec: OPS#completion
// spec: FLT#app-faults
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

    let app = app();
    with_paused_clock(async {
        let cleanup = crate::runtime::dynamic_cleanup::cleanup_dynamic_resources(
            &oi.state.db,
            &oi.state.driver,
            &app,
            "op-1",
            &progress,
            &notify,
        );
        tokio::pin!(cleanup);
        tokio::select! {
            () = &mut cleanup => panic!("the cleanup gave up while the network stood"),
            () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
        }
        assert_eq!(
            cleanup_stalled(&oi, "op-1"),
            0,
            "not within the first minute"
        );
        tokio::select! {
            () = &mut cleanup => panic!("the cleanup gave up while the network stood"),
            () = tokio::time::sleep(TEN_MINUTES) => {}
        }
        assert_eq!(cleanup_stalled(&oi, "op-1"), 1, "held past a minute");
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
    assert_eq!(cleanup_stalled(&oi, "op-1"), 0, "cleared once torn down");
}

// spec: APP#dynamic-resource-lifetime
// spec: SUSP#abandoned-replay
// A record that could not be written was logged and the start went ahead, so
// after a restart nothing knew the resource existed.
#[test]
fn rt_start_fails_when_the_dynamic_resource_cannot_be_recorded() {
    const SCRIPT: &str = r#"
        app.deployment("web").image("docker.io/library/nginx:1");
        app.on_action("work", |rt, _p| {
            rt.start(app.job().image("docker.io/library/busybox:1.37"));
        });
    "#;
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
        let name = name.clone();
        db.call(move |db| {
            generations::register_script(db, &name, SCRIPT).unwrap();
            db.conn
                .execute_batch("DROP TABLE dynamic_resources;")
                .unwrap();
        });
    }
    let tx = new_event_channel();
    let op_ctx = tx.operation(
        name,
        ActionName::new("work").unwrap().into(),
        operation_id.0.clone(),
        1,
        None,
    );
    let progress = Arc::new(RwLock::new(None));
    let ok = run_operation_loop(
        &app,
        db,
        Ok(BoundParams {
            params: serde_json::Map::new(),
            bindings: HashMap::new(),
        }),
        Arc::clone(&progress),
        Arc::new(Notify::new()),
        &op_ctx,
        &crate::ScriptLimits::default(),
        Arc::new(Cipher::for_tests()),
        Arc::new(CancelToken::new()),
        None,
        None,
        None,
    );

    assert!(!ok, "the operation fails");
    let progress = progress.read();
    assert!(
        progress
            .as_ref()
            .is_none_or(|p| p.dynamic_defs.is_empty() && p.is_empty()),
        "nothing was placed in the desired state"
    );
}

// ---------------------------------------------------------------------------
// Gate bodies
// ---------------------------------------------------------------------------

use super::GateInput;
use crate::defs::app::App;
use seedling_protocol::names::GateName;

fn evaluated(script: &str) -> App {
    let (app, err) = crate::runtime::apps::evaluate_script(
        &AppName::new("demo").unwrap(),
        script,
        &Default::default(),
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none(), "the script evaluates: {err:?}");
    app
}

/// Run the gate `name` of `app` as the dispatcher does, with `last` as the
/// runtime built it, against `db`.
fn run_gate_body(
    app: &App,
    script: &str,
    name: &str,
    last: Option<&App>,
    db: &DbHandle,
    operation_id: &str,
) -> OperationEnd {
    {
        let (n, script) = (AppName::new("demo").unwrap(), script.to_owned());
        db.call(move |db| {
            let _ = generations::register_script(db, &n, &script);
        });
    }
    let tx = new_event_channel();
    let op_ctx = tx.operation(
        AppName::new("demo").unwrap(),
        OperationTarget::Gate(GateName::new(name).unwrap()),
        operation_id,
        1,
        None,
    );
    super::run_operation_loop(
        app,
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
        &crate::runtime::gates::GateRuntime::new(),
        &RwLock::new(crate::runtime::apps::AppRegistry::new()),
        Arc::new(CancelToken::new()),
        None,
        None,
        None,
        Some(GateInput { last }),
        Persisted::No,
    )
}

fn failure_of(end: OperationEnd) -> String {
    match end {
        OperationEnd::Failed(why) | OperationEnd::NotRun(why) => why,
        OperationEnd::Completed => panic!("the body completed"),
        OperationEnd::Cancelled => panic!("the body was cancelled"),
    }
}

// spec: GATE#body
#[test]
fn a_gate_body_is_called_with_as_many_arguments_as_it_declares() {
    for (declared, body) in [
        ("no parameters", "|| { }"),
        (
            "a runtime handle",
            "|rt| { rt.restart(app.deployment(\"web\")); }",
        ),
        (
            "a runtime handle and last",
            "|rt, last| { if last != () { throw \"last is not the unit value\"; } }",
        ),
    ] {
        let script = format!(
            r#"
            app.deployment("web").image("docker.io/library/nginx:1");
            app.gate("gated").key(1).run({body});
            "#
        );
        let app = evaluated(&script);
        let db = DbHandle::open_in_memory().unwrap();
        let end = run_gate_body(&app, &script, "gated", None, &db, "op");
        assert!(
            matches!(end, OperationEnd::Completed),
            "a body declaring {declared} runs: {}",
            match end {
                OperationEnd::Failed(why) | OperationEnd::NotRun(why) => why,
                _ => String::new(),
            }
        );
    }
}

// spec: GATE#body
#[test]
fn a_gate_body_declaring_more_than_two_parameters_fails_when_called() {
    let script = r#"
        app.deployment("web").image("docker.io/library/nginx:1");
        app.gate("gated").key(1).run(|rt, last, extra| { });
    "#;
    let app = evaluated(script);
    let db = DbHandle::open_in_memory().unwrap();
    let why = failure_of(run_gate_body(&app, script, "gated", None, &db, "op"));
    assert!(why.contains("at most two"), "{why}");
}

// spec: STD#the-std-module
#[test]
fn a_gate_body_reaches_the_std_module() {
    let script = r#"
        app.deployment("web").image("docker.io/library/nginx:1");
        app.gate("gated").key(1).run(|rt| { std::nope(); });
    "#;
    let app = evaluated(script);
    let db = DbHandle::open_in_memory().unwrap();
    let why = failure_of(run_gate_body(&app, script, "gated", None, &db, "op"));
    assert!(why.contains("Function not found: std::nope"), "{why}");
}

// spec: GATE#body
#[test]
fn last_is_the_app_at_the_last_success_and_the_unit_value_where_there_is_none() {
    let script = r#"
        app.deployment("web").image("docker.io/library/nginx:1");
        app.gate("gated").key(1).run(|rt, last| {
            // Only the earlier generation has this deployment.
            last.deployment("old");
        });
    "#;
    let earlier = r#"
        app.deployment("web").image("docker.io/library/nginx:1");
        app.deployment("old").image("docker.io/library/nginx:1");
    "#;
    let current = evaluated(script);
    let db = DbHandle::open_in_memory().unwrap();

    let last = evaluated(earlier);
    assert!(
        matches!(
            run_gate_body(&current, script, "gated", Some(&last), &db, "op-last"),
            OperationEnd::Completed
        ),
        "`last` shows the app as it was at the last success"
    );
    failure_of(run_gate_body(
        &current, script, "gated", None, &db, "op-none",
    ));
    failure_of(run_gate_body(
        &current,
        script,
        "gated",
        Some(&current),
        &db,
        "op-current",
    ));
}

// spec: BND#metadata-table
#[test]
fn last_metadata_reads_the_bundle_of_the_last_success() {
    let script = r#"
        app.deployment("web").image("docker.io/library/nginx:1");
        app.gate("gated").key(1).run(|rt, last| {
            if app.metadata().version != "2.0" { throw "app reads the current bundle"; }
            if last.metadata().version != "1.0" { throw "last reads its own bundle"; }
        });
    "#;
    let evaluated_with = |version: &str| {
        let bundle = Arc::new(
            crate::runtime::definition::Bundle::from_files([
                ("app.seed.rhai".to_owned(), script.as_bytes().to_vec()),
                (
                    "seedling.toml".to_owned(),
                    format!("[metadata]\nversion = \"{version}\"").into_bytes(),
                ),
            ])
            .unwrap(),
        );
        let (app, err) = crate::runtime::apps::evaluate(
            &AppName::new("demo").unwrap(),
            &bundle,
            &Default::default(),
            &crate::runtime::seeds::AppSeed::for_tests(),
            &crate::ScriptLimits::default(),
        );
        assert!(err.is_none(), "the script evaluates: {err:?}");
        app
    };
    let (earlier, current) = (evaluated_with("1.0"), evaluated_with("2.0"));
    let db = DbHandle::open_in_memory().unwrap();
    let end = run_gate_body(&current, script, "gated", Some(&earlier), &db, "op");
    assert!(
        matches!(end, OperationEnd::Completed),
        "{}",
        match end {
            OperationEnd::Failed(why) | OperationEnd::NotRun(why) => why,
            _ => String::new(),
        }
    );
}

// spec: RT#resources-a-gate-holds
#[test]
fn rt_start_leaves_out_what_a_gate_holds_and_what_waits_on_it_except_in_that_gates_body() {
    use crate::runtime::barrier::replay::DbActionLog;

    let script = r#"
        let api = app.deployment("api").image("docker.io/library/nginx:1");
        let migrate = app.gate("migrate").key(1).exclusive(api).run(|rt| { rt.start(api); });
        app.deployment("web").image("docker.io/library/nginx:1").after(migrate);
        app.deployment("plain").image("docker.io/library/nginx:1");
        app.on_action("poke", |rt, _p| { rt.start(app); });
    "#;
    let name = AppName::new("demo").unwrap();
    let app = evaluated(script);
    let started = |target: OperationTarget, db: &DbHandle, id: &str| -> Vec<String> {
        let log = DbActionLog::new(db.clone(), OperationId(id.to_owned()), name.clone(), target);
        let mut names: Vec<_> = log
            .load()
            .unwrap()
            .into_iter()
            .flat_map(|entry| entry.resources)
            .filter_map(|resource| resource.name)
            .collect();
        names.sort();
        names
    };

    // An action's `rt.start(app)` leaves out the held selection and what waits
    // on the gate, and starts the rest.
    let db = DbHandle::open_in_memory().unwrap();
    {
        let name = name.clone();
        let script = script.to_owned();
        db.call(move |db| {
            generations::register_script(db, &name, &script).unwrap();
            let api = crate::defs::resource::ResourceId {
                kind: crate::defs::resource::ResourceKind::Deployment,
                name: Arc::new("api".to_owned()),
            };
            crate::runtime::gates::store::set_hold(
                db,
                &name,
                &GateName::new("migrate").unwrap(),
                &[api].into(),
            )
            .unwrap();
        });
    }
    let tx = new_event_channel();
    let op_ctx = tx.operation(
        name.clone(),
        ActionName::new("poke").unwrap().into(),
        "poke-op",
        1,
        None,
    );
    let end = super::run_operation_loop(
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
        &crate::runtime::gates::GateRuntime::new(),
        &RwLock::new(crate::runtime::apps::AppRegistry::new()),
        Arc::new(CancelToken::new()),
        None,
        None,
        None,
        None,
        Persisted::No,
    );
    assert!(matches!(end, OperationEnd::Completed));
    assert_eq!(
        started(ActionName::new("poke").unwrap().into(), &db, "poke-op"),
        vec!["plain"],
        "api is held and web waits on the unsatisfied gate"
    );

    // The gate's own body is not kept from its hold.
    let end = run_gate_body(&app, script, "migrate", None, &db, "gate-op");
    assert!(matches!(end, OperationEnd::Completed));
    assert_eq!(
        started(
            OperationTarget::Gate(GateName::new("migrate").unwrap()),
            &db,
            "gate-op"
        ),
        vec!["api"],
        "the gate's body starts what it holds"
    );
}

// spec: GATE#check
#[test]
fn rt_start_and_rt_stop_leave_out_the_job_of_a_gates_check() {
    use crate::runtime::barrier::replay::DbActionLog;

    let script = r#"
        let probe = app.job("probe").image("docker.io/library/tools:1").deadline(5);
        app.gate("ready").key(1).check(probe).run(|rt| { rt.start(app); });
        app.deployment("plain").image("docker.io/library/nginx:1");
        app.on_action("poke", |rt, _p| { rt.start(app); });
        app.on_action("halt", |rt, _p| { rt.stop(app.job("probe")); });
    "#;
    let name = AppName::new("demo").unwrap();
    let app = evaluated(script);
    let db = DbHandle::open_in_memory().unwrap();
    {
        let (name, script) = (name.clone(), script.to_owned());
        db.call(move |db| generations::register_script(db, &name, &script).unwrap());
    }
    let acted_on = |target: OperationTarget, id: &str| -> Vec<String> {
        let log = DbActionLog::new(db.clone(), OperationId(id.to_owned()), name.clone(), target);
        let mut names: Vec<_> = log
            .load()
            .unwrap()
            .into_iter()
            .flat_map(|entry| entry.resources)
            .filter_map(|resource| resource.name)
            .collect();
        names.sort();
        names
    };

    for (action, id, expected) in [
        ("poke", "op-poke", vec!["plain"]),
        ("halt", "op-halt", vec![]),
    ] {
        let end = run_action_body(&app, action, &db, id);
        assert!(matches!(end, OperationEnd::Completed), "{action}");
        assert_eq!(
            acted_on(ActionName::new(action).unwrap().into(), id),
            expected,
            "{action} leaves the check's Job alone"
        );
    }

    let end = run_gate_body(&app, script, "ready", None, &db, "op-gate");
    assert!(matches!(end, OperationEnd::Completed));
    assert_eq!(
        acted_on(
            OperationTarget::Gate(GateName::new("ready").unwrap()),
            "op-gate"
        ),
        vec!["plain"],
        "the gate's own body leaves its check's Job alone"
    );
}

/// Run the action `name` of `app` outside any gate, against `db`.
fn run_action_body(app: &App, name: &str, db: &DbHandle, operation_id: &str) -> OperationEnd {
    let action = ActionName::new(name).unwrap();
    let tx = new_event_channel();
    let op_ctx = tx.operation(
        AppName::new("demo").unwrap(),
        action.into(),
        operation_id,
        1,
        None,
    );
    super::run_operation_loop(
        app,
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
        &crate::runtime::gates::GateRuntime::new(),
        &RwLock::new(crate::runtime::apps::AppRegistry::new()),
        Arc::new(CancelToken::new()),
        None,
        None,
        None,
        None,
        Persisted::No,
    )
}

const HELD_SCRIPT: &str = r#"
    let api = app.deployment("api").image("docker.io/library/nginx:1");
    let migrate = app.gate("migrate").key(1).exclusive(api);
    app.deployment("web").image("docker.io/library/nginx:1").after(migrate);
    app.on_action("restart-api", |rt, _p| { rt.restart(api); });
    app.on_action("restart-web", |rt, _p| { rt.restart(app.deployment("web")); });
    app.on_action("stop-all", |rt, _p| { rt.stop(app); });
"#;

fn db_holding_api(script: &str) -> DbHandle {
    let db = DbHandle::open_in_memory().unwrap();
    let (name, script) = (AppName::new("demo").unwrap(), script.to_owned());
    db.call(move |db| {
        generations::register_script(db, &name, &script).unwrap();
        let api = crate::defs::resource::ResourceId {
            kind: crate::defs::resource::ResourceKind::Deployment,
            name: Arc::new("api".to_owned()),
        };
        crate::runtime::gates::store::set_hold(
            db,
            &name,
            &GateName::new("migrate").unwrap(),
            &[api].into(),
        )
        .unwrap();
    });
    db
}

// spec: RT#resources-a-gate-holds
#[test]
fn rt_restart_of_a_deployment_a_gate_holds_or_that_waits_on_a_gate_throws() {
    let app = evaluated(HELD_SCRIPT);
    let db = db_holding_api(HELD_SCRIPT);
    for (action, id) in [("restart-api", "op-api"), ("restart-web", "op-web")] {
        let why = failure_of(run_action_body(&app, action, &db, id));
        assert!(why.contains("a gate holds it"), "{action}: {why}");
    }
}

// spec: RT#resources-a-gate-holds
#[test]
fn rt_stop_leaves_out_what_a_gate_holds_and_what_waits_on_it() {
    use crate::runtime::barrier::replay::DbActionLog;

    let name = AppName::new("demo").unwrap();
    let app = evaluated(HELD_SCRIPT);
    let db = db_holding_api(HELD_SCRIPT);
    let end = run_action_body(&app, "stop-all", &db, "op-stop");
    assert!(
        matches!(end, OperationEnd::Completed),
        "{}",
        match end {
            OperationEnd::Failed(why) | OperationEnd::NotRun(why) => why,
            _ => String::new(),
        }
    );
    let log = DbActionLog::new(
        db.clone(),
        OperationId("op-stop".to_owned()),
        name,
        ActionName::new("stop-all").unwrap().into(),
    );
    let stopped: Vec<_> = log
        .load()
        .unwrap()
        .into_iter()
        .flat_map(|entry| entry.resources)
        .filter_map(|resource| resource.name)
        .collect();
    assert!(
        stopped.is_empty(),
        "api is held and web waits on the unsatisfied gate: {stopped:?}"
    );
}

// spec: RT#resources-a-gate-holds
#[test]
fn a_call_that_would_act_on_a_workload_fails_while_what_gates_hold_cannot_be_read() {
    let script = r#"
        let api = app.deployment("api").image("docker.io/library/nginx:1");
        app.gate("migrate").key(1).exclusive(api);
        app.on_action("poke", |rt, _p| { rt.start(app); });
    "#;
    let app = evaluated(script);
    let db = db_holding_api(script);
    db.call(|db| {
        db.conn
            .execute("ALTER TABLE gate_holds RENAME TO gate_holds_away", [])
            .unwrap();
    });
    let why = failure_of(run_action_body(&app, "poke", &db, "op-unreadable"));
    assert!(why.contains("cannot tell whether a gate holds"), "{why}");
}

/// What the action `derive` threw when replayed against an operation persisted
/// with `seed`, as after a restart severs everything held in memory. The
/// action throws `thrown`, an expression over the derived value `id`.
fn derived_on_replay(seed: &crate::runtime::seeds::Seed, thrown: &str) -> String {
    let name = AppName::new("demo").unwrap();
    let script = format!(
        r#"app.on_action("derive", |rt, _p| {{ let id = std::uuid_v4("id"); throw {thrown}; }});"#
    );
    let (app, _) = crate::runtime::apps::evaluate_script(
        &name,
        &script,
        &Default::default(),
        &crate::ScriptLimits::default(),
    );
    let db = DbHandle::open_in_memory().unwrap();
    let cipher = Arc::new(Cipher::for_tests());
    let operation_id = OperationId::new();
    let target: OperationTarget = ActionName::new("derive").unwrap().into();
    {
        let record = crate::runtime::history::CurrentOperation {
            operation_id: operation_id.clone(),
            app: name.clone(),
            target: target.clone(),
            generation: 0,
        };
        let (cipher, seed, id) = (Arc::clone(&cipher), seed.clone(), operation_id.clone());
        db.call(move |db| {
            crate::runtime::history::save_current_operation(
                db,
                &cipher,
                &record,
                &serde_json::Map::new(),
                &HashMap::new(),
            )
            .unwrap();
            crate::runtime::history::record_operation_seed(db, &cipher, &id, &seed);
        });
    }
    let tx = new_event_channel();
    let op_ctx = tx.operation(name, target, operation_id.0.clone(), 0, None);
    let end = super::run_operation_loop(
        &app,
        db,
        Ok(BoundParams {
            params: serde_json::Map::new(),
            bindings: HashMap::new(),
        }),
        Arc::new(RwLock::new(None)),
        Arc::new(Notify::new()),
        &op_ctx,
        &crate::ScriptLimits::default(),
        cipher,
        &crate::runtime::gates::GateRuntime::new(),
        &RwLock::new(crate::runtime::apps::AppRegistry::new()),
        Arc::new(CancelToken::new()),
        None,
        None,
        None,
        None,
        Persisted::Yes,
    );
    match end {
        OperationEnd::Failed(thrown) => thrown,
        _ => panic!("expected the throw"),
    }
}

// spec: SEC#operation-seeds
#[test]
fn a_replay_derives_from_the_seed_on_record() {
    // A part of the derived value is not the value itself, so it is not
    // withheld from what the throw reports.
    let prefix = r#"id.sub_string(0, 8)"#;
    let seed = crate::runtime::seeds::Seed::from_bytes([1; 32]);
    let first = derived_on_replay(&seed, prefix);
    assert_eq!(first, derived_on_replay(&seed, prefix));
    assert_ne!(
        first,
        derived_on_replay(&crate::runtime::seeds::Seed::from_bytes([2; 32]), prefix)
    );
}

// spec: SEC#withheld-values
// spec: SEC#matching
#[test]
fn a_failure_masks_a_value_the_operation_derived() {
    let seed = crate::runtime::seeds::Seed::from_bytes([1; 32]);
    let thrown = derived_on_replay(&seed, r#"`password=${id};`"#);
    assert!(thrown.contains("password=<redacted>;"), "{thrown}");
}
