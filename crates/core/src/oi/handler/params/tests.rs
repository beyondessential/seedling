use serde_json::json;

use crate::oi::test_support::{PARAMS_SCRIPT, TestOi};

fn oi_with_params_app() -> TestOi {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "demo", "script": PARAMS_SCRIPT }),
    )
    .expect("register app");
    oi
}

// i[verify param.set]
// i[verify param.store]
#[test]
fn set_param_bumps_generation_and_stores_value() {
    let oi = oi_with_params_app();
    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "name": "greeting", "value": "hello" }),
        )
        .unwrap();
    assert_eq!(result["generation"], 2);
    assert_eq!(result["schedule"], "not_scheduled");

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    let params = desc["params"].as_array().unwrap();
    let greeting = params
        .iter()
        .find(|p| p["name"] == "greeting")
        .expect("greeting param");
    assert_eq!(greeting["value"], "hello");
    assert_eq!(greeting["is_set"], true);
}

// i[verify param.set]
#[test]
fn set_same_value_is_a_noop() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "name": "greeting", "value": "hello" }),
    )
    .unwrap();
    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "demo", "name": "greeting", "value": "hello" }),
        )
        .unwrap();
    assert_eq!(result["generation"], 2);
    assert_eq!(result["schedule"], "not_scheduled");
}

// i[verify param.store.secret]
// i[verify app.describe.param-secret]
#[test]
fn secret_param_is_redacted_in_describe_and_history() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "name": "api-key", "value": "hunter2hunter2" }),
    )
    .unwrap();

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    let params = desc["params"].as_array().unwrap();
    let key = params
        .iter()
        .find(|p| p["name"] == "api-key")
        .expect("api-key param");
    assert_eq!(key["is_set"], true);
    assert!(key["value"].is_null());

    let generations = oi
        .call("/apps/generations", json!({ "app": "demo" }))
        .unwrap();
    let entry = generations
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "param_set")
        .expect("param_set entry");
    assert_eq!(entry["redacted"], true);
    assert!(entry.get("new_value").is_none());
}

// i[verify param.unknown]
#[test]
fn set_undeclared_param_is_stored_as_unknown() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "name": "mystery", "value": "42" }),
    )
    .unwrap();

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    let unknown = desc["unknown_params"].as_array().unwrap();
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0]["name"], "mystery");
    assert_eq!(unknown[0]["value"], "42");
}

// i[verify param.unset]
#[test]
fn unset_param_removes_value_and_bumps_generation() {
    let oi = oi_with_params_app();
    oi.call(
        "/apps/params/set",
        json!({ "app": "demo", "name": "greeting", "value": "hello" }),
    )
    .unwrap();
    let result = oi
        .call(
            "/apps/params/unset",
            json!({ "app": "demo", "name": "greeting" }),
        )
        .unwrap();
    assert_eq!(result["generation"], 3);

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    let greeting = desc["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "greeting")
        .expect("greeting param");
    assert_eq!(greeting["is_set"], false);
    assert!(greeting["value"].is_null());
}

// i[verify param.unset]
#[test]
fn unset_never_set_param_is_a_noop() {
    let oi = oi_with_params_app();
    let result = oi
        .call(
            "/apps/params/unset",
            json!({ "app": "demo", "name": "greeting" }),
        )
        .unwrap();
    assert_eq!(result["generation"], 1);
    assert_eq!(result["schedule"], "not_scheduled");
}

// i[verify param.set]
#[test]
fn param_calls_on_unknown_app_are_not_found() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call(
            "/apps/params/set",
            json!({ "app": "ghost", "name": "greeting", "value": "x" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    let (code, _) = oi
        .call(
            "/apps/params/unset",
            json!({ "app": "ghost", "name": "greeting" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}
