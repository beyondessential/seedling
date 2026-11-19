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

/// The old set/unset entry points, as the tests that predate a parameter's
/// two values being protected independently still read.
fn bump_param_set(
    db: &Db,
    app: &AppName,
    name: &ParamName,
    previous: Option<&str>,
    new_value: &str,
    cipher: &Cipher,
    is_secret: bool,
) -> rusqlite::Result<Generation> {
    let change = ParamChange {
        name,
        previous,
        new_value: Some(new_value),
        is_secret,
        previous_is_secret: is_secret,
    };
    bump_param_change(db, app, &[change], cipher)
}

fn bump_param_unset(
    db: &Db,
    app: &AppName,
    name: &ParamName,
    previous: &str,
    cipher: &Cipher,
    is_secret: bool,
) -> rusqlite::Result<Generation> {
    let change = ParamChange {
        name,
        previous: Some(previous),
        new_value: None,
        is_secret,
        previous_is_secret: is_secret,
    };
    bump_param_change(db, app, &[change], cipher)
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

// spec: GEN#generation-numbers
#[test]
fn register_bumps_to_one() {
    let db = test_db();
    let g = register_script(&db, &app(), SCRIPT_A).unwrap();
    assert_eq!(g, 1);
    assert_eq!(current(&db, &app()).unwrap(), Some(1));
}

/// Make the last statement of every bump, moving `current_generation`, fail.
fn fail_current_generation_updates(db: &Db) {
    db.conn
        .execute_batch(
            "CREATE TRIGGER fail_bump BEFORE UPDATE OF current_generation ON registered_apps
             BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
        .expect("create trigger");
}

fn generation_rows(db: &Db) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM generations", [], |r| r.get(0))
        .unwrap()
}

// spec: GEN#generation-numbers
#[test]
fn failed_script_update_leaves_no_generation_behind() {
    let db = test_db();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    fail_current_generation_updates(&db);

    update_script(&db, &app(), SCRIPT_B).expect_err("injected failure");

    assert_eq!(generation_rows(&db), 1);
    assert_eq!(current(&db, &app()).unwrap(), Some(1));
    assert!(db.conn.is_autocommit(), "savepoint left open");
}

// spec: GEN#generation-numbers
#[test]
fn failed_param_bump_leaves_no_generation_behind() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    fail_current_generation_updates(&db);

    bump_param_set(&db, &app(), &param("p"), None, "v", &cipher, false)
        .expect_err("injected failure");

    assert_eq!(generation_rows(&db), 1);
    assert_eq!(current(&db, &app()).unwrap(), Some(1));
    assert!(db.conn.is_autocommit(), "savepoint left open");
}

// spec: GEN#generation-numbers
#[test]
fn bump_inside_a_transaction_commits_with_it() {
    let db = test_db();
    register_script(&db, &app(), SCRIPT_A).unwrap();

    let tx = db.conn.unchecked_transaction().unwrap();
    update_script(&db, &app(), SCRIPT_B).unwrap();
    tx.rollback().unwrap();

    assert_eq!(generation_rows(&db), 1);
    assert_eq!(current(&db, &app()).unwrap(), Some(1));
}

// spec: GEN#generation-numbers
// spec: GEN#bundle-storage
#[test]
fn script_update_increments_generation_and_dedups_bodies() {
    let db = test_db();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    let g2 = update_script(&db, &app(), SCRIPT_B).unwrap();
    let g3 = update_script(&db, &app(), SCRIPT_A).unwrap();
    assert_eq!(g2, 2);
    assert_eq!(g3, 3);
    let count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM definition_bundles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2, "identical script content should dedupe");
}

// spec: GEN#generation-numbers
#[test]
fn param_set_records_previous_value() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
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
    assert_eq!(entry.kind, Kind::ParamChange);
    let [changed] = entry.params.as_slice() else {
        panic!("one parameter changed: {:?}", entry.params);
    };
    assert_eq!(changed.name, "version");
    assert_eq!(changed.previous_value.as_deref(), Some("1.0"));
    assert_eq!(changed.new_value.as_deref(), Some("2.0"));
    assert!(!changed.previous_value_redacted);
    assert!(!changed.new_value_redacted);
}

