use secrecy::SecretString;
use seedling_protocol::names::{AppName, ParamName};

use super::*;
use crate::runtime::apps::upsert_param;
use crate::runtime::db::Db;

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

fn param(s: &str) -> ParamName {
    ParamName::new_unchecked(s)
}

fn secret(s: &str) -> SecretString {
    SecretString::new(s.to_owned().into())
}

// r[verify secret.storage]
#[test]
fn upsert_and_load_round_trip_with_ciphertext_at_rest() {
    let db = Db::open_in_memory().expect("open");
    let cipher = Cipher::for_tests();

    upsert_secret_param(&db, &cipher, &app("myapp"), &param("apikey"), &secret("hunter2"))
        .expect("upsert");

    let loaded = load_secret_params_for_app(&db, &cipher, &app("myapp")).expect("load");
    assert_eq!(loaded.get("apikey").map(String::as_str), Some("hunter2"));

    let ciphertext: Vec<u8> = db
        .conn
        .query_row(
            "SELECT ciphertext FROM secret_params WHERE app_name = 'myapp' AND param_name = 'apikey'",
            [],
            |row| row.get(0),
        )
        .expect("raw row");
    assert!(
        !ciphertext
            .windows(b"hunter2".len())
            .any(|w| w == b"hunter2"),
        "plaintext must not appear in the stored ciphertext"
    );
}

// r[verify secret.storage]
#[test]
fn upsert_replaces_existing_value() {
    let db = Db::open_in_memory().expect("open");
    let cipher = Cipher::for_tests();

    upsert_secret_param(&db, &cipher, &app("myapp"), &param("apikey"), &secret("old"))
        .expect("first upsert");
    upsert_secret_param(&db, &cipher, &app("myapp"), &param("apikey"), &secret("new"))
        .expect("second upsert");

    let loaded = load_secret_params_for_app(&db, &cipher, &app("myapp")).expect("load");
    assert_eq!(loaded.get("apikey").map(String::as_str), Some("new"));
    assert_eq!(loaded.len(), 1);
}

#[test]
fn delete_one_removes_only_that_param() {
    let db = Db::open_in_memory().expect("open");
    let cipher = Cipher::for_tests();

    upsert_secret_param(&db, &cipher, &app("myapp"), &param("apikey"), &secret("a"))
        .expect("upsert");
    upsert_secret_param(&db, &cipher, &app("myapp"), &param("token"), &secret("b"))
        .expect("upsert");

    delete_one_secret_param(&db, &app("myapp"), &param("apikey")).expect("delete");

    let loaded = load_secret_params_for_app(&db, &cipher, &app("myapp")).expect("load");
    assert!(!loaded.contains_key("apikey"));
    assert_eq!(loaded.get("token").map(String::as_str), Some("b"));
}

#[test]
fn delete_app_secret_params_is_scoped_to_app() {
    let db = Db::open_in_memory().expect("open");
    let cipher = Cipher::for_tests();

    upsert_secret_param(&db, &cipher, &app("app-a"), &param("apikey"), &secret("a"))
        .expect("upsert a");
    upsert_secret_param(&db, &cipher, &app("app-b"), &param("apikey"), &secret("b"))
        .expect("upsert b");

    delete_app_secret_params(&db, &app("app-a")).expect("delete");

    assert!(
        load_secret_params_for_app(&db, &cipher, &app("app-a"))
            .expect("load a")
            .is_empty()
    );
    assert_eq!(
        load_secret_params_for_app(&db, &cipher, &app("app-b"))
            .expect("load b")
            .len(),
        1
    );
}

// r[verify secret.migration]
#[test]
fn migrate_to_secret_moves_plaintext_row_into_secret_storage() {
    let db = Db::open_in_memory().expect("open");
    let cipher = Cipher::for_tests();

    upsert_param(&db, &app("myapp"), &param("apikey"), "was-plaintext").expect("upsert plaintext");

    migrate_to_secret(&db, &cipher, &app("myapp"), &param("apikey")).expect("migrate");

    let plaintext_rows: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM params WHERE app_name = 'myapp' AND param_name = 'apikey'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(plaintext_rows, 0, "plaintext row must be deleted");

    let loaded = load_secret_params_for_app(&db, &cipher, &app("myapp")).expect("load");
    assert_eq!(
        loaded.get("apikey").map(String::as_str),
        Some("was-plaintext")
    );
}

// r[verify secret.migration]
#[test]
fn migrate_to_secret_is_a_noop_when_no_plaintext_exists() {
    let db = Db::open_in_memory().expect("open");
    let cipher = Cipher::for_tests();

    migrate_to_secret(&db, &cipher, &app("myapp"), &param("apikey")).expect("migrate");

    assert!(
        load_secret_params_for_app(&db, &cipher, &app("myapp"))
            .expect("load")
            .is_empty()
    );
}
