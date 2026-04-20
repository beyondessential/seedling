use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Notify;

use super::params::load_params_for_app;
use super::*;
use crate::runtime::db::Db;

// i[verify param.store]
#[test]
fn upsert_and_load_params_round_trip() {
    let db = Db::open_in_memory().expect("open");
    upsert_param(&db, "myapp", "host", "example.com").expect("upsert");
    upsert_param(&db, "myapp", "port", "8080").expect("upsert");

    let params = load_params_for_app(&db, "myapp").expect("load");
    assert_eq!(params.get("host").map(String::as_str), Some("example.com"));
    assert_eq!(params.get("port").map(String::as_str), Some("8080"));
}

// i[verify param.store]
#[test]
fn upsert_param_replaces_existing_value() {
    let db = Db::open_in_memory().expect("open");
    upsert_param(&db, "myapp", "host", "old.example.com").expect("first upsert");
    upsert_param(&db, "myapp", "host", "new.example.com").expect("second upsert");

    let params = load_params_for_app(&db, "myapp").expect("load");
    assert_eq!(
        params.get("host").map(String::as_str),
        Some("new.example.com")
    );
    assert_eq!(params.len(), 1);
}

fn make_entry(name: &str, script_error: Option<&str>) -> AppEntry {
    AppEntry {
        name: name.to_owned(),
        script: String::new(),
        app: crate::defs::app::App::default(),
        phase: Arc::new(parking_lot::Mutex::new(AppPhase::NotInstalled)),
        active_progress: Arc::new(parking_lot::RwLock::new(None)),
        tick_notify: Arc::new(Notify::new()),
        script_error: script_error.map(|msg| (msg.to_owned(), Timestamp::now())),
        current_generation: 0,
    }
}

// i[verify fault.derived]
#[test]
fn sync_clears_fault_on_successful_reload() {
    let db = Db::open_in_memory().expect("open");

    let entry = make_entry("myapp", Some("has_value not found"));
    sync_script_error_fault(&db, &entry);
    assert_eq!(
        crate::runtime::faults::list_active_faults(&db, Some("myapp"))
            .unwrap()
            .len(),
        1
    );

    let entry = make_entry("myapp", None);
    sync_script_error_fault(&db, &entry);
    assert!(
        crate::runtime::faults::list_active_faults(&db, Some("myapp"))
            .unwrap()
            .is_empty(),
        "fault should be cleared after successful reload"
    );
}

// i[verify fault.derived]
#[test]
fn sync_replaces_fault_when_error_changes() {
    let db = Db::open_in_memory().expect("open");

    let entry = make_entry("myapp", Some("has_value not found"));
    sync_script_error_fault(&db, &entry);
    let faults = crate::runtime::faults::list_active_faults(&db, Some("myapp")).unwrap();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].description, "has_value not found");
    let old_id = faults[0].id.clone();

    let entry = make_entry("myapp", Some("different error"));
    sync_script_error_fault(&db, &entry);
    let faults = crate::runtime::faults::list_active_faults(&db, Some("myapp")).unwrap();
    assert_eq!(
        faults.len(),
        1,
        "should still have exactly one active fault"
    );
    assert_eq!(faults[0].description, "different error");
    assert_ne!(faults[0].id, old_id, "should be a new fault record");
}

// i[verify fault.derived]
#[test]
fn sync_is_idempotent_for_same_error() {
    let db = Db::open_in_memory().expect("open");

    let entry = make_entry("myapp", Some("parse failed"));
    sync_script_error_fault(&db, &entry);
    let first = crate::runtime::faults::list_active_faults(&db, Some("myapp")).unwrap();
    assert_eq!(first.len(), 1);
    let first_id = first[0].id.clone();

    sync_script_error_fault(&db, &entry);
    let second = crate::runtime::faults::list_active_faults(&db, Some("myapp")).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].id, first_id,
        "same error should keep the same fault"
    );
}

// i[verify param.store]
#[test]
fn load_params_scoped_to_app() {
    let db = Db::open_in_memory().expect("open");
    upsert_param(&db, "app-a", "key", "val-a").expect("upsert a");
    upsert_param(&db, "app-b", "key", "val-b").expect("upsert b");

    let params_a = load_params_for_app(&db, "app-a").expect("load a");
    let params_b = load_params_for_app(&db, "app-b").expect("load b");

    assert_eq!(params_a.get("key").map(String::as_str), Some("val-a"));
    assert_eq!(params_b.get("key").map(String::as_str), Some("val-b"));
}

// i[verify param.store]
#[test]
fn load_params_returns_empty_for_unknown_app() {
    let db = Db::open_in_memory().expect("open");
    let params = load_params_for_app(&db, "nonexistent").expect("load");
    assert!(params.is_empty());
}

// i[verify param.store]
#[test]
fn delete_app_params_removes_only_that_apps_params() {
    let db = Db::open_in_memory().expect("open");
    upsert_param(&db, "app-a", "key", "val-a").expect("upsert a");
    upsert_param(&db, "app-b", "key", "val-b").expect("upsert b");

    delete_app_params(&db, "app-a").expect("delete");

    assert!(
        load_params_for_app(&db, "app-a")
            .expect("load a")
            .is_empty()
    );
    assert_eq!(
        load_params_for_app(&db, "app-b")
            .expect("load b")
            .get("key")
            .map(String::as_str),
        Some("val-b")
    );
}

