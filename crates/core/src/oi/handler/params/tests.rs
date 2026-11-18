use serde_json::json;

use crate::oi::test_support::{PARAMS_SCRIPT, TestOi};

fn oi_with_params_app() -> TestOi {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "demo", "script": PARAMS_SCRIPT }),
    )
    .expect("register app");
    oi
}

// spec: PRM#parameter-changes
// spec: PRM#parameter-storage
#[test]
fn set_param_bumps_generation_and_stores_value() {
    let oi = oi_with_params_app();
    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "greeting": "hello" } }),
        )
        .unwrap();
    assert_eq!(result, json!({ "generation": 2 }));

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    let params = desc["params"].as_array().unwrap();
    let greeting = params
        .iter()
        .find(|p| p["name"] == "greeting")
        .expect("greeting param");
    assert_eq!(greeting["value"], "hello");
    assert_eq!(greeting["is_set"], true);
}

// spec: PRM#parameter-changes
#[test]
fn set_same_value_is_a_noop() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "greeting": "hello" } }),
    )
    .unwrap();
    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "greeting": "hello" } }),
        )
        .unwrap();
    assert_eq!(result["generation"], 2);
}

// spec: PRM#parameter-storage
// spec: APPS#parameters
#[test]
fn secret_param_is_redacted_in_describe_and_history() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "api-key": "hunter2hunter2" } }),
    )
    .unwrap();

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    let params = desc["params"].as_array().unwrap();
    let key = params
        .iter()
        .find(|p| p["name"] == "api-key")
        .expect("api-key param");
    assert_eq!(key["is_set"], true);
    assert!(key["value"].is_null());

    let generations = oi
        .call("/apps/generations", json!({ "app": "demo" }))
        .unwrap();
    let entry = generations
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "param_change")
        .expect("param_change entry");
    let changed = &entry["params"][0];
    assert_eq!(changed["name"], "api-key");
    assert_eq!(changed["redacted"], true);
    assert!(changed.get("new_value").is_none());
    assert!(!generations.to_string().contains("hunter2hunter2"));
}

fn drain_events(
    rx: &mut tokio::sync::broadcast::Receiver<seedling_protocol::events::OiEvent>,
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        out.push(serde_json::to_value(&event).unwrap());
    }
    out
}

// spec: SEC#redaction
// spec: EVT#app-events
#[test]
fn setting_and_unsetting_a_secret_param_emits_no_value() {
    let oi = oi_with_params_app();
    let mut rx = oi.state.event_tx.subscribe();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "api-key": "hunter2hunter2" } }),
    )
    .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "api-key": "correcthorse9" } }),
    )
    .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "api-key": null } }),
    )
    .unwrap();

    let events = drain_events(&mut rx);
    let events_of_changes: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "ParamsChanged")
        .collect();
    assert_eq!(events_of_changes.len(), 3);
    for event in events_of_changes {
        let change = &event["changes"][0];
        assert_eq!(change["name"], "api-key", "{event}");
        assert_eq!(change["redacted"], true, "{event}");
        assert!(change.get("previous_value").is_none(), "{event}");
        assert!(change.get("new_value").is_none(), "{event}");
    }
    let everything = serde_json::to_string(&events).unwrap();
    for value in ["hunter2hunter2", "correcthorse9"] {
        assert!(!everything.contains(value), "{value} leaked: {everything}");
    }
}

// spec: SEC#redaction
// A value still in plain storage under a parameter the definition now marks
// secret is withheld from the event as it is from every response.
#[test]
fn unsetting_a_param_the_definition_makes_secret_withholds_a_plain_stored_value() {
    let oi = oi_with_params_app();
    oi.state.db.call(|db| {
        let app = seedling_protocol::names::AppName::new_unchecked("demo");
        let key = seedling_protocol::names::ParamName::new_unchecked("api-key");
        crate::runtime::apps::upsert_param(db, &app, &key, "plain-leftover-1").unwrap();
    });
    let mut rx = oi.state.event_tx.subscribe();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "api-key": null } }),
    )
    .unwrap();

    let events = drain_events(&mut rx);
    let changed = events
        .iter()
        .find(|e| e["type"] == "ParamsChanged")
        .expect("a ParamsChanged event");
    let entry = &changed["changes"][0];
    assert_eq!(entry["redacted"], true, "{changed}");
    assert!(entry.get("previous_value").is_none(), "{changed}");
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("plain-leftover-1")
    );
}

