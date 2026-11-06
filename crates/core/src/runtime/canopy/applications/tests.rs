use seedling_protocol::names::ActionName;

use super::*;
use crate::runtime::db::Db;

fn name(n: &str) -> AppName {
    AppName::new_unchecked(n)
}

fn db() -> Db {
    Db::open_in_memory().expect("in-memory db")
}

fn operating() -> AppStatus {
    AppStatus::Operating {
        action_name: ActionName::new_unchecked("migrate"),
    }
}

fn result_of(entry: &Value) -> &str {
    entry["health"][0]["result"].as_str().unwrap()
}

// r[verify canopy.app.key]
#[test]
fn several_apps_of_one_type_are_each_an_application_keyed_by_name() {
    let db = db();
    store::sync_app_type(&db, "tamanu-a", Some("tamanu-central")).unwrap();
    store::sync_app_type(&db, "tamanu-b", Some("tamanu-central")).unwrap();
    let statuses = [
        (name("tamanu-a"), AppStatus::Running),
        (name("tamanu-b"), AppStatus::Running),
    ];

    let report = report_applications(&db, &statuses).unwrap();
    let entries = report.as_object().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(report["tamanu-a"]["type"], "tamanu-central");
    assert_eq!(report["tamanu-b"]["type"], "tamanu-central");
    assert_eq!(report["tamanu-a"]["health"][0]["check"], "health/app");
}

// r[verify canopy.app.key]
#[test]
fn an_untyped_app_is_not_reported() {
    let db = db();
    store::sync_app_type(&db, "typed", Some("tamanu-central")).unwrap();
    let statuses = [
        (name("typed"), AppStatus::Running),
        (name("plain"), AppStatus::Running),
    ];
    let report = report_applications(&db, &statuses).unwrap();
    assert!(report.get("plain").is_none());
    assert!(report.get("typed").is_some());
}

// r[verify canopy.app.key]
#[test]
fn a_type_with_no_registered_app_is_not_reported() {
    let db = db();
    store::sync_app_type(&db, "gone", Some("tamanu-central")).unwrap();
    let report = report_applications(&db, &[]).unwrap();
    assert!(report.as_object().unwrap().is_empty());
}

// r[verify canopy.app.health]
#[test]
fn each_status_grades_as_specified() {
    let cases = [
        (AppStatus::Running, "passed"),
        (AppStatus::Degraded, "warning"),
        (AppStatus::Faulted, "failed"),
        (AppStatus::NotInstalled, "skipped"),
        (AppStatus::Installing, "skipped"),
        (AppStatus::Uninstalling, "skipped"),
        (operating(), "skipped"),
    ];
    for (status, expected) in cases {
        let db = db();
        store::sync_app_type(&db, "app", Some("t")).unwrap();
        let name_ = status.name();
        let report = report_applications(&db, &[(name("app"), status)]).unwrap();
        assert_eq!(result_of(&report["app"]), expected, "{name_}");
        assert_eq!(report["app"]["health"][0]["status"], name_);
    }
}

// r[verify canopy.app.health]
#[test]
fn an_operating_app_resends_the_grade_from_before_the_operation() {
    for (before, expected) in [
        (AppStatus::Running, "passed"),
        (AppStatus::Degraded, "warning"),
        (AppStatus::Faulted, "failed"),
    ] {
        let db = db();
        store::sync_app_type(&db, "app", Some("t")).unwrap();
        report_applications(&db, &[(name("app"), before)]).unwrap();

        let during = report_applications(&db, &[(name("app"), operating())]).unwrap();
        assert_eq!(result_of(&during["app"]), expected);
        let again = report_applications(&db, &[(name("app"), operating())]).unwrap();
        assert_eq!(
            result_of(&again["app"]),
            expected,
            "the operation does not wear the grade away"
        );
    }
}

// r[verify canopy.app.health]
#[test]
fn the_grade_before_an_operation_is_in_the_database_not_in_memory() {
    // Nothing in memory carries the grade: the report reads it from the
    // database each time, so a restart between the grade and the operation
    // reads the same row a fresh handle would.
    let db = db();
    store::sync_app_type(&db, "app", Some("t")).unwrap();
    report_applications(&db, &[(name("app"), AppStatus::Faulted)]).unwrap();

    let stored = store::list_apps(&db).unwrap();
    assert_eq!(stored[0].last_grade.as_deref(), Some("failed"));

    let after_restart = report_applications(&db, &[(name("app"), operating())]).unwrap();
    assert_eq!(result_of(&after_restart["app"]), "failed");
}

