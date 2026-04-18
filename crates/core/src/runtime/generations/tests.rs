use std::collections::BTreeMap;

use super::*;
use crate::{ScriptLimits, runtime::db::Db};

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

#[test]
fn register_bumps_to_one() {
    let db = test_db();
    let g = bump_register(&db, "myapp", SCRIPT_A).unwrap();
    assert_eq!(g, 1);
    assert_eq!(current(&db, "myapp").unwrap(), Some(1));
}

#[test]
fn script_update_increments_generation_and_dedups_bodies() {
    let db = test_db();
    bump_register(&db, "myapp", SCRIPT_A).unwrap();
    let g2 = bump_script_update(&db, "myapp", SCRIPT_B).unwrap();
    let g3 = bump_script_update(&db, "myapp", SCRIPT_A).unwrap();
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
    bump_register(&db, "myapp", SCRIPT_A).unwrap();
    bump_param_set(&db, "myapp", "version", None, "1.0").unwrap();
    let g = bump_param_set(&db, "myapp", "version", Some("1.0"), "2.0").unwrap();
    let entry = get(&db, "myapp", g).unwrap().unwrap();
    assert_eq!(entry.kind, Kind::ParamSet);
    assert_eq!(entry.param_name.as_deref(), Some("version"));
    assert_eq!(entry.previous_value.as_deref(), Some("1.0"));
    assert_eq!(entry.new_value.as_deref(), Some("2.0"));
}

#[test]
fn param_unset_records_previous_value_and_no_new() {
    let db = test_db();
    bump_register(&db, "myapp", SCRIPT_A).unwrap();
    bump_param_set(&db, "myapp", "domain", None, "old.example").unwrap();
    let g = bump_param_unset(&db, "myapp", "domain", "old.example").unwrap();
    let entry = get(&db, "myapp", g).unwrap().unwrap();
    assert_eq!(entry.kind, Kind::ParamUnset);
    assert_eq!(entry.previous_value.as_deref(), Some("old.example"));
    assert_eq!(entry.new_value, None);
}

#[test]
fn param_map_at_walks_history() {
    let db = test_db();
    bump_register(&db, "myapp", SCRIPT_A).unwrap();
    bump_param_set(&db, "myapp", "domain", None, "v1").unwrap();
    let g_after_v1 = current(&db, "myapp").unwrap().unwrap();
    bump_param_set(&db, "myapp", "domain", Some("v1"), "v2").unwrap();
    let g_after_v2 = current(&db, "myapp").unwrap().unwrap();
    bump_param_set(&db, "myapp", "other", None, "x").unwrap();
    bump_param_unset(&db, "myapp", "domain", "v2").unwrap();
    let g_after_unset = current(&db, "myapp").unwrap().unwrap();

    let m1 = param_map_at(&db, "myapp", g_after_v1).unwrap();
    assert_eq!(m1.get("domain").map(|s| s.as_str()), Some("v1"));

    let m2 = param_map_at(&db, "myapp", g_after_v2).unwrap();
    assert_eq!(m2.get("domain").map(|s| s.as_str()), Some("v2"));

    let m_now = param_map_at(&db, "myapp", g_after_unset).unwrap();
    assert!(!m_now.contains_key("domain"));
    assert_eq!(m_now.get("other").map(|s| s.as_str()), Some("x"));
}

#[test]
fn reconstruct_at_prior_generation_uses_old_script_and_params() {
    let db = test_db();
    let limits = ScriptLimits::default();
    bump_register(&db, "myapp", SCRIPT_A).unwrap();
    let g_old = current(&db, "myapp").unwrap().unwrap();
    bump_script_update(&db, "myapp", SCRIPT_B).unwrap();

    let app_old = reconstruct_app_def(&db, "myapp", g_old, &limits).unwrap();
    let app_new = reconstruct_app_def(
        &db,
        "myapp",
        current(&db, "myapp").unwrap().unwrap(),
        &limits,
    )
    .unwrap();

    let old_image = app_old
        .def
        .lock()
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
        .lock()
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
    bump_register(&db, "myapp", SCRIPT_A).unwrap();
    for i in 0..5 {
        bump_param_set(
            &db,
            "myapp",
            "k",
            Some(&format!("v{i}")),
            &format!("v{}", i + 1),
        )
        .unwrap();
    }
    let all = list(&db, "myapp", None, 10).unwrap();
    assert_eq!(all.len(), 6);
    assert!(all.windows(2).all(|w| w[0].generation > w[1].generation));

    let first_three = list(&db, "myapp", None, 3).unwrap();
    assert_eq!(first_three.len(), 3);
    assert_eq!(first_three[0].generation, 6);

    let before_3 = list(&db, "myapp", Some(3), 10).unwrap();
    assert!(before_3.iter().all(|e| e.generation < 3));
}

#[test]
fn delete_for_app_wipes_history_and_orphan_bodies() {
    let db = test_db();
    bump_register(&db, "myapp", SCRIPT_A).unwrap();
    bump_script_update(&db, "myapp", SCRIPT_B).unwrap();

    db.conn
        .execute(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation)
             VALUES ('other', 0, 0, 0)",
            [],
        )
        .unwrap();
    bump_register(&db, "other", SCRIPT_A).unwrap();

    delete_for_app(&db, "myapp").unwrap();

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
    bump_register(&db, "myapp", SCRIPT_A).unwrap();
    let g = bump_param_set(&db, "myapp", "k", None, "v").unwrap();
    attach_operation(&db, "myapp", g, "op-123").unwrap();
    let entry = get(&db, "myapp", g).unwrap().unwrap();
    assert_eq!(entry.operation_id.as_deref(), Some("op-123"));
    assert_eq!(entry.outcome, Some(Outcome::Pending));

    record_outcome(&db, "myapp", g, Outcome::Failed, Some("boom")).unwrap();
    let entry = get(&db, "myapp", g).unwrap().unwrap();
    assert_eq!(entry.outcome, Some(Outcome::Failed));
    assert_eq!(entry.outcome_error.as_deref(), Some("boom"));
}

#[test]
fn reconstruct_unknown_generation_returns_not_found() {
    let db = test_db();
    bump_register(&db, "myapp", SCRIPT_A).unwrap();
    let limits = ScriptLimits::default();
    let err = reconstruct_app_def(&db, "myapp", 99, &limits).unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }));
    let _ = BTreeMap::<String, String>::new();
}
