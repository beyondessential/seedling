use jiff::Timestamp;
use seedling_protocol::{
    events::OperationTarget,
    names::{AppName, GateName},
};
use serde_json::{Value, json};

use crate::{
    defs::resource::{ResourceId, ResourceKind},
    oi::test_support::TestOi,
    runtime::{apps::AppPhase, gates::store, secrets::KeyDigest},
};

const SCRIPT: &str = r#"
    let api = app.deployment("api").image("docker.io/library/nginx:1.29");
    let migrate = app.gate("migrate").key(1).exclusive(api);
    let seed = app.gate("seed").key(1).after(migrate);
    app.deployment("web").image("docker.io/library/nginx:1.29").after(seed);
    app.deployment("plain").image("docker.io/library/nginx:1.29");
"#;

fn app_name() -> AppName {
    AppName::new("demo").unwrap()
}

fn gate(name: &str) -> GateName {
    GateName::new(name).unwrap()
}

fn gated() -> TestOi {
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "demo", "script": SCRIPT }))
        .unwrap();
    oi.install("demo");
    oi
}

fn digest(oi: &TestOi, name: &str) -> KeyDigest {
    let reg = oi.state.registry.read();
    let def = reg.get("demo").unwrap().app.def.load_full();
    def.gates[&gate(name)].key.digest(&oi.state.cipher).unwrap()
}

fn satisfy(oi: &TestOi, name: &str, generation: u64) {
    let key = digest(oi, name);
    let name = gate(name);
    oi.state
        .db
        .call(move |db| store::record_success(db, &app_name(), &name, &key, generation).unwrap());
}

fn show(oi: &TestOi) -> Value {
    oi.call("/apps/show", json!({ "app": "demo" })).unwrap()
}

fn gate_entry(shown: &Value, name: &str) -> Value {
    shown["gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"] == name)
        .unwrap_or_else(|| panic!("no gate {name} in {shown}"))
        .clone()
}