// spec: SEC#redaction
#[test]
fn a_definition_update_changing_a_secret_param_emits_no_value() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "api-key": "hunter2hunter2" } }),
    )
    .unwrap();
    let mut rx = oi.state.event_tx.subscribe();
    oi.call(
        "/apps/update",
        json!({
            "app": "demo",
            "script": PARAMS_SCRIPT,
            "params": { "api-key": "correcthorse9" },
        }),
    )
    .unwrap();

    let events = drain_events(&mut rx);
    let updated = events
        .iter()
        .find(|e| e["type"] == "AppUpdated")
        .expect("an AppUpdated event");
    assert_eq!(
        updated["changes"],
        json!([{ "name": "api-key", "redacted": true }])
    );
    let everything = serde_json::to_string(&events).unwrap();
    for value in ["hunter2hunter2", "correcthorse9"] {
        assert!(!everything.contains(value), "{value} leaked: {everything}");
    }
}

// spec: PRM#parameter-storage
#[test]
fn set_undeclared_param_is_stored_as_unknown() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "mystery": "42" } }),
    )
    .unwrap();

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    let unknown = desc["unknown_params"].as_array().unwrap();
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0]["name"], "mystery");
    assert_eq!(unknown[0]["value"], "42");
}

// spec: SEC#redaction
// spec: APPS#parameters
#[test]
fn unknown_param_held_in_secret_storage_is_not_returned() {
    let oi = oi_with_params_app();
    let cipher = std::sync::Arc::clone(&oi.state.cipher);
    oi.state.db.call(move |db| {
        crate::runtime::apps::secret_params::upsert_secret_param(
            db,
            &cipher,
            &seedling_protocol::names::AppName::new_unchecked("demo"),
            &seedling_protocol::names::ParamName::new_unchecked("retired-token"),
            &secrecy::SecretString::new("hunter2hunter2".into()),
        )
        .unwrap();
    });

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    let unknown = desc["unknown_params"].as_array().unwrap();
    let retired = unknown
        .iter()
        .find(|p| p["name"] == "retired-token")
        .expect("the stored secret is listed as unknown");
    assert!(retired["value"].is_null());
    assert!(!desc.to_string().contains("hunter2hunter2"));
}

// spec: PRM#parameter-changes
#[test]
fn unset_param_removes_value_and_bumps_generation() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "greeting": "hello" } }),
    )
    .unwrap();
    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "greeting": null } }),
        )
        .unwrap();
    assert_eq!(result["generation"], 3);

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    let greeting = desc["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "greeting")
        .expect("greeting param");
    assert_eq!(greeting["is_set"], false);
    assert!(greeting["value"].is_null());
}

// spec: GEN#generation-numbers
#[test]
fn a_param_set_whose_bump_fails_stores_nothing() {
    let oi = oi_with_params_app();
    oi.state.db.call(|db| {
        db.conn
            .execute_batch(
                "CREATE TRIGGER fail_bump BEFORE UPDATE OF current_generation ON registered_apps
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            )
            .unwrap()
    });

    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "greeting": "hello" } }),
    )
    .expect_err("injected failure");

    let (stored, generations): (i64, i64) = oi.state.db.call(|db| {
        let stored = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM params WHERE app_name = 'demo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let generations = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM generations WHERE app = 'demo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        (stored, generations)
    });
    assert_eq!(stored, 0);
    assert_eq!(generations, 1);
}

// spec: PRM#parameter-changes
#[test]
fn unset_never_set_param_is_a_noop() {
    let oi = oi_with_params_app();
    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "greeting": null } }),
        )
        .unwrap();
    assert_eq!(result["generation"], 1);
}

