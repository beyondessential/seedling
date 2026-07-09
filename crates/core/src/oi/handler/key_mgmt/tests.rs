use serde_json::json;

use crate::oi::test_support::TestOi;

// i[verify key.authorize]
// i[verify key.list]
#[test]
fn authorise_then_list_shows_key() {
    let oi = TestOi::new();
    oi.call(
        "/keys/authorise",
        json!({ "fingerprint": "aabbcc", "label": "laptop" }),
    )
    .unwrap();

    let keys = oi.call("/keys/list", json!({})).unwrap();
    let keys = keys.as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["fingerprint"], "aabbcc");
    assert_eq!(keys[0]["label"], "laptop");
    assert!(keys[0]["added_at"].as_i64().unwrap() > 0);
}

// i[verify key.authorize]
#[test]
fn authorise_defaults_label_and_updates_existing() {
    let oi = TestOi::new();
    oi.call("/keys/authorise", json!({ "fingerprint": "aabbcc" }))
        .unwrap();
    let keys = oi.call("/keys/list", json!({})).unwrap();
    assert_eq!(keys[0]["label"], "unnamed");

    // Re-authorising the same fingerprint relabels rather than duplicating.
    oi.call(
        "/keys/authorise",
        json!({ "fingerprint": "aabbcc", "label": "desk" }),
    )
    .unwrap();
    let keys = oi.call("/keys/list", json!({})).unwrap();
    let keys = keys.as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["label"], "desk");
}

// i[verify key.revoke]
#[test]
fn revoke_removes_key_or_errors_when_absent() {
    let oi = TestOi::new();
    oi.call("/keys/authorise", json!({ "fingerprint": "aabbcc" }))
        .unwrap();
    oi.call("/keys/revoke", json!({ "fingerprint": "aabbcc" }))
        .unwrap();
    let keys = oi.call("/keys/list", json!({})).unwrap();
    assert!(keys.as_array().unwrap().is_empty());

    let (code, _) = oi
        .call("/keys/revoke", json!({ "fingerprint": "aabbcc" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}
