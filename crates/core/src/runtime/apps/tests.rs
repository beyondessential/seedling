use std::collections::BTreeMap;
use std::sync::Arc;

use seedling_protocol::names::{AppName, ParamName};
use tokio::sync::Notify;

use super::params::load_params_for_app;
use super::*;
use crate::runtime::db::Db;

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

fn param(s: &str) -> ParamName {
    ParamName::new_unchecked(s)
}

// i[verify app.status] i[verify action.invoke.install]
#[test]
fn phase_encode_round_trip() {
    use super::{decode_phase, encode_phase};
    for phase in [
        AppPhase::NotInstalled,
        AppPhase::Installing,
        AppPhase::Installed,
        AppPhase::Uninstalling,
    ] {
        let (installed, uninstalling, installing) = encode_phase(&phase);
        let decoded = decode_phase("test-app", installed, uninstalling, installing);
        assert_eq!(decoded, phase, "round-trip for {phase:?}");
    }
}

// i[verify app.status]
#[test]
fn decode_phase_prefers_uninstalling_over_conflicts() {
    use super::decode_phase;
    // All bits set: uninstalling wins because teardown is the highest-
    // priority in-flight state.
    assert_eq!(
        decode_phase("test-app", true, true, true),
        AppPhase::Uninstalling,
    );
}

// i[verify param.store]
#[test]
fn upsert_and_load_params_round_trip() {
    let db = Db::open_in_memory().expect("open");
    upsert_param(&db, &app("myapp"), &param("host"), "example.com").expect("upsert");
    upsert_param(&db, &app("myapp"), &param("port"), "8080").expect("upsert");

    let params = load_params_for_app(&db, &app("myapp")).expect("load");
    assert_eq!(params.get("host").map(String::as_str), Some("example.com"));
    assert_eq!(params.get("port").map(String::as_str), Some("8080"));
}

// i[verify param.store]
#[test]
fn upsert_param_replaces_existing_value() {
    let db = Db::open_in_memory().expect("open");
    upsert_param(&db, &app("myapp"), &param("host"), "old.example.com").expect("first upsert");
    upsert_param(&db, &app("myapp"), &param("host"), "new.example.com").expect("second upsert");

    let params = load_params_for_app(&db, &app("myapp")).expect("load");
    assert_eq!(
        params.get("host").map(String::as_str),
        Some("new.example.com")
    );
    assert_eq!(params.len(), 1);
}

fn make_entry(name: &str, script_error: Option<&str>) -> AppEntry {
    AppEntry {
        name: app(name),
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
        crate::runtime::faults::list_active_faults(&db, Some(&app("myapp")))
            .unwrap()
            .len(),
        1
    );

    let entry = make_entry("myapp", None);
    sync_script_error_fault(&db, &entry);
    assert!(
        crate::runtime::faults::list_active_faults(&db, Some(&app("myapp")))
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
    let faults = crate::runtime::faults::list_active_faults(&db, Some(&app("myapp"))).unwrap();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].description, "has_value not found");
    let old_id = faults[0].id.clone();

    let entry = make_entry("myapp", Some("different error"));
    sync_script_error_fault(&db, &entry);
    let faults = crate::runtime::faults::list_active_faults(&db, Some(&app("myapp"))).unwrap();
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
    let first = crate::runtime::faults::list_active_faults(&db, Some(&app("myapp"))).unwrap();
    assert_eq!(first.len(), 1);
    let first_id = first[0].id.clone();

    sync_script_error_fault(&db, &entry);
    let second = crate::runtime::faults::list_active_faults(&db, Some(&app("myapp"))).unwrap();
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
    upsert_param(&db, &app("app-a"), &param("key"), "val-a").expect("upsert a");
    upsert_param(&db, &app("app-b"), &param("key"), "val-b").expect("upsert b");

    let params_a = load_params_for_app(&db, &app("app-a")).expect("load a");
    let params_b = load_params_for_app(&db, &app("app-b")).expect("load b");

    assert_eq!(params_a.get("key").map(String::as_str), Some("val-a"));
    assert_eq!(params_b.get("key").map(String::as_str), Some("val-b"));
}

