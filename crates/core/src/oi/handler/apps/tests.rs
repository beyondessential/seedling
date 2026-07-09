use serde_json::json;

use crate::oi::test_support::{MINIMAL_SCRIPT, TestOi};

// i[verify app.register]
// i[verify app.list]
#[test]
fn register_creates_generation_one_and_lists_app() {
    let oi = TestOi::new();
    let result = oi
        .call(
            "/apps/create",
            json!({ "app": "demo", "script": MINIMAL_SCRIPT }),
        )
        .unwrap();
    assert_eq!(result["generation"], 1);

    let list = oi.call("/apps/list", json!({})).unwrap();
    let apps = list.as_array().unwrap();
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0]["name"], "demo");
    assert_eq!(apps[0]["status"], "not_installed");
    assert_eq!(apps[0]["fault_count"], 0);
    assert_eq!(apps[0]["has_stopped_resources"], false);
}

// i[verify app.register]
#[test]
fn register_rejects_invalid_names() {
    let oi = TestOi::new();
    for name in ["_hidden", "ab", "-leading", "trailing-"] {
        let (code, _) = oi
            .call(
                "/apps/create",
                json!({ "app": name, "script": MINIMAL_SCRIPT }),
            )
            .unwrap_err();
        assert_eq!(code, "requirements_invalid", "name: {name}");
    }
}

// i[verify app.register]
#[test]
fn register_rejects_duplicate_app() {
    let oi = TestOi::with_app("demo");
    let (code, message) = oi
        .call(
            "/apps/create",
            json!({ "app": "demo", "script": MINIMAL_SCRIPT }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(message.contains("already registered"), "message: {message}");
}

// i[verify app.register]
// i[verify fault.record]
#[test]
fn register_with_failing_script_files_script_error_fault() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "broken", "script": r#"throw "boom";"# }),
    )
    .unwrap();

    let faults = oi.call("/faults/list", json!({ "app": "broken" })).unwrap();
    let faults = faults.as_array().unwrap();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0]["kind"], "script_error");

    // The script-error gate also blocks install.
    let (code, _) = oi
        .call("/apps/install/invoke", json!({ "app": "broken" }))
        .unwrap_err();
    assert_eq!(code, "script_error");
}