#[test]
fn param_unset_records_previous_value_and_no_new() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
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
    assert_eq!(entry.kind, Kind::ParamChange);
    let [changed] = entry.params.as_slice() else {
        panic!("one parameter changed: {:?}", entry.params);
    };
    assert_eq!(changed.previous_value.as_deref(), Some("old.example"));
    assert_eq!(changed.new_value, None);
    assert!(!changed.previous_value_redacted);
}

// spec: GEN#history-entries
// spec: GEN#generation-numbers
#[test]
fn a_change_of_several_parameters_is_one_generation() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    let (host, port, mode) = (param("host"), param("port"), param("mode"));
    let g = bump_param_change(
        &db,
        &app(),
        &[
            ParamChange {
                name: &port,
                previous: Some("80"),
                new_value: Some("8080"),
                is_secret: false,
                previous_is_secret: false,
            },
            ParamChange {
                name: &host,
                previous: None,
                new_value: Some("example.test"),
                is_secret: false,
                previous_is_secret: false,
            },
            ParamChange {
                name: &mode,
                previous: Some("old"),
                new_value: None,
                is_secret: false,
                previous_is_secret: false,
            },
        ],
        &cipher,
    )
    .unwrap();

    assert_eq!(g, 2);
    assert_eq!(generation_rows(&db), 2);
    let entry = get(&db, &app(), g).unwrap().unwrap();
    assert_eq!(entry.kind, Kind::ParamChange);
    let seen: Vec<_> = entry
        .params
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.previous_value.as_deref(),
                p.new_value.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            ("host", None, Some("example.test")),
            ("mode", Some("old"), None),
            ("port", Some("80"), Some("8080")),
        ],
        "one entry per parameter, in ascending order of name"
    );
}

// spec: GEN#history-entries
// spec: GEN#reconstruction
#[test]
fn a_definition_update_records_the_parameters_changed_alongside_it() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    let (a, b) = (param("a"), param("b"));
    let g = bump_script_update(
        &db,
        &app(),
        &Bundle::from_stored_script(SCRIPT_B),
        &Source::unknown_push(),
        Some((
            &[
                ParamChange {
                    name: &a,
                    previous: None,
                    new_value: Some("1"),
                    is_secret: false,
                    previous_is_secret: false,
                },
                ParamChange {
                    name: &b,
                    previous: None,
                    new_value: Some("2"),
                    is_secret: true,
                    previous_is_secret: false,
                },
            ],
            &cipher,
        )),
    )
    .unwrap();

    let entry = get(&db, &app(), g).unwrap().unwrap();
    assert_eq!(entry.kind, Kind::ScriptUpdate);
    assert_eq!(entry.params.len(), 2);
    assert!(!entry.params[0].new_value_redacted);
    assert!(entry.params[1].new_value_redacted);
    let map = param_map_at(&db, &app(), g, &cipher).unwrap();
    assert_eq!(
        map,
        BTreeMap::from([
            ("a".to_owned(), "1".to_owned()),
            ("b".to_owned(), "2".to_owned())
        ])
    );
}

// spec: GEN#reconstruction
#[test]
fn param_map_at_takes_each_parameter_from_its_own_latest_change() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    let (a, b) = (param("a"), param("b"));
    let first = bump_param_change(
        &db,
        &app(),
        &[
            ParamChange {
                name: &a,
                previous: None,
                new_value: Some("a1"),
                is_secret: false,
                previous_is_secret: false,
            },
            ParamChange {
                name: &b,
                previous: None,
                new_value: Some("b1"),
                is_secret: false,
                previous_is_secret: false,
            },
        ],
        &cipher,
    )
    .unwrap();
    let second = bump_param_change(
        &db,
        &app(),
        &[
            ParamChange {
                name: &a,
                previous: Some("a1"),
                new_value: Some("a2"),
                is_secret: false,
                previous_is_secret: false,
            },
            ParamChange {
                name: &b,
                previous: Some("b1"),
                new_value: None,
                is_secret: false,
                previous_is_secret: false,
            },
        ],
        &cipher,
    )
    .unwrap();

    let at_first = param_map_at(&db, &app(), first, &cipher).unwrap();
    assert_eq!(at_first.get("a").map(String::as_str), Some("a1"));
    assert_eq!(at_first.get("b").map(String::as_str), Some("b1"));
    let at_second = param_map_at(&db, &app(), second, &cipher).unwrap();
    assert_eq!(at_second.get("a").map(String::as_str), Some("a2"));
    assert!(!at_second.contains_key("b"));
}