// i[verify param.store]
#[test]
fn load_params_returns_empty_for_unknown_app() {
    let db = Db::open_in_memory().expect("open");
    let params = load_params_for_app(&db, &app("nonexistent")).expect("load");
    assert!(params.is_empty());
}

// i[verify param.store]
#[test]
fn delete_app_params_removes_only_that_apps_params() {
    let db = Db::open_in_memory().expect("open");
    upsert_param(&db, &app("app-a"), &param("key"), "val-a").expect("upsert a");
    upsert_param(&db, &app("app-b"), &param("key"), "val-b").expect("upsert b");

    delete_app_params(&db, &app("app-a")).expect("delete");

    assert!(
        load_params_for_app(&db, &app("app-a"))
            .expect("load a")
            .is_empty()
    );
    assert_eq!(
        load_params_for_app(&db, &app("app-b"))
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

    let (app_def, err) = evaluate_script(
        &app("test-app"),
        r#"let h = app.param("hostname");"#,
        &params,
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none(), "unexpected script error: {err:?}");

    assert!(
        app_def.def.load().params.contains_key("hostname"),
        "hostname should be in declared params"
    );
    assert_eq!(
        app_def.stored.lock().get("hostname").map(String::as_str),
        Some("prod.example.com"),
        "stored param value should be accessible via app.stored"
    );
}

// i[verify param.store]
#[test]
fn evaluate_script_absent_param_has_no_stored_value() {
    let params = BTreeMap::new();
    let (app_def, err) = evaluate_script(
        &app("test-app"),
        r#"let h = app.param("hostname");"#,
        &params,
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none(), "unexpected script error: {err:?}");

    assert!(
        app_def.def.load().params.contains_key("hostname"),
        "hostname should be recorded as declared"
    );
    assert!(
        app_def.stored.lock().get("hostname").is_none(),
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
        &app("myapp"),
        &param("apikey"),
        &SecretString::new("sekret123".to_owned().into()),
    )
    .expect("upsert secret");

    let loaded = load_all_params_for_app(&db, &cipher, &app("myapp"));
    let (app_def, err) = evaluate_script(
        &app("myapp"),
        script,
        &loaded,
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none(), "script error: {err:?}");

    assert_eq!(
        app_def.stored.lock().get("apikey").map(String::as_str),
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
        &app("myapp"),
        &param("apikey"),
        &SecretString::new("sekret".to_owned().into()),
    )
    .expect("upsert secret");

    let plaintext_only = load_params_for_app(&db, &app("myapp")).expect("load");
    assert!(
        !plaintext_only.contains_key("apikey"),
        "plaintext-only loader must not surface secrets (confirms the trap)"
    );

    let merged = load_all_params_for_app(&db, &cipher, &app("myapp"));
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

    crate::runtime::generations::bump_register(
        &db,
        &app("myapp"),
        r#"let h = app.param("hostname");"#,
    )
    .expect("bump register");

    upsert_param(
        &db,
        &app("myapp"),
        &param("hostname"),
        "restored.example.com",
    )
    .expect("upsert");
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

// -----------------------------------------------------------------------
// AppRegistry — in-memory CRUD
// -----------------------------------------------------------------------

fn trivial_script() -> &'static str {
    r#"app.deployment("web").image("docker.io/library/nginx:latest");"#
}

// i[verify app.register]
#[test]
fn register_adds_entry_and_makes_it_discoverable() {
    let mut reg = AppRegistry::new();
    let notify = Arc::new(Notify::new());
    reg.register(
        app("myapp"),
        trivial_script().to_owned(),
        Arc::clone(&notify),
        &crate::ScriptLimits::default(),
    )
    .unwrap();
    assert!(reg.is_registered("myapp"));
    assert!(reg.get("myapp").is_some());
}

// i[verify app.register]
#[test]
fn register_persists_script_verbatim_for_later_replay() {
    let mut reg = AppRegistry::new();
    let notify = Arc::new(Notify::new());
    reg.register(
        app("myapp"),
        trivial_script().to_owned(),
        notify,
        &crate::ScriptLimits::default(),
    )
    .unwrap();
    let entry = reg.get("myapp").unwrap();
    assert_eq!(entry.script, trivial_script());
    assert_eq!(entry.current_generation, 0);
}

// i[verify app.deregister]
#[test]
fn deregister_removes_entry() {
    let mut reg = AppRegistry::new();
    let notify = Arc::new(Notify::new());
    reg.register(
        app("myapp"),
        trivial_script().to_owned(),
        notify,
        &crate::ScriptLimits::default(),
    )
    .unwrap();
    assert!(reg.deregister("myapp"));
    assert!(!reg.is_registered("myapp"));
    assert!(reg.get("myapp").is_none());
}

// i[verify app.deregister]
#[test]
fn deregister_unknown_returns_false() {
    let mut reg = AppRegistry::new();
    assert!(!reg.deregister("nonexistent"));
}

// i[verify app.list]
#[test]
fn list_returns_registered_apps_sorted() {
    let mut reg = AppRegistry::new();
    let notify = Arc::new(Notify::new());
    for name in ["zeta-app", "alpha-app", "mu-app"] {
        reg.register(
            app(name),
            trivial_script().to_owned(),
            Arc::clone(&notify),
            &crate::ScriptLimits::default(),
        )
        .unwrap();
    }
    let names: Vec<_> = reg.list().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, vec!["alpha-app", "mu-app", "zeta-app"]);
}