// spec: PRM#parameter-changes
#[test]
fn param_calls_on_unknown_app_are_not_found() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call(
            "/apps/params/set",
            json!({ "app": "ghost", "params": { "greeting": "x" } }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    let (code, _) = oi
        .call(
            "/apps/params/set",
            json!({ "app": "ghost", "params": { "greeting": null } }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// spec: PRM#parameter-changes, TRD#uninstall-admission
#[test]
fn a_param_set_during_an_uninstall_stores_the_value() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"
                app.deployment("web").image("docker.io/library/nginx:1.29");
                app.param("greeting");
            "#,
        }),
    )
    .unwrap();
    oi.install("demo");
    oi.call("/apps/uninstall", json!({ "app": "demo" }))
        .unwrap();

    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "greeting": "hello" } }),
        )
        .unwrap();
    assert_eq!(result["generation"], 2);
    assert!(oi.state.scheduler.lock().active().is_none());
}

// spec: OPS#changes-to-an-app
#[test]
fn a_param_change_initiates_no_operation() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"
                app.deployment("web").image("docker.io/library/nginx:1.29");
                app.param("greeting");
            "#,
        }),
    )
    .unwrap();
    oi.install("demo");

    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "greeting": "hello" } }),
        )
        .unwrap();
    assert_eq!(result["generation"], 2);
    assert!(oi.state.scheduler.lock().active().is_none());
    assert_eq!(oi.state.scheduler.lock().queue_iter().count(), 0);
}

// spec: OPS#changes-to-an-app
#[test]
fn a_param_change_is_accepted_while_an_operation_runs_for_the_app() {
    use seedling_protocol::events::OperationTarget;
    use seedling_protocol::names::{ActionName, AppName};

    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"
                app.deployment("web").image("docker.io/library/nginx:1.29");
                app.param("greeting");
            "#,
        }),
    )
    .unwrap();
    oi.install("demo");
    oi.state.scheduler.lock().admit(
        &AppName::new_unchecked("demo"),
        &OperationTarget::Action(ActionName::new_unchecked("backup")),
        serde_json::Map::new(),
        1,
        "test",
        &parking_lot::Mutex::new(crate::runtime::apps::AppPhase::Installed),
        None,
    );

    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "greeting": "hello" } }),
        )
        .unwrap();
    assert_eq!(result["generation"], 2);
    assert_eq!(
        oi.state.scheduler.lock().active().map(|a| a.target.clone()),
        Some(OperationTarget::Action(ActionName::new_unchecked("backup"))),
        "the operation goes on"
    );
}

// spec: SEC#migration
// Only startup and a definition update migrated, so a param made secret by
// another param's change kept its plaintext row until the daemon restarted.
#[test]
fn a_param_made_secret_by_a_param_change_moves_to_secret_storage() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"
                let mode = app.param("mode").default_value("plain");
                let key = app.param("apikey");
                if mode.value() == "secure" {
                    key.secret(true);
                }
            "#,
        }),
    )
    .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "apikey": "hunter2" } }),
    )
    .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "mode": "secure" } }),
    )
    .unwrap();

    let (plaintext, secret) = oi.state.db.call(|db| {
        let app = seedling_protocol::names::AppName::new_unchecked("demo");
        let key = seedling_protocol::names::ParamName::new_unchecked("apikey");
        let plaintext: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM params WHERE app_name = 'demo' AND param_name = 'apikey'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let secret = crate::runtime::apps::secret_params::is_stored_secret(db, &app, &key).unwrap();
        (plaintext, secret)
    });
    assert_eq!(plaintext, 0, "no plaintext copy remains");
    assert!(secret, "the value is in secret storage");
}

