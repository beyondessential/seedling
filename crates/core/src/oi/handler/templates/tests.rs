use serde_json::json;

use crate::oi::test_support::TestOi;

const TEMPLATE_BODY: &str = r#"
    app.description("demo template");
    app.volume("data");
"#;

// i[verify template.create]
// i[verify template.list]
// i[verify template.show]
#[test]
fn template_create_list_show_roundtrip() {
    let oi = TestOi::new();
    assert_eq!(oi.call("/templates/list", json!({})).unwrap(), json!([]));

    let created = oi
        .call(
            "/templates/create",
            json!({ "name": "demo", "body": TEMPLATE_BODY, "description": "a demo" }),
        )
        .unwrap();
    assert_eq!(created["name"], "demo");
    assert!(created["created_at"].is_string());

    let list = oi.call("/templates/list", json!({})).unwrap();
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"], "demo");
    assert_eq!(list[0]["description"], "a demo");

    let shown = oi
        .call("/templates/show", json!({ "name": "demo" }))
        .unwrap();
    assert_eq!(shown["body"], TEMPLATE_BODY);
    assert_eq!(shown["description"], "a demo");

    let (code, _) = oi
        .call("/templates/show", json!({ "name": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify template.create]
// i[verify template.name]
#[test]
fn template_create_rejects_duplicates_and_bad_names() {
    let oi = TestOi::new();
    oi.call("/templates/create", json!({ "name": "demo", "body": "1;" }))
        .unwrap();

    let (code, msg) = oi
        .call("/templates/create", json!({ "name": "demo", "body": "2;" }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("already exists"), "{msg}");

    let (code, _) = oi
        .call(
            "/templates/create",
            json!({ "name": "_bad name!", "body": "1;" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
}

// i[verify template.update]
#[test]
fn template_update_body_and_description() {
    let oi = TestOi::new();
    oi.call(
        "/templates/create",
        json!({ "name": "demo", "body": "1;", "description": "before" }),
    )
    .unwrap();

    assert_eq!(
        oi.call("/templates/update", json!({ "name": "demo", "body": "2;" }),)
            .unwrap()["updated"],
        true
    );
    let shown = oi
        .call("/templates/show", json!({ "name": "demo" }))
        .unwrap();
    assert_eq!(shown["body"], "2;");
    assert_eq!(shown["description"], "before", "absent field keeps value");

    // An explicit null clears the description.
    oi.call(
        "/templates/update",
        json!({ "name": "demo", "description": null }),
    )
    .unwrap();
    let shown = oi
        .call("/templates/show", json!({ "name": "demo" }))
        .unwrap();
    assert_eq!(shown["description"], json!(null));

    let (code, _) = oi
        .call(
            "/templates/update",
            json!({ "name": "ghost", "body": "2;" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify template.remove]
#[test]
fn template_remove_deletes_once() {
    let oi = TestOi::new();
    oi.call("/templates/create", json!({ "name": "demo", "body": "1;" }))
        .unwrap();

    assert_eq!(
        oi.call("/templates/remove", json!({ "name": "demo" }))
            .unwrap()["removed"],
        true
    );
    assert_eq!(oi.call("/templates/list", json!({})).unwrap(), json!([]));

    let (code, _) = oi
        .call("/templates/remove", json!({ "name": "demo" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify template.preview]
#[test]
fn template_preview_from_body_and_name() {
    let oi = TestOi::new();

    let (code, msg) = oi
        .call(
            "/templates/preview",
            json!({ "name": "demo", "body": TEMPLATE_BODY }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("exactly one"), "{msg}");

    let (code, _) = oi.call("/templates/preview", json!({})).unwrap_err();
    assert_eq!(code, "requirements_invalid");

    let preview = oi
        .call("/templates/preview", json!({ "body": TEMPLATE_BODY }))
        .unwrap();
    assert_eq!(preview["description"], "demo template");
    assert_eq!(preview["script_error"], json!(null));
    let resources = preview["resources"].as_array().unwrap();
    assert!(
        resources
            .iter()
            .any(|r| r["name"] == "data" && r["type"] == "volume"),
        "{resources:?}"
    );

    oi.call(
        "/templates/create",
        json!({ "name": "demo", "body": TEMPLATE_BODY }),
    )
    .unwrap();
    let preview = oi
        .call("/templates/preview", json!({ "name": "demo" }))
        .unwrap();
    assert_eq!(preview["description"], "demo template");

    let broken = oi
        .call("/templates/preview", json!({ "body": "app.nonsense();" }))
        .unwrap();
    assert!(broken["script_error"].is_string(), "{broken}");

    let (code, _) = oi
        .call("/templates/preview", json!({ "name": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify template.instantiate]
#[test]
fn template_instantiate_registers_app() {
    let oi = TestOi::new();

    let (code, _) = oi
        .call(
            "/templates/instantiate",
            json!({ "template": "ghost", "app": "demo-app" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    oi.call(
        "/templates/create",
        json!({ "name": "demo", "body": TEMPLATE_BODY }),
    )
    .unwrap();
    let result = oi
        .call(
            "/templates/instantiate",
            json!({ "template": "demo", "app": "demo-app" }),
        )
        .unwrap();
    assert_eq!(result["app"], "demo-app");
    assert_eq!(result["generation"], 1);

    let apps = oi.call("/apps/list", json!({})).unwrap();
    assert!(
        apps.to_string().contains("demo-app"),
        "app list should contain the instantiated app: {apps}"
    );

    let (code, msg) = oi
        .call(
            "/templates/instantiate",
            json!({ "template": "demo", "app": "demo-app" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("already registered"), "{msg}");
}