fn resource_entry(shown: &Value, name: &str) -> Value {
    shown["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("no resource {name} in {shown}"))
        .clone()
}

fn hold(oi: &TestOi, name: &str, resource: &str) {
    let (name, id) = (
        gate(name),
        ResourceId {
            kind: ResourceKind::Deployment,
            name: std::sync::Arc::new(resource.to_owned()),
        },
    );
    oi.state.db.call(move |db| {
        store::set_hold(db, &app_name(), &name, &[id].into()).unwrap();
    });
}

fn admit_gate_run(oi: &TestOi, app: &str, gate_name: &str) {
    oi.state.scheduler.lock().admit(
        &AppName::new(app).unwrap(),
        &OperationTarget::Gate(gate(gate_name)),
        crate::runtime::param_map::ParamMap::default(),
        1,
        "test",
        &parking_lot::Mutex::new(AppPhase::Installed),
        None,
    );
}

fn run(oi: &TestOi, gate_name: &str) -> Result<Value, (String, String)> {
    oi.call(
        "/apps/gates/run",
        json!({ "app": "demo", "gate": gate_name }),
    )
}

fn paused_for(oi: &TestOi) -> Option<KeyDigest> {
    oi.state.db.call(|db| {
        store::load_records(db, &app_name()).unwrap()[&gate("migrate")]
            .clone()
            .unwrap()
            .paused_key_digest
    })
}

fn pause(oi: &TestOi) {
    let key = digest(oi, "migrate");
    oi.state.db.call(move |db| {
        store::record_failure(
            db,
            &app_name(),
            &gate("migrate"),
            &store::FailedRuns::one(key, Timestamp::now(), "cancelled", true),
        )
        .unwrap();
    });
}

// spec: APPS#gates
#[test]
fn show_reports_each_gate_in_declaration_order_with_its_state_and_targets() {
    let oi = gated();
    let shown = show(&oi);

    let names: Vec<_> = shown["gates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["migrate", "seed"]);

    let migrate = gate_entry(&shown, "migrate");
    assert_eq!(migrate["state"], "unsatisfied");
    assert_eq!(migrate["last_success_generation"], Value::Null);
    assert_eq!(migrate["holds_exclusive"], false);
    assert_eq!(migrate["next_run_at"], Value::Null);
    assert_eq!(migrate["after"], json!([]));
    assert_eq!(
        gate_entry(&shown, "seed")["after"],
        json!([{ "kind": "gate", "name": "migrate" }])
    );

    satisfy(&oi, "migrate", 1);
    let shown = show(&oi);
    let migrate = gate_entry(&shown, "migrate");
    assert_eq!(migrate["state"], "satisfied");
    assert_eq!(migrate["last_success_generation"], 1);
    assert_eq!(gate_entry(&shown, "seed")["state"], "unsatisfied");
}

// spec: APPS#resources
#[test]
fn a_workload_reports_the_targets_it_waits_on_and_the_gates_that_hold_it() {
    let oi = gated();
    let shown = show(&oi);
    assert_eq!(
        resource_entry(&shown, "web")["waiting_on"],
        json!([{ "kind": "gate", "name": "seed" }])
    );
    assert_eq!(resource_entry(&shown, "plain")["waiting_on"], json!([]));
    assert_eq!(resource_entry(&shown, "api")["held_by"], json!([]));

    satisfy(&oi, "migrate", 1);
    satisfy(&oi, "seed", 1);
    assert_eq!(
        resource_entry(&show(&oi), "web")["waiting_on"],
        json!([]),
        "nothing it is ordered after is unready"
    );

    hold(&oi, "migrate", "api");
    let shown = show(&oi);
    assert_eq!(resource_entry(&shown, "api")["held_by"], json!(["migrate"]));
    assert_eq!(gate_entry(&shown, "migrate")["holds_exclusive"], true);
    assert_eq!(resource_entry(&shown, "plain")["held_by"], json!([]));
}

// spec: APPS#resources
#[test]
fn a_workload_held_back_by_a_pending_check_waits_on_the_check_rather_than_the_satisfied_gate() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "demo", "script": r#"
            let probe = app.job("probe").image("docker.io/library/tools:1").deadline("30s");
            let migrate = app.gate("migrate").key(1).check(probe);
            app.deployment("web").image("docker.io/library/nginx:1.29").after(migrate);
        "# }),
    )
    .unwrap();
    oi.install("demo");
    satisfy(&oi, "migrate", 1);

    let shown = show(&oi);
    assert_eq!(gate_entry(&shown, "migrate")["state"], "satisfied");
    assert_eq!(
        resource_entry(&shown, "web")["waiting_on"],
        json!([{ "kind": "check", "name": "migrate" }])
    );
}

// spec: APPS#gates
#[test]
fn a_gate_is_running_while_its_run_is_in_progress() {
    let oi = gated();
    admit_gate_run(&oi, "demo", "migrate");

    let shown = show(&oi);
    assert_eq!(gate_entry(&shown, "migrate")["state"], "running");
    assert_eq!(gate_entry(&shown, "seed")["state"], "unsatisfied");
    assert_eq!(shown["current_operation"]["gate"], "migrate");
    assert!(shown["current_operation"].get("action_name").is_none());
    assert_eq!(shown["current_operation"]["generation"], 1);

    let list = oi.call("/apps/list", json!({})).unwrap();
    assert_eq!(list[0]["gate"], "migrate");
    assert!(list[0].get("action_name").is_none());
}

// spec: APPS#gates
#[test]
fn a_gate_a_cancelled_run_paused_is_reported_paused() {
    let oi = gated();
    pause(&oi);
    let migrate = gate_entry(&show(&oi), "migrate");
    assert_eq!(migrate["state"], "paused");
    assert_eq!(migrate["next_run_at"], Value::Null, "no run is due");
}

// spec: APPS#gates
#[test]
fn a_gate_whose_last_run_failed_reports_when_it_runs_next() {
    let oi = gated();
    let key = digest(&oi, "migrate");
    oi.state.db.call(move |db| {
        store::record_failure(
            db,
            &app_name(),
            &gate("migrate"),
            &store::FailedRuns::one(key, Timestamp::now(), "the body threw", false),
        )
        .unwrap();
    });
    let migrate = gate_entry(&show(&oi), "migrate");
    assert_eq!(migrate["state"], "unsatisfied");
    let next: Timestamp = migrate["next_run_at"].as_str().unwrap().parse().unwrap();
    assert!(
        next > Timestamp::now(),
        "the delay is still to pass: {next}"
    );
}

