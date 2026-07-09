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

// i[verify fault.list]
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

// i[verify fault.clear-app]
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
