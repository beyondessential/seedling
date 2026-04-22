use std::collections::BTreeMap;

use seedling_protocol::names::{AppName, ParamName};

use super::*;
use crate::{ScriptLimits, runtime::db::Db};

fn app() -> AppName {
    AppName::new("myapp").unwrap()
}

fn param(s: &str) -> ParamName {
    ParamName::new_unchecked(s)
}

const SCRIPT_A: &str = r#"app.deployment("web").image("ghcr.io/example/web:1.0");"#;
const SCRIPT_B: &str = r#"app.deployment("web").image("ghcr.io/example/web:2.0");"#;

fn test_db() -> Db {
    let db = Db::open_in_memory().expect("open in-memory db");
    // Tests register apps directly through the generations API; we still need a
    // registered_apps row so current_generation has a place to live.
    db.conn
        .execute(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation)
             VALUES ('myapp', 0, 0, 0)",
            [],
        )
        .expect("insert registered_apps");
    db
}

fn test_cipher() -> crate::runtime::secrets::Cipher {
    crate::runtime::secrets::Cipher::for_tests()
}

#[test]
fn register_bumps_to_one() {
    let db = test_db();
    let g = bump_register(&db, &app(), SCRIPT_A).unwrap();
    assert_eq!(g, 1);
    assert_eq!(current(&db, &app()).unwrap(), Some(1));
}

#[test]
fn script_update_increments_generation_and_dedups_bodies() {
    let db = test_db();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    let g2 = bump_script_update(&db, &app(), SCRIPT_B).unwrap();
    let g3 = bump_script_update(&db, &app(), SCRIPT_A).unwrap();
    assert_eq!(g2, 2);
    assert_eq!(g3, 3);
    let count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM script_bodies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2, "identical script content should dedupe");
}

#[test]
fn param_set_records_previous_value() {
    let db = test_db();
    let cipher = test_cipher();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    bump_param_set(&db, &app(), &param("version"), None, "1.0", &cipher, false).unwrap();
    let g = bump_param_set(
        &db,
        &app(),
        &param("version"),
        Some("1.0"),
        "2.0",
        &cipher,
        false,
    )
    .unwrap();
    let entry = get(&db, &app(), g).unwrap().unwrap();
    assert_eq!(entry.kind, Kind::ParamSet);
    assert_eq!(entry.param_name.as_deref(), Some("version"));
    assert_eq!(entry.previous_value.as_deref(), Some("1.0"));
    assert_eq!(entry.new_value.as_deref(), Some("2.0"));
    assert!(!entry.previous_value_redacted);
    assert!(!entry.new_value_redacted);
}

#[test]
fn param_unset_records_previous_value_and_no_new() {
    let db = test_db();
    let cipher = test_cipher();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    bump_param_set(
        &db,
        &app(),
        &param("domain"),
        None,
        "old.example",
        &cipher,
        false,
    )
    .unwrap();
    let g = bump_param_unset(&db, &app(), &param("domain"), "old.example", &cipher, false).unwrap();
    let entry = get(&db, &app(), g).unwrap().unwrap();
    assert_eq!(entry.kind, Kind::ParamUnset);
    assert_eq!(entry.previous_value.as_deref(), Some("old.example"));
    assert_eq!(entry.new_value, None);
    assert!(!entry.previous_value_redacted);
}

#[test]
fn param_map_at_walks_history() {
    let db = test_db();
    let cipher = test_cipher();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    bump_param_set(&db, &app(), &param("domain"), None, "v1", &cipher, false).unwrap();
    let g_after_v1 = current(&db, &app()).unwrap().unwrap();
    bump_param_set(
        &db,
        &app(),
        &param("domain"),
        Some("v1"),
        "v2",
        &cipher,
        false,
    )
    .unwrap();
    let g_after_v2 = current(&db, &app()).unwrap().unwrap();
    bump_param_set(&db, &app(), &param("other"), None, "x", &cipher, false).unwrap();
    bump_param_unset(&db, &app(), &param("domain"), "v2", &cipher, false).unwrap();
    let g_after_unset = current(&db, &app()).unwrap().unwrap();

    let m1 = param_map_at(&db, &app(), g_after_v1, &cipher).unwrap();
    assert_eq!(m1.get("domain").map(|s| s.as_str()), Some("v1"));

    let m2 = param_map_at(&db, &app(), g_after_v2, &cipher).unwrap();
    assert_eq!(m2.get("domain").map(|s| s.as_str()), Some("v2"));

    let m_now = param_map_at(&db, &app(), g_after_unset, &cipher).unwrap();
    assert!(!m_now.contains_key("domain"));
    assert_eq!(m_now.get("other").map(|s| s.as_str()), Some("x"));
}