// i[verify app.update]
#[test]
fn reload_replaces_script_on_existing_entry() {
    let mut reg = AppRegistry::new();
    let notify = Arc::new(Notify::new());
    reg.register(
        app("myapp"),
        trivial_script().to_owned(),
        notify,
        &crate::ScriptLimits::default(),
    )
    .unwrap();

    let new_script = r#"app.deployment("api").image("ghcr.io/acme/api:1.0");"#;
    reg.reload(
        &app("myapp"),
        new_script.to_owned(),
        &BTreeMap::new(),
        &crate::ScriptLimits::default(),
    );
    let entry = reg.get("myapp").unwrap();
    assert_eq!(entry.script, new_script);
    assert!(entry.script_error.is_none());
}

// -----------------------------------------------------------------------
// Persistence — registered_apps round-trips
// -----------------------------------------------------------------------

// i[verify app.persist]
#[test]
fn persist_and_load_round_trips_phase_and_generation() {
    let db = Db::open_in_memory().expect("open");
    let generation =
        generations::bump_register(&db, &app("myapp"), trivial_script()).expect("bump register");

    let mut entry = make_entry("myapp", None);
    entry.script = trivial_script().to_owned();
    entry.current_generation = generation;
    *entry.phase.lock() = AppPhase::Installed;
    AppRegistry::persist_app(&db, &entry).expect("persist");

    let cipher = crate::runtime::secrets::Cipher::for_tests();
    let registry = AppRegistry::load_from_db(
        &db,
        &cipher,
        Arc::new(Notify::new()),
        &crate::ScriptLimits::default(),
    )
    .expect("load registry");

    let loaded = registry.get("myapp").expect("app present after reload");
    assert_eq!(*loaded.phase.lock(), AppPhase::Installed);
    assert_eq!(loaded.current_generation, generation);
    assert_eq!(loaded.script, trivial_script());
}

// i[verify app.persist]
#[test]
fn load_from_db_skips_apps_without_a_generation() {
    let db = Db::open_in_memory().expect("open");
    db.conn
        .execute(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation) \
             VALUES ('broken', 0, 0, 0)",
            [],
        )
        .expect("insert app");

    let cipher = crate::runtime::secrets::Cipher::for_tests();
    let registry = AppRegistry::load_from_db(
        &db,
        &cipher,
        Arc::new(Notify::new()),
        &crate::ScriptLimits::default(),
    )
    .expect("load registry");
    assert!(!registry.is_registered("broken"));
}