// spec: GEN#generation-numbers
#[test]
fn a_failed_multi_parameter_bump_records_none_of_its_parameters() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    let (a, b) = (param("a"), param("b"));
    db.conn
        .execute_batch(
            "CREATE TRIGGER fail_second_param BEFORE INSERT ON generation_params
             WHEN NEW.param_name = 'b'
             BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();

    bump_param_change(
        &db,
        &app(),
        &[
            ParamChange {
                name: &a,
                previous: None,
                new_value: Some("1"),
                is_secret: false,
                previous_is_secret: false,
            },
            ParamChange {
                name: &b,
                previous: None,
                new_value: Some("2"),
                is_secret: false,
                previous_is_secret: false,
            },
        ],
        &cipher,
    )
    .expect_err("injected failure");

    assert_eq!(generation_rows(&db), 1);
    let rows: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM generation_params", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(current(&db, &app()).unwrap(), Some(1));
}

// spec: GEN#reconstruction
#[test]
fn param_map_at_walks_history() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
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

// spec: GEN#reconstruction
#[test]
fn reconstruct_at_prior_generation_uses_old_script_and_params() {
    let db = test_db();
    let cipher = test_cipher();
    let limits = ScriptLimits::default();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    let g_old = current(&db, &app()).unwrap().unwrap();
    update_script(&db, &app(), SCRIPT_B).unwrap();

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

// spec: GEN#history-entries
#[test]
fn list_returns_descending_with_limit_and_before() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
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

// spec: GEN#history-entries
// spec: SEC#generation-history
#[test]
fn a_listing_carries_each_entrys_own_parameters() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    let first = bump_param_change(
        &db,
        &app(),
        &[
            ParamChange {
                name: &param("b"),
                previous: None,
                new_value: Some("b1"),
                is_secret: false,
                previous_is_secret: false,
            },
            ParamChange {
                name: &param("a"),
                previous: None,
                new_value: Some("a1"),
                is_secret: false,
                previous_is_secret: false,
            },
        ],
        &cipher,
    )
    .unwrap();
    let second = bump_param_set(&db, &app(), &param("token"), None, "t1", &cipher, true).unwrap();
    let third = bump_param_set(&db, &app(), &param("a"), Some("a1"), "a2", &cipher, false).unwrap();

    let listed = list(&db, &app(), None, 10).unwrap();
    let changed: Vec<_> = listed
        .iter()
        .map(|entry| {
            (
                entry.generation,
                entry
                    .params
                    .iter()
                    .map(|p| {
                        (
                            p.name.as_str(),
                            p.new_value.as_deref(),
                            p.new_value_redacted,
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        changed,
        vec![
            (third, vec![("a", Some("a2"), false)]),
            (second, vec![("token", None, true)]),
            (
                first,
                vec![("a", Some("a1"), false), ("b", Some("b1"), false)]
            ),
            (1, vec![]),
        ]
    );
}

// spec: GEN#lineage
// spec: GEN#bundle-storage
#[test]
fn delete_for_app_wipes_history_and_orphan_bodies() {
    let db = test_db();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    update_script(&db, &app(), SCRIPT_B).unwrap();

    db.conn
        .execute(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation)
             VALUES ('other', 0, 0, 0)",
            [],
        )
        .unwrap();
    register_script(&db, &AppName::new("other").unwrap(), SCRIPT_A).unwrap();
    bump_param_set(&db, &app(), &param("p"), None, "v", &test_cipher(), false).unwrap();

    delete_for_app(&db, &app()).unwrap();

    let param_rows: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM generation_params", [], |r| r.get(0))
        .unwrap();
    assert_eq!(param_rows, 0, "the parameter changes go with their history");

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
        .query_row("SELECT COUNT(*) FROM definition_bundles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(body_count, 1, "SCRIPT_A is still referenced by 'other'");
}

// spec: GEN#reconstruction
#[test]
fn reconstruct_unknown_generation_returns_not_found() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    let limits = ScriptLimits::default();
    let err = reconstruct_app_def(&db, &app(), 99, &limits, &cipher).unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }));
    let _ = BTreeMap::<String, String>::new();
}

// spec: SEC#generation-history
#[test]
fn secret_param_set_stores_ciphertext_not_plaintext() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
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
    let changed = &entry.params[0];
    assert!(
        changed.new_value_redacted,
        "new value should be flagged redacted"
    );
    assert!(
        changed.new_value.is_none(),
        "plaintext should be NULL in DB"
    );
    assert!(changed.previous_value.is_none());
    assert!(!changed.previous_value_redacted);
}