// i[verify param.store]
#[test]
fn evaluate_script_injects_params_into_stored() {
    let mut params = BTreeMap::new();
    params.insert("hostname".to_owned(), "prod.example.com".to_owned());

    let (app, err) = evaluate_script(
        "test-app",
        r#"let h = app.param("hostname");"#,
        &params,
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none(), "unexpected script error: {err:?}");

    assert!(
        app.def.load().params.contains_key("hostname"),
        "hostname should be in declared params"
    );
    assert_eq!(
        app.stored.lock().get("hostname").map(String::as_str),
        Some("prod.example.com"),
        "stored param value should be accessible via app.stored"
    );
}

// i[verify param.store]
#[test]
fn evaluate_script_absent_param_has_no_stored_value() {
    let params = BTreeMap::new();
    let (app, err) = evaluate_script(
        "test-app",
        r#"let h = app.param("hostname");"#,
        &params,
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none(), "unexpected script error: {err:?}");

    assert!(
        app.def.load().params.contains_key("hostname"),
        "hostname should be recorded as declared"
    );
    assert!(
        app.stored.lock().get("hostname").is_none(),
        "absent param should have no stored value"
    );
}

// Regression test: when a param is declared with kind("password") (implying
// is_secret=true) and its value is stored in secret_params, `load_all_params_for_app`
// must decrypt it into app.stored so scripts can read it back.
#[test]
fn reload_after_secret_param_set_populates_stored() {
    use secrecy::SecretString;

    let db = Db::open_in_memory().expect("open");
    let cipher = crate::runtime::secrets::Cipher::for_tests();

    let script = r#"
        app.param("apikey").kind("password");
    "#;

    secret_params::upsert_secret_param(
        &db,
        &cipher,
        "myapp",
        "apikey",
        &SecretString::new("sekret123".to_owned().into()),
    )
    .expect("upsert secret");

    let loaded = load_all_params_for_app(&db, &cipher, "myapp");
    let (app, err) = evaluate_script("myapp", script, &loaded, &crate::ScriptLimits::default());
    assert!(err.is_none(), "script error: {err:?}");

    assert_eq!(
        app.stored.lock().get("apikey").map(String::as_str),
        Some("sekret123"),
        "secret param must be decrypted into app.stored after reload"
    );
}

// Regression test for the "install says secret param isn't set" bug.
// The OLD `load_params_for_app` only reads the plaintext `params` table,
// so reload paths that used it (script update, registry re-eval) would
// silently drop all secret values, leaving `app.stored` empty.
// All reload paths must use `load_all_params_for_app`.
#[test]
fn load_params_for_app_alone_misses_secrets() {
    use secrecy::SecretString;

    let db = Db::open_in_memory().expect("open");
    let cipher = crate::runtime::secrets::Cipher::for_tests();

    secret_params::upsert_secret_param(
        &db,
        &cipher,
        "myapp",
        "apikey",
        &SecretString::new("sekret".to_owned().into()),
    )
    .expect("upsert secret");

    let plaintext_only = load_params_for_app(&db, "myapp").expect("load");
    assert!(
        plaintext_only.get("apikey").is_none(),
        "plaintext-only loader must not surface secrets (confirms the trap)"
    );

    let merged = load_all_params_for_app(&db, &cipher, "myapp");
    assert_eq!(
        merged.get("apikey").map(String::as_str),
        Some("sekret"),
        "merged loader must surface secrets for reload paths"
    );
}

// i[verify param.store]
#[test]
fn registry_load_from_db_restores_params() {
    let db = Db::open_in_memory().expect("open");

    db.conn
        .execute(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation) \
             VALUES ('myapp', 0, 0, 0)",
            [],
        )
        .expect("insert app");

    crate::runtime::generations::bump_register(&db, "myapp", r#"let h = app.param("hostname");"#)
        .expect("bump register");

    upsert_param(&db, "myapp", "hostname", "restored.example.com").expect("upsert");
    // The handler would also bump generation for the param — for this test
    // (purely about load_from_db), we set the current_generation directly.
    db.conn
        .execute(
            "UPDATE registered_apps SET current_generation = 1 WHERE name = 'myapp'",
            [],
        )
        .expect("set current_generation");

    let cipher = crate::runtime::secrets::Cipher::for_tests();
    let registry = AppRegistry::load_from_db(
        &db,
        &cipher,
        Arc::new(Notify::new()),
        &crate::ScriptLimits::default(),
    )
    .expect("load registry");
    let entry = registry.get("myapp").expect("app should be registered");

    assert!(
        entry.app.def.load().params.contains_key("hostname"),
        "hostname should be in declared params after load"
    );
    assert_eq!(
        entry.app.stored.lock().get("hostname").map(String::as_str),
        Some("restored.example.com"),
        "param value should be restored into app.stored on startup"
    );
}