// i[verify app.deregister]
#[test]
fn remove_app_deletes_persisted_row() {
    let db = Db::open_in_memory().expect("open");
    let generation =
        generations::bump_register(&db, &app("myapp"), trivial_script()).expect("bump register");
    let mut entry = make_entry("myapp", None);
    entry.current_generation = generation;
    AppRegistry::persist_app(&db, &entry).expect("persist");

    AppRegistry::remove_app(&db, &app("myapp")).expect("remove");

    let count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM registered_apps WHERE name = 'myapp'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(count, 0);
}

// i[verify app.status]
#[test]
fn transition_phase_updates_shared_state_and_db() {
    let db = Db::open_in_memory().expect("open");
    let entry = make_entry("myapp", None);
    AppRegistry::persist_app(&db, &entry).expect("persist");

    transition_phase(&entry.phase, AppPhase::Installing, &db, &app("myapp"), "");

    assert_eq!(*entry.phase.lock(), AppPhase::Installing);
    let (installed, uninstalling, installing): (bool, bool, bool) = db
        .conn
        .query_row(
            "SELECT installed, uninstalling, installing FROM registered_apps WHERE name = 'myapp'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("row");
    assert_eq!((installed, uninstalling, installing), (false, false, true));
}

// i[verify app.generation]
#[test]
fn script_retrieval_tracks_generation_bumps() {
    let db = Db::open_in_memory().expect("open");
    db.conn
        .execute(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation) \
             VALUES ('myapp', 0, 0, 0)",
            [],
        )
        .expect("insert app");
    let old_script = r#"app.deployment("web").image("docker.io/library/nginx:1.25");"#;
    let new_script = r#"app.deployment("web").image("docker.io/library/nginx:1.26");"#;

    let first = generations::bump_register(&db, &app("myapp"), old_script).expect("bump 1");
    let second = generations::bump_script_update(&db, &app("myapp"), new_script).expect("bump 2");
    assert!(second > first, "generations are monotonic");

    assert_eq!(
        get_script_at_generation(&db, &app("myapp"), first)
            .expect("get old")
            .as_deref(),
        Some(old_script)
    );
    let (current_gen, current_script) = get_current_script(&db, &app("myapp"))
        .expect("get current")
        .expect("current script present");
    assert_eq!(current_gen, second);
    assert_eq!(current_script, new_script);
}

// i[verify app.generation]
#[test]
fn script_retrieval_returns_none_for_unknown_app() {
    let db = Db::open_in_memory().expect("open");
    assert!(
        get_current_script(&db, &app("ghost"))
            .expect("get current")
            .is_none()
    );
    assert!(
        get_script_at_generation(&db, &app("ghost"), 1)
            .expect("get at generation")
            .is_none()
    );
}

// r[verify generation.previous]
#[test]
fn script_at_later_generation_resolves_to_most_recent_script() {
    // Param-set bumps do not change the script, so the script "at" a later
    // generation is the most recent Register/ScriptUpdate at or before it.
    let db = Db::open_in_memory().expect("open");
    generations::bump_register(&db, &app("myapp"), trivial_script()).expect("bump");
    assert_eq!(
        get_script_at_generation(&db, &app("myapp"), 5)
            .expect("get")
            .as_deref(),
        Some(trivial_script())
    );
}

// i[verify app.update]
#[test]
fn reload_of_unknown_app_is_noop() {
    let mut reg = AppRegistry::new();
    // No panic, no registration.
    reg.reload(
        &app("ghost"),
        trivial_script().to_owned(),
        &BTreeMap::new(),
        &crate::ScriptLimits::default(),
    );
    assert!(!reg.is_registered("ghost"));
}
