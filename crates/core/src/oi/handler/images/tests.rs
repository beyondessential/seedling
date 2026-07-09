use serde_json::json;

use crate::oi::test_support::TestOi;

const REF: &str = "docker.io/library/nginx:1.29";

// i[verify image.pull]
// i[verify image.list]
#[test]
fn pull_then_list_shows_image() {
    let oi = TestOi::new();
    oi.call("/images/pull", json!({ "reference": REF }))
        .unwrap();

    let result = oi.call("/images/list", json!({})).unwrap();
    let images = result["images"].as_array().unwrap();
    assert_eq!(images.len(), 1);
    let tags = images[0]["tags"].as_array().unwrap();
    assert_eq!(tags[0], REF);
    assert_eq!(images[0]["in_use"], false);
    assert!(images[0]["pinned_by"].as_array().unwrap().is_empty());
}

// i[verify image.pull]
#[test]
fn pull_rejects_empty_reference_and_unknown_app() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call("/images/pull", json!({ "reference": "  " }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    let (code, _) = oi
        .call("/images/pull", json!({ "reference": REF, "app": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify image.pull]
// i[verify image.pin.list]
#[test]
fn pull_for_app_creates_pin() {
    let oi = TestOi::with_app("demo");
    oi.call("/images/pull", json!({ "reference": REF, "app": "demo" }))
        .unwrap();

    let result = oi.call("/images/pins/list", json!({})).unwrap();
    let pins = result["pins"].as_array().unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0]["app"], "demo");
    assert_eq!(pins[0]["reference"], REF);

    // The pinning app also shows up against the image in /images/list.
    let result = oi.call("/images/list", json!({})).unwrap();
    let pinned_by = result["images"][0]["pinned_by"].as_array().unwrap();
    assert_eq!(pinned_by.len(), 1);
    assert_eq!(pinned_by[0], "demo");
}

// i[verify image.pin.list]
#[test]
fn pins_list_filters_by_app_and_validates_name() {
    let oi = TestOi::with_app("demo");
    oi.call("/images/pull", json!({ "reference": REF, "app": "demo" }))
        .unwrap();

    let result = oi
        .call("/images/pins/list", json!({ "app": "demo" }))
        .unwrap();
    assert_eq!(result["pins"].as_array().unwrap().len(), 1);

    let (code, _) = oi
        .call("/images/pins/list", json!({ "app": "_bad_" }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
}

// i[verify image.pin.clear]
#[test]
fn pins_clear_removes_pins_for_app() {
    let oi = TestOi::with_app("demo");
    oi.call("/images/pull", json!({ "reference": REF, "app": "demo" }))
        .unwrap();
    oi.call("/images/pins/clear", json!({ "app": "demo" }))
        .unwrap();

    let result = oi.call("/images/pins/list", json!({})).unwrap();
    assert!(result["pins"].as_array().unwrap().is_empty());
}

// i[verify image.remove]
#[test]
fn remove_deletes_pulled_image() {
    let oi = TestOi::new();
    oi.call("/images/pull", json!({ "reference": REF }))
        .unwrap();
    oi.call("/images/remove", json!({ "reference": REF }))
        .unwrap();

    let result = oi.call("/images/list", json!({})).unwrap();
    assert!(result["images"].as_array().unwrap().is_empty());
}

// i[verify image.remove]
#[test]
fn remove_unknown_image_is_not_found() {
    let oi = TestOi::new();
    let (code, message) = oi
        .call(
            "/images/remove",
            json!({ "reference": "docker.io/nope:latest" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
    assert!(message.contains("not found locally"), "message: {message}");
}

// i[verify image.discover]
#[test]
fn discover_returns_empty_probe_for_handlerless_app() {
    let oi = TestOi::with_app("demo");
    let result = oi
        .call("/apps/images/discover", json!({ "app": "demo" }))
        .unwrap();
    assert!(result["per_handler"].as_array().unwrap().is_empty());
    assert!(result["all_images"].as_array().unwrap().is_empty());

    let (code, _) = oi
        .call("/apps/images/discover", json!({ "app": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}