fn stored_values(oi: &TestOi) -> Vec<(String, String)> {
    oi.state.db.call(|db| {
        db.conn
            .prepare(
                "SELECT param_name, value FROM params WHERE app_name = 'demo' ORDER BY param_name",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    })
}

fn generation_count(oi: &TestOi) -> i64 {
    oi.state.db.call(|db| {
        db.conn
            .query_row(
                "SELECT COUNT(*) FROM generations WHERE app = 'demo'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    })
}

// spec: PRM#parameter-changes
// spec: GEN#generation-numbers
// spec: EVT#app-events
#[test]
fn a_change_of_several_parameters_is_one_generation_and_one_event() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "greeting": "hello" } }),
    )
    .unwrap();
    let mut rx = oi.state.event_tx.subscribe();

    let result = oi
        .call(
            "/apps/params/set",
            json!({
                "app": "demo",
                "params": {
                    "mystery": "42",
                    "greeting": null,
                    "api-key": "hunter2hunter2",
                },
            }),
        )
        .unwrap();

    assert_eq!(result, json!({ "generation": 3 }));
    assert_eq!(
        generation_count(&oi),
        3,
        "one generation for the whole change"
    );
    assert_eq!(
        stored_values(&oi),
        vec![("mystery".to_owned(), "42".to_owned())],
        "the secret is held apart from the plaintext values"
    );

    let events = drain_events(&mut rx);
    let changed: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "ParamsChanged")
        .collect();
    assert_eq!(changed.len(), 1, "{events:?}");
    assert_eq!(changed[0]["generation"], 3);
    assert_eq!(changed[0]["previous_generation"], 2);
    assert_eq!(
        changed[0]["changes"],
        json!([
            { "name": "api-key", "redacted": true },
            { "name": "greeting", "previous_value": "hello" },
            { "name": "mystery", "new_value": "42" },
        ]),
        "ascending by name, secret withheld"
    );

    let generations = oi
        .call("/apps/generations", json!({ "app": "demo" }))
        .unwrap();
    let entry = &generations[0];
    assert_eq!(entry["generation"], 3);
    assert_eq!(entry["kind"], "param_change");
    assert_eq!(
        entry["params"],
        json!([
            { "name": "api-key", "redacted": true },
            { "name": "greeting", "previous_value": "hello", "new_value": null },
            { "name": "mystery", "previous_value": null, "new_value": "42" },
        ])
    );
}

// spec: PRM#parameter-changes
#[test]
fn members_that_change_nothing_are_left_out_of_the_change() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "greeting": "hello" } }),
    )
    .unwrap();
    let mut rx = oi.state.event_tx.subscribe();

    let result = oi
        .call(
            "/apps/params/set",
            json!({
                "app": "demo",
                "params": { "greeting": "hello", "mystery": null, "api-key": "k-12345678" },
            }),
        )
        .unwrap();
    assert_eq!(result["generation"], 3);
    let events = drain_events(&mut rx);
    let changed = events
        .iter()
        .find(|e| e["type"] == "ParamsChanged")
        .expect("an event");
    assert_eq!(
        changed["changes"],
        json!([{ "name": "api-key", "redacted": true }])
    );

    let mut rx = oi.state.event_tx.subscribe();
    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "greeting": "hello", "mystery": null } }),
        )
        .unwrap();
    assert_eq!(result["generation"], 3, "a change of nothing is a no-op");
    assert_eq!(generation_count(&oi), 3);
    assert!(
        drain_events(&mut rx)
            .iter()
            .all(|e| e["type"] != "ParamsChanged"),
        "a no-op emits no event"
    );
}