// r[verify canopy.app.health]
#[test]
fn an_operating_app_with_no_earlier_grade_is_skipped() {
    let db = db();
    store::sync_app_type(&db, "app", Some("t")).unwrap();
    let report = report_applications(&db, &[(name("app"), operating())]).unwrap();
    assert_eq!(result_of(&report["app"]), "skipped");
}

// r[verify canopy.app.health]
#[test]
fn the_check_lists_each_active_fault_with_kind_resource_and_description() {
    let db = db();
    store::sync_app_type(&db, "app", Some("t")).unwrap();
    faults::file_fault(
        &db,
        &name("app"),
        Some("deployment"),
        Some("web"),
        None,
        "image_pull_failed",
        "no such tag",
    )
    .unwrap();
    faults::file_fault(&db, &name("app"), None, None, None, "script_error", "bad").unwrap();
    faults::file_fault(&db, &name("other"), None, None, None, "elsewhere", "x").unwrap();

    let report = report_applications(&db, &[(name("app"), AppStatus::Faulted)]).unwrap();
    let listed = report["app"]["health"][0]["faults"].as_array().unwrap();
    assert_eq!(listed.len(), 2, "only this app's faults: {listed:?}");
    let pull = listed
        .iter()
        .find(|f| f["kind"] == "image_pull_failed")
        .unwrap();
    assert_eq!(pull["resource"], "deployment/web");
    assert_eq!(pull["description"], "no such tag");
    let script = listed.iter().find(|f| f["kind"] == "script_error").unwrap();
    assert!(script.get("resource").is_none());
}

fn held_grade(db: &Db) -> Option<String> {
    store::list_apps(db).unwrap().remove(0).last_grade
}

// r[verify canopy.app.health]
#[test]
fn an_uninstalled_app_does_not_carry_its_grade_into_the_next_install() {
    let db = db();
    store::sync_app_type(&db, "app", Some("t")).unwrap();
    report_applications(&db, &[(name("app"), AppStatus::Faulted)]).unwrap();
    assert_eq!(held_grade(&db).as_deref(), Some("failed"));

    report_applications(&db, &[(name("app"), AppStatus::Uninstalling)]).unwrap();
    assert_eq!(
        held_grade(&db).as_deref(),
        Some("failed"),
        "an operation during the uninstall still re-sends what it went in with"
    );

    report_applications(&db, &[(name("app"), AppStatus::NotInstalled)]).unwrap();
    assert_eq!(held_grade(&db), None);

    // The next install is a lifetime of its own, and has not been graded.
    report_applications(&db, &[(name("app"), AppStatus::Installing)]).unwrap();
    let during = report_applications(&db, &[(name("app"), operating())]).unwrap();
    assert_eq!(result_of(&during["app"]), "skipped");
}

// r[verify canopy.app.health]
#[test]
fn a_grade_that_has_not_changed_is_not_written_again() {
    let db = db();
    store::sync_app_type(&db, "app", Some("t")).unwrap();
    report_applications(&db, &[(name("app"), AppStatus::Running)]).unwrap();

    let before = db.conn.total_changes();
    report_applications(&db, &[(name("app"), AppStatus::Running)]).unwrap();
    assert_eq!(
        db.conn.total_changes(),
        before,
        "a tick that grades the app the same as the last writes nothing"
    );

    report_applications(&db, &[(name("app"), AppStatus::Degraded)]).unwrap();
    assert_eq!(held_grade(&db).as_deref(), Some("warning"));
}

// r[verify canopy.app.health]
#[test]
fn a_fault_with_a_type_and_no_name_still_says_what_it_is_against() {
    let db = db();
    store::sync_app_type(&db, "app", Some("t")).unwrap();
    faults::file_fault(
        &db,
        &name("app"),
        Some("deployment"),
        None,
        None,
        "scale_refused",
        "no capacity",
    )
    .unwrap();

    let report = report_applications(&db, &[(name("app"), AppStatus::Degraded)]).unwrap();
    let listed = report["app"]["health"][0]["faults"].as_array().unwrap();
    assert_eq!(listed[0]["resource"], "deployment");
}