// spec: APPS#gates
// spec: GRUN#gate-state
#[test]
fn a_gate_whose_record_cannot_be_read_is_undetermined_not_unsatisfied() {
    let oi = gated();
    satisfy(&oi, "migrate", 1);
    oi.state.db.call(|db| {
        db.conn
            .execute("UPDATE gate_records SET key_digest = x'0102'", [])
            .unwrap();
    });

    let shown = show(&oi);
    assert_eq!(gate_entry(&shown, "migrate")["state"], "undetermined");
}

// spec: APPS#description
#[test]
fn what_the_gates_hold_cannot_be_read_so_the_description_is_internal() {
    let oi = gated();
    oi.state.db.call(|db| {
        db.conn.execute("DROP TABLE gate_holds", []).unwrap();
    });
    let (code, _) = oi.call("/apps/show", json!({ "app": "demo" })).unwrap_err();
    assert_eq!(code, "internal");
}

// spec: CTL#deployment-restart
#[test]
fn a_restart_is_refused_while_a_gate_holds_the_deployment() {
    let oi = gated();
    hold(&oi, "migrate", "api");

    let (code, message) = oi
        .call(
            "/apps/restart",
            json!({ "app": "demo", "deployment": "api" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(message.contains("migrate"), "names the gate: {message}");

    oi.call(
        "/apps/restart",
        json!({ "app": "demo", "deployment": "plain" }),
    )
    .expect("a deployment no gate holds restarts");
}

// spec: CTL#resource-stop
#[test]
fn a_resource_a_gate_holds_can_be_stopped() {
    let oi = gated();
    hold(&oi, "migrate", "api");

    oi.call(
        "/apps/resource/stop",
        json!({ "app": "demo", "kind": "deployment", "name": "api" }),
    )
    .expect("the stop of a held resource is accepted");

    let shown = show(&oi);
    let api = resource_entry(&shown, "api");
    assert_eq!(api["stopped"], true);
    assert_eq!(api["held_by"], json!(["migrate"]));
    assert_eq!(
        shown["stopped_resources"],
        json!([{ "kind": "deployment", "name": "api" }])
    );
}

// spec: CTL#gate-runs
#[test]
fn a_gate_run_is_refused_for_what_is_not_there_or_not_installed() {
    let oi = TestOi::new();
    let (code, _) = run(&oi, "migrate").unwrap_err();
    assert_eq!(code, "not_found", "unregistered app");

    oi.call("/apps/create", json!({ "app": "demo", "script": SCRIPT }))
        .unwrap();
    let (code, _) = run(&oi, "nope").unwrap_err();
    assert_eq!(code, "not_found", "undeclared gate");
    let (code, _) = run(&oi, "migrate").unwrap_err();
    assert_eq!(code, "not_installed");

    oi.install("demo");
    oi.state
        .registry
        .write()
        .get_mut("demo")
        .unwrap()
        .script_error = Some(("it broke".to_owned(), Timestamp::now()));
    let (code, _) = run(&oi, "migrate").unwrap_err();
    assert_eq!(code, "script_error");
}

// spec: CTL#gate-runs
#[test]
fn a_gate_run_is_refused_while_a_target_is_not_ready_naming_each() {
    let oi = gated();
    let (code, message) = run(&oi, "seed").unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(message.contains("gate 'migrate'"), "{message}");

    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"
                let database = app.deployment("database").image("docker.io/library/postgres:17");
                let cache = app.deployment("cache").image("docker.io/library/redis:7");
                app.gate("seed").key(1).after([database, cache]);
            "#,
        }),
    )
    .unwrap();
    oi.install("demo");
    let (code, message) = run(&oi, "seed").unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(
        message.contains("deployment 'database'") && message.contains("deployment 'cache'"),
        "names each target: {message}"
    );
}