// spec: PRM#parameter-changes
#[test]
fn a_change_the_server_refuses_as_malformed_stores_nothing() {
    let oi = oi_with_params_app();
    let refused = |params: &str| {
        let request = format!(
            r#"{{"method":"/apps/params/set","params":{{"app":"demo","params":{params}}}}}"#
        );
        let (code, message) = oi
            .call_raw(request.as_bytes())
            .expect_err("the change is refused");
        assert_eq!(code, "requirements_invalid", "{params}: {message}");
        message
    };

    refused("{}");
    refused("null");
    refused(r#"["greeting"]"#);
    let message = refused(r#"{"greeting":"a","greeting":"b"}"#);
    assert!(message.contains("greeting"), "{message}");
    let message = refused(r#"{"mystery":"x","not a name":"y"}"#);
    assert!(message.contains("not a name"), "{message}");
    refused(r#"{"_hidden":"y"}"#);
    refused(r#"{"ab":"y"}"#);
    let message = refused(r#"{"greeting":1}"#);
    assert!(message.contains("greeting"), "{message}");
    refused(r#"{"greeting":true}"#);
    refused(r#"{"greeting":{"a":"b"}}"#);
    refused(r#"{"greeting":["a"]}"#);
    refused(r#"{"mystery":"x","greeting":2}"#);

    assert_eq!(stored_values(&oi), vec![]);
    assert_eq!(generation_count(&oi), 1);
}

// spec: PRM#parameter-changes
#[test]
fn a_change_without_params_is_refused() {
    let oi = oi_with_params_app();
    let (code, _) = oi
        .call("/apps/params/set", json!({ "app": "demo" }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    let (code, _) = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "name": "greeting", "value": "hello" }),
        )
        .unwrap_err();
    assert_eq!(
        code, "requirements_invalid",
        "the single-parameter form is gone"
    );
}

// spec: PRM#parameter-changes
#[test]
fn the_unset_method_no_longer_exists() {
    let oi = oi_with_params_app();
    let (code, _) = oi
        .call(
            "/apps/params/unset",
            json!({ "app": "demo", "name": "greeting" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// spec: PRM#validation
// spec: PRM#parameter-changes
#[test]
fn validators_see_the_whole_change_and_a_rejection_stores_none_of_it() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"
                app.param("host").required(false);
                app.param("port").required(false).validate(|v, all| {
                    if "host" in all && v == "0" { throw "port must not be 0 with a host"; }
                });
                app.deployment("web").image("docker.io/library/nginx:1.29");
            "#,
        }),
    )
    .unwrap();

    let (code, message) = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "host": "h.example", "port": "0" } }),
        )
        .unwrap_err();
    assert_eq!(code, "validation_failed", "{message}");
    assert_eq!(stored_values(&oi), vec![]);
    assert_eq!(generation_count(&oi), 1);

    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "host": "h.example", "port": "80" } }),
    )
    .unwrap();
    assert_eq!(generation_count(&oi), 2);
}

// spec: PRM#parameter-changes
// spec: OPS#changes-to-an-app
#[test]
fn a_change_is_refused_while_another_is_being_applied() {
    let oi = oi_with_params_app();
    let demo = seedling_protocol::names::AppName::new_unchecked("demo");
    let hold = crate::runtime::scheduler::AppHold::take(&oi.state.scheduler, &demo)
        .expect("a free app can be held");

    let (code, _) = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "greeting": "hello" } }),
        )
        .unwrap_err();
    assert_eq!(code, "operation_in_progress");
    assert_eq!(stored_values(&oi), vec![]);

    drop(hold);
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "params": { "greeting": "hello" } }),
    )
    .expect("accepted once the other change is applied");
}

// spec: OPS#changes-to-an-app
#[test]
fn a_change_is_accepted_while_an_operation_runs_for_the_app() {
    use seedling_protocol::events::OperationTarget;
    use seedling_protocol::names::{ActionName, AppName};

    let oi = oi_with_params_app();
    oi.install("demo");
    oi.state.scheduler.lock().admit(
        &AppName::new_unchecked("demo"),
        &OperationTarget::Action(ActionName::new_unchecked("backup")),
        serde_json::Map::new(),
        1,
        "test",
        &parking_lot::Mutex::new(crate::runtime::apps::AppPhase::Installed),
        None,
    );

    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "params": { "greeting": "hello", "mystery": "1" } }),
        )
        .expect("an operation does not refuse a change");
    assert_eq!(result["generation"], 2);
    assert_eq!(
        oi.state
            .scheduler
            .lock()
            .active()
            .map(|a| a.target.name().to_owned()),
        Some("backup".to_owned()),
        "the operation goes on"
    );
}