// i[verify app.describe]
#[test]
fn describe_returns_resources_and_status() {
    let oi = TestOi::with_app("demo");
    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    assert_eq!(desc["status"], "not_installed");
    assert_eq!(desc["generation"], 1);
    let resources = desc["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["name"], "web");
    assert_eq!(resources[0]["stopped"], false);
    // i[verify scale.describe]
    assert_eq!(resources[0]["scale"]["low"], 1);
    assert_eq!(resources[0]["scale"]["high"], 4);
    assert_eq!(resources[0]["scale"]["current"], 1);
    assert!(desc["faults"].as_array().unwrap().is_empty());
    assert!(desc["params"].as_array().unwrap().is_empty());
}

// i[verify app.describe]
#[test]
fn describe_unknown_app_is_not_found() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call("/apps/show", json!({ "app": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify app.update]
// i[verify generation.history]
#[test]
fn update_bumps_generation_and_records_history() {
    let oi = TestOi::with_app("demo");
    let updated_script = r#"
        app.deployment("web")
            .image("docker.io/library/nginx:1.30")
            .scale(2);
    "#;
    let result = oi
        .call(
            "/apps/update",
            json!({ "app": "demo", "script": updated_script }),
        )
        .unwrap();
    assert_eq!(result["generation"], 2);

    let generations = oi
        .call("/apps/generations", json!({ "app": "demo" }))
        .unwrap();
    let entries = generations.as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let kinds: Vec<&str> = entries
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"register"));
    assert!(kinds.contains(&"script_update"));
}

// i[verify app.update]
#[test]
fn update_unknown_app_is_not_found() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call(
            "/apps/update",
            json!({ "app": "ghost", "script": MINIMAL_SCRIPT }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify app.deregister]
#[test]
fn remove_deletes_registered_app() {
    let oi = TestOi::with_app("demo");
    oi.call("/apps/remove", json!({ "app": "demo" })).unwrap();
    let list = oi.call("/apps/list", json!({})).unwrap();
    assert!(list.as_array().unwrap().is_empty());

    let (code, _) = oi
        .call("/apps/remove", json!({ "app": "demo" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify app.deregister]
#[test]
fn remove_rejects_installed_app() {
    let oi = TestOi::with_app("demo");
    oi.install("demo");
    let (code, message) = oi
        .call("/apps/remove", json!({ "app": "demo" }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(message.contains("uninstall first"), "message: {message}");
}

#[test]
fn uninstall_requires_installed_phase() {
    let oi = TestOi::with_app("demo");
    let (code, _) = oi
        .call("/apps/uninstall", json!({ "app": "demo" }))
        .unwrap_err();
    assert_eq!(code, "not_installed");

    let (code, _) = oi
        .call("/apps/uninstall", json!({ "app": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify scale.reset-on-uninstall]
#[test]
fn uninstall_transitions_phase_and_resets_scaling() {
    let oi = TestOi::with_app("demo");
    oi.install("demo");
    oi.call(
        "/apps/scale",
        json!({ "app": "demo", "deployment": "web", "scale": 3 }),
    )
    .unwrap();

    oi.call("/apps/uninstall", json!({ "app": "demo" }))
        .unwrap();

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    assert_eq!(desc["status"], "uninstalling");
    assert_eq!(desc["resources"][0]["scale"]["current"], 1);
}

// i[verify app.script]
#[test]
fn script_returns_current_and_historic_generations() {
    let oi = TestOi::with_app("demo");
    let result = oi.call("/apps/script", json!({ "app": "demo" })).unwrap();
    assert_eq!(result["generation"], 1);
    assert_eq!(result["script"], MINIMAL_SCRIPT);

    let updated_script = r#"app.deployment("web").image("docker.io/library/nginx:1.30");"#;
    oi.call(
        "/apps/update",
        json!({ "app": "demo", "script": updated_script }),
    )
    .unwrap();
    let result = oi
        .call("/apps/script", json!({ "app": "demo", "generation": 1 }))
        .unwrap();
    assert_eq!(result["generation"], 1);
    assert_eq!(result["script"], MINIMAL_SCRIPT);

    let (code, _) = oi
        .call("/apps/script", json!({ "app": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify generation.history]
#[test]
fn generations_unknown_app_is_not_found() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call("/apps/generations", json!({ "app": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify plan.dry-run]
#[test]
fn plan_with_no_proposals_returns_empty_diff() {
    let oi = TestOi::with_app("demo");
    let plan = oi.call("/apps/plan", json!({ "app": "demo" })).unwrap();
    assert!(plan["diff"].as_array().unwrap().is_empty());
    assert!(plan["on_change_would_fire"].as_array().unwrap().is_empty());
}

// i[verify plan.dry-run]
#[test]
fn plan_reports_added_removed_and_modified_resources() {
    let oi = TestOi::with_app("demo");
    let proposed = r#"
        app.deployment("web")
            .image("docker.io/library/nginx:1.30")
            .scale(1..4);
        app.volume("data");
    "#;
    let plan = oi
        .call(
            "/apps/plan",
            json!({ "app": "demo", "proposed_script": proposed }),
        )
        .unwrap();
    let diff = plan["diff"].as_array().unwrap();
    let added = diff
        .iter()
        .find(|d| d["change"] == "added")
        .expect("added entry");
    assert_eq!(added["resource_type"], "Volume");
    assert_eq!(added["resource_name"], "data");
    let modified = diff
        .iter()
        .find(|d| d["change"] == "modified")
        .expect("modified entry");
    assert_eq!(modified["resource_name"], "web");
    assert!(!modified["fields"].as_array().unwrap().is_empty());

    let plan = oi
        .call(
            "/apps/plan",
            json!({ "app": "demo", "proposed_script": r#"app.volume("data");"# }),
        )
        .unwrap();
    let diff = plan["diff"].as_array().unwrap();
    assert!(
        diff.iter()
            .any(|d| d["change"] == "removed" && d["resource_name"] == "web")
    );
}

// i[verify plan.dry-run]
#[test]
fn plan_with_failing_proposed_script_reports_errors() {
    let oi = TestOi::with_app("demo");
    let plan = oi
        .call(
            "/apps/plan",
            json!({ "app": "demo", "proposed_script": r#"throw "nope";"# }),
        )
        .unwrap();
    assert_eq!(plan["errors"].as_array().unwrap().len(), 1);
}

// i[verify scale.set]
// i[verify scale.decision-persistence]
#[test]
fn scale_persists_decision_within_bounds() {
    let oi = TestOi::with_app("demo");
    let result = oi
        .call(
            "/apps/scale",
            json!({ "app": "demo", "deployment": "web", "scale": 3 }),
        )
        .unwrap();
    assert_eq!(result["scale"], 3);
    assert_eq!(result["bounds"]["low"], 1);
    assert_eq!(result["bounds"]["high"], 4);

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    assert_eq!(desc["resources"][0]["scale"]["current"], 3);
}

// i[verify scale.set]
#[test]
fn scale_clamps_to_bounds_and_validates_targets() {
    let oi = TestOi::with_app("demo");
    let result = oi
        .call(
            "/apps/scale",
            json!({ "app": "demo", "deployment": "web", "scale": 99 }),
        )
        .unwrap();
    assert_eq!(result["scale"], 4);

    let (code, _) = oi
        .call(
            "/apps/scale",
            json!({ "app": "demo", "deployment": "ghost", "scale": 2 }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    let (code, _) = oi
        .call(
            "/apps/scale",
            json!({ "app": "ghost", "deployment": "web", "scale": 2 }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify deployment.restart]
#[test]
fn restart_returns_operation_id_for_known_deployment() {
    let oi = TestOi::with_app("demo");
    let result = oi
        .call(
            "/apps/restart",
            json!({ "app": "demo", "deployment": "web" }),
        )
        .unwrap();
    assert!(!result["operation_id"].as_str().unwrap().is_empty());

    let (code, _) = oi
        .call(
            "/apps/restart",
            json!({ "app": "demo", "deployment": "ghost" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify resource.stop]
// i[verify resource.stop.status]
#[test]
fn stop_resource_marks_it_stopped_everywhere() {
    let oi = TestOi::with_app("demo");
    oi.call(
        "/apps/resource/stop",
        json!({ "app": "demo", "kind": "deployment", "name": "web" }),
    )
    .unwrap();

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    assert_eq!(desc["resources"][0]["stopped"], true);
    let stopped = desc["stopped_resources"].as_array().unwrap();
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0]["kind"], "deployment");
    assert_eq!(stopped[0]["name"], "web");

    let list = oi.call("/apps/list", json!({})).unwrap();
    assert_eq!(list[0]["has_stopped_resources"], true);
}

// i[verify resource.stop]
#[test]
fn stop_resource_rejects_unstoppable_kind_and_unknown_resource() {
    let oi = TestOi::with_app("demo");
    let (code, message) = oi
        .call(
            "/apps/resource/stop",
            json!({ "app": "demo", "kind": "service", "name": "web" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(message.contains("cannot be stopped"), "message: {message}");

    let (code, _) = oi
        .call(
            "/apps/resource/stop",
            json!({ "app": "demo", "kind": "deployment", "name": "ghost" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify resource.unstop]
#[test]
fn unstop_resource_clears_stopped_flag() {
    let oi = TestOi::with_app("demo");
    oi.call(
        "/apps/resource/stop",
        json!({ "app": "demo", "kind": "deployment", "name": "web" }),
    )
    .unwrap();
    oi.call(
        "/apps/resource/unstop",
        json!({ "app": "demo", "kind": "deployment", "name": "web" }),
    )
    .unwrap();

    let desc = oi.call("/apps/show", json!({ "app": "demo" })).unwrap();
    assert_eq!(desc["resources"][0]["stopped"], false);
    assert!(desc["stopped_resources"].as_array().unwrap().is_empty());

    let (code, _) = oi
        .call(
            "/apps/resource/unstop",
            json!({ "app": "demo", "kind": "gadget", "name": "web" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
}

// i[verify resource.unstop-all]
#[test]
fn unstop_all_clears_every_stopped_resource() {
    let oi = TestOi::with_app("demo");
    oi.call(
        "/apps/resource/stop",
        json!({ "app": "demo", "kind": "deployment", "name": "web" }),
    )
    .unwrap();
    oi.call("/apps/unstop", json!({ "app": "demo" })).unwrap();

    let list = oi.call("/apps/list", json!({})).unwrap();
    assert_eq!(list[0]["has_stopped_resources"], false);

    let (code, _) = oi
        .call("/apps/unstop", json!({ "app": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}
