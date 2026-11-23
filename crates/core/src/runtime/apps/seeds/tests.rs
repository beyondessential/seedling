use seedling_protocol::names::AppName;

use super::*;

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

fn register_row(db: &Db, name: &str) {
    db.conn
        .execute(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation) \
             VALUES (?1, 0, 0, 1)",
            [name],
        )
        .unwrap();
}

fn ready(seed: &AppSeed) -> [u8; 32] {
    match seed {
        AppSeed::Ready(seed) => *seed.expose(),
        AppSeed::Unreadable(why) => panic!("unreadable: {why}"),
    }
}

// spec: SEC#app-seeds
#[test]
fn a_stored_seed_loads_back_and_is_encrypted_at_rest() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let seed = Seed::draw().unwrap();
    store_app_seed(&db, &cipher, &app("web"), &seed).unwrap();

    assert_eq!(
        ready(&load_app_seed(&db, &cipher, &app("web")).unwrap()),
        *seed.expose()
    );
    let at_rest: Vec<u8> = db
        .conn
        .query_row("SELECT ciphertext FROM app_seeds", [], |row| row.get(0))
        .unwrap();
    assert!(!at_rest.windows(32).any(|w| w == seed.expose()));
    assert!(crate::runtime::secrets::protected_values_exist(&db).unwrap());
}

// spec: SEC#app-seed-rotation
#[test]
fn storing_again_replaces_the_seed() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    store_app_seed(&db, &cipher, &app("web"), &Seed::draw().unwrap()).unwrap();
    let next = Seed::draw().unwrap();
    store_app_seed(&db, &cipher, &app("web"), &next).unwrap();
    assert_eq!(
        ready(&load_app_seed(&db, &cipher, &app("web")).unwrap()),
        *next.expose()
    );
}

// spec: SEC#app-seeds
#[test]
fn a_missing_or_undecryptable_seed_is_unreadable() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    assert!(matches!(
        load_app_seed(&db, &cipher, &app("web")).unwrap(),
        AppSeed::Unreadable(_)
    ));
    store_app_seed(
        &db,
        &Cipher::for_tests_with(&[3; 32]),
        &app("web"),
        &Seed::draw().unwrap(),
    )
    .unwrap();
    assert!(matches!(
        load_app_seed(&db, &cipher, &app("web")).unwrap(),
        AppSeed::Unreadable(_)
    ));
}

// spec: SEC#app-seeds
#[test]
fn backfill_seeds_unseeded_apps_once_and_keeps_existing_rows() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    register_row(&db, "old");
    register_row(&db, "kept");
    let other_key = Cipher::for_tests_with(&[3; 32]);
    store_app_seed(&db, &other_key, &app("kept"), &Seed::draw().unwrap()).unwrap();

    backfill_app_seeds(&db, &cipher).unwrap();
    let old = ready(&load_app_seed(&db, &cipher, &app("old")).unwrap());
    assert!(
        matches!(
            load_app_seed(&db, &cipher, &app("kept")).unwrap(),
            AppSeed::Unreadable(_)
        ),
        "an existing row, readable or not, is never replaced",
    );

    register_row(&db, "later");
    backfill_app_seeds(&db, &cipher).unwrap();
    assert!(
        matches!(
            load_app_seed(&db, &cipher, &app("later")).unwrap(),
            AppSeed::Unreadable(_)
        ),
        "the backfill runs once",
    );
    assert_eq!(
        ready(&load_app_seed(&db, &cipher, &app("old")).unwrap()),
        old
    );
}

// spec: SEC#app-seeds
#[test]
fn deleting_removes_only_that_apps_seed() {
    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    store_app_seed(&db, &cipher, &app("app-a"), &Seed::draw().unwrap()).unwrap();
    store_app_seed(&db, &cipher, &app("app-b"), &Seed::draw().unwrap()).unwrap();
    delete_app_seed(&db, &app("app-a")).unwrap();
    assert!(matches!(
        load_app_seed(&db, &cipher, &app("app-a")).unwrap(),
        AppSeed::Unreadable(_)
    ));
    ready(&load_app_seed(&db, &cipher, &app("app-b")).unwrap());
}