// spec: APPS#update
// spec: EVT#app-events
#[test]
fn an_update_changes_several_parameters_in_one_generation_and_event() {
    let oi = oi_with_params_app();
    let mut rx = oi.state.event_tx.subscribe();

    let result = oi
        .call(
            "/apps/update",
            json!({
                "app": "demo",
                "script": PARAMS_SCRIPT,
                "params": { "greeting": "hello", "mystery": "42", "api-key": "hunter2hunter2" },
            }),
        )
        .unwrap();

    assert_eq!(result, json!({ "generation": 2 }));
    assert_eq!(generation_count(&oi), 2);
    let events = drain_events(&mut rx);
    let updated = events
        .iter()
        .find(|e| e["type"] == "AppUpdated")
        .expect("an AppUpdated event");
    assert_eq!(
        updated["changes"],
        json!([
            { "name": "api-key", "redacted": true },
            { "name": "greeting", "new_value": "hello" },
            { "name": "mystery", "new_value": "42" },
        ])
    );
    let generations = oi
        .call("/apps/generations", json!({ "app": "demo" }))
        .unwrap();
    assert_eq!(generations[0]["kind"], "script_update");
    assert_eq!(generations[0]["params"].as_array().unwrap().len(), 3);
}

// spec: APPS#update
#[test]
fn an_update_without_a_parameter_change_has_no_changes_member() {
    let oi = oi_with_params_app();
    let mut rx = oi.state.event_tx.subscribe();
    oi.call(
        "/apps/update",
        json!({ "app": "demo", "script": PARAMS_SCRIPT }),
    )
    .unwrap();
    let events = drain_events(&mut rx);
    let updated = events
        .iter()
        .find(|e| e["type"] == "AppUpdated")
        .expect("an AppUpdated event");
    assert!(updated.get("changes").is_none(), "{updated}");
}

// spec: APPS#update
#[test]
fn an_update_refuses_what_a_parameter_change_refuses() {
    let oi = oi_with_params_app();
    let script = serde_json::to_string(PARAMS_SCRIPT).unwrap();
    let refused = |params: &str| {
        let request = format!(
            r#"{{"method":"/apps/update","params":{{"app":"demo","script":{script},"params":{params}}}}}"#
        );
        let (code, message) = oi
            .call_raw(request.as_bytes())
            .expect_err("the update is refused");
        assert_eq!(code, "requirements_invalid", "{params}: {message}");
    };
    refused("{}");
    refused(r#"{"greeting":"a","greeting":"b"}"#);
    refused(r#"{"not a name":"y"}"#);
    refused(r#"{"greeting":3}"#);

    assert_eq!(generation_count(&oi), 1);
    assert_eq!(stored_values(&oi), vec![]);
}

// spec: PRM#change-planning
#[test]
fn a_plan_takes_several_proposed_parameters() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"
                let tier = app.param("tier").required(false);
                let extra = app.param("extra").required(false);
                let web = app.deployment("web").image("docker.io/library/nginx:1.29");
                if tier.is_set() { web.env("TIER", tier.value()); }
                if extra.is_set() {
                    app.deployment("extra").image("docker.io/library/nginx:1.29");
                }
            "#,
        }),
    )
    .unwrap();

    let plan = oi
        .call(
            "/apps/plan",
            json!({ "app": "demo", "proposed_params": { "tier": "gold", "extra": "on" } }),
        )
        .unwrap();
    let diff = plan["diff"].as_array().unwrap();
    assert!(
        diff.iter()
            .any(|d| d["resource_name"] == "extra" && d["change"] == "added"),
        "{plan}"
    );
    assert!(
        diff.iter()
            .any(|d| d["resource_name"] == "web" && d["change"] == "modified"),
        "{plan}"
    );
    assert_eq!(generation_count(&oi), 1, "a plan changes nothing");

    let (code, _) = oi
        .call(
            "/apps/plan",
            json!({ "app": "demo", "proposed_params": {} }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
}
