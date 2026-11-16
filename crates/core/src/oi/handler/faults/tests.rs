use serde_json::json;

use crate::oi::test_support::TestOi;

/// Registers `broken` (script error fault) and `demo` (no faults).
fn oi_with_fault() -> TestOi {
    let oi = TestOi::with_app("demo");
    oi.call(
        "/apps/create",
        json!({ "app": "broken", "script": r#"throw "kaboom";"# }),
    )
    .unwrap();
    oi
}

// spec: FLTS#listing-faults
#[test]
fn list_returns_all_faults_or_filters_by_app() {
    let oi = oi_with_fault();

    let all = oi.call("/faults/list", json!({})).unwrap();
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["app"], "broken");
    assert_eq!(all[0]["kind"], "script_error");
    assert!(all[0]["description"].as_str().unwrap().contains("kaboom"));
    assert!(!all[0]["id"].as_str().unwrap().is_empty());

    let filtered = oi.call("/faults/list", json!({ "app": "demo" })).unwrap();
    assert!(filtered.as_array().unwrap().is_empty());
}

// spec: FLTS#clearing-faults
#[test]
fn clear_app_faults_reports_count_and_empties_list() {
    let oi = oi_with_fault();

    let result = oi
        .call("/faults/clear", json!({ "app": "broken" }))
        .unwrap();
    assert_eq!(result["app"], "broken");
    assert_eq!(result["cleared"], 1);

    let all = oi.call("/faults/list", json!({})).unwrap();
    assert!(all.as_array().unwrap().is_empty());

    // Clearing an app with no active faults is a no-op, not an error.
    let result = oi.call("/faults/clear", json!({ "app": "demo" })).unwrap();
    assert_eq!(result["cleared"], 0);
}

fn file_host_fault(oi: &TestOi, app: &str, kind: &str) {
    let (app, kind) = (
        seedling_protocol::names::AppName::new_unchecked(app),
        kind.to_owned(),
    );
    oi.state.db.call(move |db| {
        crate::runtime::faults::file_fault(db, &app, None, None, None, &kind, "filed by a test")
            .unwrap();
    });
}

// spec: FLTS#clearing-faults
#[test]
fn clear_accepts_the_host_fault_names_and_clears_every_fault_filed_against_them() {
    let oi = oi_with_fault();
    file_host_fault(&oi, "_system", "reconciler_panic");
    file_host_fault(&oi, "_system", "timezone_undetermined");
    file_host_fault(&oi, "seedling", "host_note");

    let result = oi
        .call("/faults/clear", json!({ "app": "_system" }))
        .unwrap();
    assert_eq!(result["app"], "_system");
    assert_eq!(result["cleared"], 2);

    let remaining = oi.call("/faults/list", json!({})).unwrap();
    let apps: Vec<&str> = remaining
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["app"].as_str().unwrap())
        .collect();
    assert_eq!(apps.len(), 2);
    assert!(apps.contains(&"broken") && apps.contains(&"seedling"));

    let result = oi
        .call("/faults/clear", json!({ "app": "seedling" }))
        .unwrap();
    assert_eq!(result["cleared"], 1);
}

// spec: FLTS#listing-faults
#[test]
fn list_filters_by_a_host_fault_name() {
    let oi = oi_with_fault();
    file_host_fault(&oi, "_system", "reconciler_panic");

    let listed = oi
        .call("/faults/list", json!({ "app": "_system" }))
        .unwrap();
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["kind"], "reconciler_panic");
}

// spec: FLTS#clearing-faults
// spec: FLTS#listing-faults
#[test]
fn an_unregistered_app_is_not_found() {
    let oi = oi_with_fault();
    for method in ["/faults/clear", "/faults/list"] {
        let (code, _) = oi.call(method, json!({ "app": "ghost" })).unwrap_err();
        assert_eq!(code, "not_found", "{method}");
    }
}
