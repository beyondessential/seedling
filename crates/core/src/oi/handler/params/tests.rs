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

// spec: PRM#parameter-set
// spec: PRM#parameter-storage
#[test]
fn set_param_bumps_generation_and_stores_value() {
    let oi = oi_with_params_app();
    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "name": "greeting", "value": "hello" }),
        )
        .unwrap();
    assert_eq!(result["generation"], 2);
    assert_eq!(result["schedule"], "not_scheduled");

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    let params = desc["params"].as_array().unwrap();
    let greeting = params
        .iter()
        .find(|p| p["name"] == "greeting")
        .expect("greeting param");
    assert_eq!(greeting["value"], "hello");
    assert_eq!(greeting["is_set"], true);
}

// spec: PRM#parameter-set
#[test]
fn set_same_value_is_a_noop() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "name": "greeting", "value": "hello" }),
    )
    .unwrap();
    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "name": "greeting", "value": "hello" }),
        )
        .unwrap();
    assert_eq!(result["generation"], 2);
    assert_eq!(result["schedule"], "not_scheduled");
}

// spec: PRM#parameter-storage
// spec: APPS#parameters
#[test]
fn secret_param_is_redacted_in_describe_and_history() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "name": "api-key", "value": "hunter2hunter2" }),
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
        .find(|e| e["kind"] == "param_set")
        .expect("param_set entry");
    assert_eq!(entry["redacted"], true);
    assert!(entry.get("new_value").is_none());
}

// spec: PRM#parameter-storage
#[test]
fn set_undeclared_param_is_stored_as_unknown() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "name": "mystery", "value": "42" }),
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

// spec: PRM#parameter-unset
#[test]
fn unset_param_removes_value_and_bumps_generation() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "name": "greeting", "value": "hello" }),
    )
    .unwrap();
    let result = oi
        .call(
            "/apps/params/unset",
            json!({ "app": "demo", "name": "greeting" }),
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
        json!({ "app": "demo", "name": "greeting", "value": "hello" }),
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

// spec: PRM#parameter-unset
#[test]
fn unset_never_set_param_is_a_noop() {
    let oi = oi_with_params_app();
    let result = oi
        .call(
            "/apps/params/unset",
            json!({ "app": "demo", "name": "greeting" }),
        )
        .unwrap();
    assert_eq!(result["generation"], 1);
    assert_eq!(result["schedule"], "not_scheduled");
}

// spec: PRM#parameter-set
#[test]
fn param_calls_on_unknown_app_are_not_found() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call(
            "/apps/params/set",
            json!({ "app": "ghost", "name": "greeting", "value": "x" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    let (code, _) = oi
        .call(
            "/apps/params/unset",
            json!({ "app": "ghost", "name": "greeting" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// spec: PRM#parameter-set, TRD#uninstall-admission
// The handler was asked for during a teardown, which the scheduler refuses,
// and the request failed after the value had been stored.
#[test]
fn a_param_set_during_an_uninstall_stores_the_value_and_runs_no_handler() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"
                app.deployment("web").image("docker.io/library/nginx:1.29");
                let p = app.param("greeting");
                p.on_change(|rt| {});
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
            json!({ "app": "demo", "name": "greeting", "value": "hello" }),
        )
        .unwrap();
    assert_eq!(result["schedule"], "not_scheduled");
    assert!(oi.state.scheduler.lock().active().is_none());
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
        json!({ "app": "demo", "name": "apikey", "value": "hunter2" }),
    )
    .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "name": "mode", "value": "secure" }),
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

// spec: APPS#generation-history
// A handler queued behind another app's operation is still the operation the
// change triggered, but only one that started at once was recorded against
// the generation.
#[test]
fn a_queued_on_change_handler_is_recorded_against_its_generation() {
    use seedling_protocol::names::{ActionName, AppName};

    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"
                app.deployment("web").image("docker.io/library/nginx:1.29");
                let p = app.param("greeting");
                p.on_change(|rt| {});
            "#,
        }),
    )
    .unwrap();
    oi.install("demo");

    // Another app holds the one operation slot.
    oi.state.scheduler.lock().admit(
        &AppName::new_unchecked("other"),
        &ActionName::new_unchecked("backup"),
        serde_json::Map::new(),
        1,
        1,
        "test",
        &parking_lot::Mutex::new(crate::runtime::apps::AppPhase::Installed),
    );

    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "name": "greeting", "value": "hello" }),
        )
        .unwrap();
    assert_eq!(result["schedule"], "accepted");
    let queued = oi
        .state
        .scheduler
        .lock()
        .queue_iter()
        .find(|q| q.app.as_str() == "demo")
        .map(|q| q.operation_id.0.clone())
        .expect("the handler is queued");

    let generations = oi
        .call("/apps/generations", json!({ "app": "demo" }))
        .unwrap();
    let entry = generations
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["generation"] == result["generation"])
        .expect("the change's generation");
    assert_eq!(entry["operation_id"], queued.as_str());
    assert_eq!(entry["outcome"], "pending");
}