#[test]
fn reconstruct_at_prior_generation_uses_old_script_and_params() {
    let db = test_db();
    let cipher = test_cipher();
    let limits = ScriptLimits::default();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    let g_old = current(&db, &app()).unwrap().unwrap();
    bump_script_update(&db, &app(), SCRIPT_B).unwrap();

    let app_old = reconstruct_app_def(&db, &app(), g_old, &limits, &cipher).unwrap();
    let app_new = reconstruct_app_def(
        &db,
        &app(),
        current(&db, &app()).unwrap().unwrap(),
        &limits,
        &cipher,
    )
    .unwrap();

    let old_image = app_old
        .def
        .load()
        .resources
        .iter()
        .find_map(|(_, r)| match r {
            crate::defs::resource::Resource::Deployment(d) => {
                d.def.lock().pod.lock().container.lock().image.clone()
            }
            _ => None,
        });
    let new_image = app_new
        .def
        .load()
        .resources
        .iter()
        .find_map(|(_, r)| match r {
            crate::defs::resource::Resource::Deployment(d) => {
                d.def.lock().pod.lock().container.lock().image.clone()
            }
            _ => None,
        });
    assert!(old_image.unwrap().contains("1.0"));
    assert!(new_image.unwrap().contains("2.0"));
}

#[test]
fn list_returns_descending_with_limit_and_before() {
    let db = test_db();
    let cipher = test_cipher();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    for i in 0..5 {
        bump_param_set(
            &db,
            &app(),
            &param("k"),
            Some(&format!("v{i}")),
            &format!("v{}", i + 1),
            &cipher,
            false,
        )
        .unwrap();
    }
    let all = list(&db, &app(), None, 10).unwrap();
    assert_eq!(all.len(), 6);
    assert!(all.windows(2).all(|w| w[0].generation > w[1].generation));

    let first_three = list(&db, &app(), None, 3).unwrap();
    assert_eq!(first_three.len(), 3);
    assert_eq!(first_three[0].generation, 6);

    let before_3 = list(&db, &app(), Some(3), 10).unwrap();
    assert!(before_3.iter().all(|e| e.generation < 3));
}

#[test]
fn delete_for_app_wipes_history_and_orphan_bodies() {
    let db = test_db();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    bump_script_update(&db, &app(), SCRIPT_B).unwrap();

    db.conn
        .execute(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation)
             VALUES ('other', 0, 0, 0)",
            [],
        )
        .unwrap();
    bump_register(&db, &AppName::new("other").unwrap(), SCRIPT_A).unwrap();

    delete_for_app(&db, &app()).unwrap();

    let myapp_count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM generations WHERE app = 'myapp'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(myapp_count, 0);

    let body_count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM script_bodies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(body_count, 1, "SCRIPT_A is still referenced by 'other'");
}

#[test]
fn attach_operation_and_record_outcome() {
    let db = test_db();
    let cipher = test_cipher();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    let g = bump_param_set(&db, &app(), &param("k"), None, "v", &cipher, false).unwrap();
    attach_operation(&db, &app(), g, "op-123").unwrap();
    let entry = get(&db, &app(), g).unwrap().unwrap();
    assert_eq!(entry.operation_id.as_deref(), Some("op-123"));
    assert_eq!(entry.outcome, Some(Outcome::Pending));

    record_outcome(&db, &app(), g, Outcome::Failed, Some("boom")).unwrap();
    let entry = get(&db, &app(), g).unwrap().unwrap();
    assert_eq!(entry.outcome, Some(Outcome::Failed));
    assert_eq!(entry.outcome_error.as_deref(), Some("boom"));
}

#[test]
fn reconstruct_unknown_generation_returns_not_found() {
    let db = test_db();
    let cipher = test_cipher();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    let limits = ScriptLimits::default();
    let err = reconstruct_app_def(&db, &app(), 99, &limits, &cipher).unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }));
    let _ = BTreeMap::<String, String>::new();
}

// r[verify secret.history]
#[test]
fn secret_param_set_stores_ciphertext_not_plaintext() {
    let db = test_db();
    let cipher = test_cipher();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    let g = bump_param_set(
        &db,
        &app(),
        &param("api_key"),
        None,
        "my-secret-token",
        &cipher,
        true,
    )
    .unwrap();
    let entry = get(&db, &app(), g).unwrap().unwrap();
    assert!(
        entry.new_value_redacted,
        "new value should be flagged redacted"
    );
    assert!(entry.new_value.is_none(), "plaintext should be NULL in DB");
    assert!(entry.previous_value.is_none());
    assert!(!entry.previous_value_redacted);
}

// r[verify secret.history]
#[test]
fn secret_param_unset_stores_ciphertext_not_plaintext() {
    let db = test_db();
    let cipher = test_cipher();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    bump_param_set(
        &db,
        &app(),
        &param("api_key"),
        None,
        "my-secret-token",
        &cipher,
        true,
    )
    .unwrap();
    let g = bump_param_unset(
        &db,
        &app(),
        &param("api_key"),
        "my-secret-token",
        &cipher,
        true,
    )
    .unwrap();
    let entry = get(&db, &app(), g).unwrap().unwrap();
    assert!(
        entry.previous_value_redacted,
        "previous value should be flagged redacted"
    );
    assert!(
        entry.previous_value.is_none(),
        "plaintext should be NULL in DB"
    );
}

// r[verify secret.history]
#[test]
fn param_map_at_decrypts_secret_history() {
    let db = test_db();
    let cipher = test_cipher();
    bump_register(&db, &app(), SCRIPT_A).unwrap();
    bump_param_set(
        &db,
        &app(),
        &param("api_key"),
        None,
        "secret-value",
        &cipher,
        true,
    )
    .unwrap();
    let g = current(&db, &app()).unwrap().unwrap();

    let map = param_map_at(&db, &app(), g, &cipher).unwrap();
    assert_eq!(map.get("api_key").map(String::as_str), Some("secret-value"));
}