// spec: SEC#generation-history
#[test]
fn secret_param_unset_stores_ciphertext_not_plaintext() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
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
    let changed = &entry.params[0];
    assert!(
        changed.previous_value_redacted,
        "previous value should be flagged redacted"
    );
    assert!(
        changed.previous_value.is_none(),
        "plaintext should be NULL in DB"
    );
}

// spec: SEC#generation-history
#[test]
fn param_map_at_decrypts_secret_history() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
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

// spec: SEC#generation-history
#[test]
fn a_value_that_was_secret_stays_protected_when_the_flag_is_dropped() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
    let name = param("api_key");
    // The definition stops marking the parameter secret in the same update
    // that changes its value, so the two values are held differently.
    let change = ParamChange {
        name: &name,
        previous: Some("old-secret"),
        new_value: Some("now-public"),
        is_secret: false,
        previous_is_secret: true,
    };
    let g = bump_param_change(&db, &app(), &[change], &cipher).unwrap();
    let entry = get(&db, &app(), g).unwrap().unwrap();
    let changed = &entry.params[0];
    assert!(changed.previous_value_redacted);
    assert_eq!(changed.previous_value, None);
    assert_eq!(changed.new_value.as_deref(), Some("now-public"));
    assert!(!changed.new_value_redacted);

    let plaintext: Vec<Option<String>> = db
        .conn
        .prepare("SELECT previous_value FROM generation_params WHERE app = ?1 AND generation = ?2")
        .unwrap()
        .query_map(rusqlite::params![app(), g as i64], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        plaintext,
        vec![None],
        "the old secret must not be in the clear"
    );

    // Walking history still reconstructs it.
    let map = param_map_at(&db, &app(), g, &cipher).unwrap();
    assert_eq!(map.get("api_key").map(String::as_str), Some("now-public"));
}

// spec: GEN#reconstruction
#[test]
fn an_unreadable_secret_value_fails_reconstruction_rather_than_reading_as_unset() {
    let db = test_db();
    let cipher = test_cipher();
    register_script(&db, &app(), SCRIPT_A).unwrap();
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
    db.conn
        .execute(
            "UPDATE generation_params SET new_value_ciphertext = x'00010203' WHERE generation = ?1",
            rusqlite::params![g as i64],
        )
        .unwrap();

    let err = param_map_at(&db, &app(), g, &cipher).unwrap_err();
    assert!(matches!(err, Error::UnreadableParam { .. }), "{err}");
}

// spec: GEN#history-entries
#[test]
fn an_entry_whose_kind_or_provenance_cannot_be_read_fails_the_listing() {
    for (column, value) in [("kind", "mystery"), ("provenance", "not json")] {
        let db = test_db();
        register_script(&db, &app(), SCRIPT_A).unwrap();
        db.conn
            .execute(
                &format!("UPDATE generations SET {column} = ?1 WHERE app = ?2"),
                [value, app().as_str()],
            )
            .unwrap();
        assert!(list(&db, &app(), None, 10).is_err(), "{column}");
    }
}
