use super::*;
use crate::runtime::db::Db;

const START: i64 = 1_000_000;
const PERIOD: i64 = ABSENT_AFTER_SECS;

fn app(n: &str) -> AppName {
    AppName::new_unchecked(n)
}

fn held(db: &Db) -> Vec<String> {
    faults::list_active_faults(db, None)
        .unwrap()
        .into_iter()
        .filter(|f| f.kind == FAULT_KIND)
        .map(|f| f.app.to_string())
        .collect()
}

/// A db with a typed app that came to rely on Canopy at `since`.
fn db_with_app(since: i64) -> Db {
    let db = Db::open_in_memory().unwrap();
    store::sync_app_type_at(&db, "central", Some("tamanu-central"), since).unwrap();
    db
}

// spec: CNR#missing-offers
#[test]
fn the_period_runs_from_each_starting_moment_when_the_others_are_earlier() {
    // The daemon's start.
    let db = db_with_app(START - 10 * PERIOD);
    sweep(&db, START + PERIOD - 1, START, true, false).unwrap();
    assert!(held(&db).is_empty());
    sweep(&db, START + PERIOD, START, true, false).unwrap();
    assert_eq!(held(&db), ["central"]);

    // The last live offer.
    let db = db_with_app(START - 10 * PERIOD);
    store::mark_offer_live(&db, START + 100).unwrap();
    sweep(&db, START + 100 + PERIOD - 1, START, true, false).unwrap();
    assert!(held(&db).is_empty());
    sweep(&db, START + 100 + PERIOD, START, true, false).unwrap();
    assert_eq!(held(&db), ["central"]);

    // Access last enabled.
    let db = db_with_app(START - 10 * PERIOD);
    store::set_enabled_at(&db, false, START + 50).unwrap();
    store::set_enabled_at(&db, true, START + 200).unwrap();
    sweep(&db, START + 200 + PERIOD - 1, START, true, false).unwrap();
    assert!(held(&db).is_empty());
    sweep(&db, START + 200 + PERIOD, START, true, false).unwrap();
    assert_eq!(held(&db), ["central"]);

    // The app coming to rely on Canopy.
    let db = db_with_app(START + 300);
    sweep(&db, START + 300 + PERIOD - 1, START, true, false).unwrap();
    assert!(held(&db).is_empty());
    sweep(&db, START + 300 + PERIOD, START, true, false).unwrap();
    assert_eq!(held(&db), ["central"]);
}

// spec: CNR#missing-offers
#[test]
fn a_short_absence_files_nothing() {
    let db = db_with_app(START - 10 * PERIOD);
    store::mark_offer_live(&db, START + 1000).unwrap();
    sweep(&db, START + 1000 + 60, START, true, false).unwrap();
    assert!(held(&db).is_empty());
}

// spec: CNR#missing-offers
#[test]
fn the_description_says_when_an_offer_was_last_live_or_that_none_ever_was() {
    let db = db_with_app(START - 10 * PERIOD);
    sweep(&db, START + PERIOD, START, true, false).unwrap();
    let never = faults::list_active_faults(&db, None).unwrap().remove(0);
    assert!(never.description.contains("ever"), "{}", never.description);

    let db = db_with_app(START - 10 * PERIOD);
    store::mark_offer_live(&db, START + 1).unwrap();
    sweep(&db, START + 1 + PERIOD, START, true, false).unwrap();
    let once = faults::list_active_faults(&db, None).unwrap().remove(0);
    assert!(once.description.contains("since"), "{}", once.description);
    assert!(
        once.description.contains("1970-01-12"),
        "{}",
        once.description
    );
}

// spec: CNR#missing-offers
#[test]
fn sweeping_again_does_not_file_a_second_fault() {
    let db = db_with_app(START - 10 * PERIOD);
    for i in 0..3 {
        sweep(&db, START + PERIOD + i * 60, START, true, false).unwrap();
    }
    assert_eq!(held(&db), ["central"]);
}

// spec: CNR#missing-offers
#[test]
fn a_held_fault_survives_a_restart_and_clears_on_the_next_offer() {
    let db = db_with_app(START - 10 * PERIOD);
    sweep(&db, START + PERIOD, START, true, false).unwrap();
    assert_eq!(held(&db), ["central"]);

    // The daemon starts again, so the period has not run on its new start. The
    // sweep neither files nor clears.
    let restarted_at = START + PERIOD + 600;
    sweep(&db, restarted_at + 30, restarted_at, true, false).unwrap();
    assert_eq!(held(&db), ["central"]);

    // An offer going live clears it, with the sweep or without.
    clear_all(&db).unwrap();
    assert!(held(&db).is_empty());
}

// spec: CNR#missing-offers
#[test]
fn a_sweep_that_finds_an_offer_live_clears_the_fault() {
    let db = db_with_app(START - 10 * PERIOD);
    sweep(&db, START + PERIOD, START, true, false).unwrap();
    sweep(&db, START + PERIOD + 60, START, true, true).unwrap();
    assert!(held(&db).is_empty());
}

// spec: CNR#missing-offers
#[test]
fn disabling_access_clears_the_fault_and_files_nothing_while_disabled() {
    let db = db_with_app(START - 10 * PERIOD);
    sweep(&db, START + PERIOD, START, true, false).unwrap();
    assert_eq!(held(&db), ["central"]);

    sweep(&db, START + PERIOD + 60, START, false, false).unwrap();
    assert!(held(&db).is_empty());
    sweep(&db, START + 100 * PERIOD, START, false, false).unwrap();
    assert!(held(&db).is_empty());
}

// spec: CNR#missing-offers
#[test]
fn re_enabling_access_restarts_the_period() {
    let db = db_with_app(START - 10 * PERIOD);
    store::set_enabled_at(&db, false, START + 100).unwrap();
    let reenabled = START + 50 * PERIOD;
    store::set_enabled_at(&db, true, reenabled).unwrap();

    sweep(&db, reenabled + PERIOD - 1, START, true, false).unwrap();
    assert!(held(&db).is_empty());
    sweep(&db, reenabled + PERIOD, START, true, false).unwrap();
    assert_eq!(held(&db), ["central"]);
}

// spec: CNR#missing-offers
#[test]
fn an_app_that_drops_its_type_loses_the_fault() {
    let db = db_with_app(START - 10 * PERIOD);
    store::sync_app_type_at(&db, "other", Some("tamanu-facility"), START - 10 * PERIOD).unwrap();
    sweep(&db, START + PERIOD, START, true, false).unwrap();
    assert_eq!(held(&db).len(), 2);

    sync_app_type(&db, &app("central"), None).unwrap();
    assert_eq!(held(&db), ["other"]);
}

// spec: CNR#missing-offers
#[test]
fn an_untyped_app_is_never_faulted() {
    let db = Db::open_in_memory().unwrap();
    sweep(&db, START + 100 * PERIOD, START, true, false).unwrap();
    assert!(held(&db).is_empty());
}

// spec: CNR#missing-offers
#[test]
fn each_app_relying_on_canopy_is_faulted_on_its_own() {
    let db = db_with_app(START - 10 * PERIOD);
    store::sync_app_type_at(&db, "late", Some("tamanu-facility"), START + 5 * PERIOD).unwrap();
    sweep(&db, START + PERIOD, START, true, false).unwrap();
    assert_eq!(held(&db), ["central"], "the later app has had less time");
}
