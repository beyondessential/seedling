use serde_json::json;

use crate::oi::test_support::TestOi;

// i[verify registry.list]
#[test]
fn list_returns_default_allowlist() {
    let oi = TestOi::new();
    let result = oi.call("/registries/list", json!({})).unwrap();
    let registries = result["registries"].as_array().unwrap();
    assert!(registries.contains(&json!("docker.io")));
    assert!(registries.contains(&json!("ghcr.io")));
}

// i[verify registry.add]
#[test]
fn add_extends_allowlist_idempotently() {
    let oi = TestOi::new();
    oi.call("/registries/add", json!({ "registry": "quay.io" }))
        .unwrap();
    oi.call("/registries/add", json!({ "registry": "quay.io" }))
        .unwrap();

    let result = oi.call("/registries/list", json!({})).unwrap();
    let registries = result["registries"].as_array().unwrap();
    assert_eq!(
        registries
            .iter()
            .filter(|r| **r == json!("quay.io"))
            .count(),
        1
    );
}

// i[verify registry.remove]
#[test]
fn remove_deletes_registry_or_errors_when_absent() {
    let oi = TestOi::new();
    oi.call("/registries/remove", json!({ "registry": "ghcr.io" }))
        .unwrap();
    let result = oi.call("/registries/list", json!({})).unwrap();
    assert!(
        !result["registries"]
            .as_array()
            .unwrap()
            .contains(&json!("ghcr.io"))
    );

    let (code, _) = oi
        .call("/registries/remove", json!({ "registry": "ghcr.io" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify registry.remove]
// i[verify fault.derived]
#[test]
fn disallowed_registry_files_fault_and_removal_reevaluates_apps() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"app.deployment("web").image("quay.io/example/web:1");"#,
        }),
    )
    .unwrap();

    let faults = oi.call("/faults/list", json!({ "app": "demo" })).unwrap();
    let faults = faults.as_array().unwrap();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0]["kind"], "disallowed_registry");
    assert!(
        faults[0]["description"]
            .as_str()
            .unwrap()
            .contains("quay.io")
    );

    // Removing an unrelated registry re-evaluates apps; the quay.io fault
    // stays because quay.io is still not allowed.
    oi.call("/registries/remove", json!({ "registry": "ghcr.io" }))
        .unwrap();
    let faults = oi.call("/faults/list", json!({ "app": "demo" })).unwrap();
    assert_eq!(faults.as_array().unwrap().len(), 1);
}