// spec: CTL#gate-runs
#[test]
fn a_gate_run_is_refused_while_an_operation_runs_or_is_queued() {
    let oi = gated();
    pause(&oi);
    admit_gate_run(&oi, "demo", "migrate");

    let (code, _) = run(&oi, "migrate").unwrap_err();
    assert_eq!(code, "operation_in_progress");
    assert!(paused_for(&oi).is_some(), "a refusal leaves the pause");

    let oi = gated();
    admit_gate_run(&oi, "other", "migrate");
    admit_gate_run(&oi, "demo", "migrate");
    let (code, _) = run(&oi, "migrate").unwrap_err();
    assert_eq!(code, "already_queued");
}

// spec: CTL#gate-runs
#[test]
fn a_gate_run_is_accepted_for_a_satisfied_gate() {
    let oi = gated();
    satisfy(&oi, "migrate", 1);
    let result = run(&oi, "migrate").expect("a satisfied gate runs on request");
    assert_eq!(result["schedule"], "accepted");
    assert!(!result["operation_id"].as_str().unwrap().is_empty());
}

// spec: CTL#gate-runs
// spec: GRUN#pauses
#[test]
fn running_a_paused_gate_ends_its_pause() {
    let oi = gated();
    pause(&oi);
    assert!(paused_for(&oi).is_some());

    // Another app holds the one operation slot, so the run queues and has not
    // begun to clear the pause itself.
    admit_gate_run(&oi, "other", "migrate");
    let result = run(&oi, "migrate").expect("an operator runs a paused gate");
    assert_eq!(result["schedule"], "queued");
    assert_eq!(paused_for(&oi), None);
}

// spec: PRM#change-planning
#[test]
fn a_plan_lists_the_gates_the_proposal_would_leave_unsatisfied() {
    let oi = gated();
    satisfy(&oi, "migrate", 1);
    satisfy(&oi, "seed", 1);

    let plan = oi
        .call(
            "/apps/plan",
            json!({ "app": "demo", "proposed_script": SCRIPT }),
        )
        .unwrap();
    assert_eq!(plan["gates_would_run"], json!([]), "no key moves");

    let moved = SCRIPT.replace(r#"gate("migrate").key(1)"#, r#"gate("migrate").key(2)"#);
    let plan = oi
        .call(
            "/apps/plan",
            json!({ "app": "demo", "proposed_script": moved }),
        )
        .unwrap();
    assert_eq!(plan["gates_would_run"], json!(["migrate"]));

    let both = moved.replace(r#"gate("seed").key(1)"#, r#"gate("seed").key(2)"#);
    let plan = oi
        .call(
            "/apps/plan",
            json!({ "app": "demo", "proposed_script": both }),
        )
        .unwrap();
    assert_eq!(plan["gates_would_run"], json!(["migrate", "seed"]));

    let shown = show(&oi);
    assert_eq!(
        gate_entry(&shown, "migrate")["state"],
        "satisfied",
        "a plan changes nothing"
    );
}

// spec: PRM#change-planning
#[test]
fn a_plan_with_no_proposal_lists_the_gates_that_are_unsatisfied_now() {
    let oi = gated();
    satisfy(&oi, "migrate", 1);
    let plan = oi.call("/apps/plan", json!({ "app": "demo" })).unwrap();
    assert_eq!(plan["gates_would_run"], json!(["seed"]));
}

// spec: PRM#change-planning
#[test]
fn a_plan_for_an_app_that_is_not_installed_lists_no_gates() {
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "demo", "script": SCRIPT }))
        .unwrap();
    let moved = SCRIPT.replace(r#"gate("migrate").key(1)"#, r#"gate("migrate").key(2)"#);
    let plan = oi
        .call(
            "/apps/plan",
            json!({ "app": "demo", "proposed_script": moved }),
        )
        .unwrap();
    assert_eq!(plan["gates_would_run"], json!([]));
}

// spec: PRM#change-planning
// spec: GRUN#gate-state
#[test]
fn a_plan_does_not_fold_an_unreadable_gate_record_into_an_answer() {
    let oi = gated();
    satisfy(&oi, "migrate", 1);
    oi.state.db.call(|db| {
        db.conn
            .execute("UPDATE gate_records SET key_digest = x'0102'", [])
            .unwrap();
    });
    let (code, _) = oi
        .call(
            "/apps/plan",
            json!({ "app": "demo", "proposed_script": SCRIPT }),
        )
        .unwrap_err();
    assert_eq!(code, "internal");
}
