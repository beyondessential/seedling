use serde_json::json;

use crate::oi::test_support::TestOi;

// i[verify key.authorize]
// i[verify key.list]
#[test]
fn authorise_then_list_shows_key() {
    let oi = TestOi::new();
    oi.call(
        "/keys/authorise",
        json!({ "fingerprint": "aabbcc0000000000000000000000000000000000000000000000000000000000", "label": "laptop" }),
    )
    .unwrap();

    let keys = oi.call("/keys/list", json!({})).unwrap();
    let keys = keys.as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(
        keys[0]["fingerprint"],
        "aabbcc0000000000000000000000000000000000000000000000000000000000"
    );
    assert_eq!(keys[0]["label"], "laptop");
    assert!(keys[0]["added_at"].as_i64().unwrap() > 0);
}

// i[verify key.authorize]
#[test]
fn authorise_defaults_label_and_updates_existing() {
    let oi = TestOi::new();
    oi.call("/keys/authorise", json!({ "fingerprint": "aabbcc0000000000000000000000000000000000000000000000000000000000" }))
        .unwrap();
    let keys = oi.call("/keys/list", json!({})).unwrap();
    assert_eq!(keys[0]["label"], "unnamed");

    // Re-authorising the same fingerprint relabels rather than duplicating.
    oi.call(
        "/keys/authorise",
        json!({ "fingerprint": "aabbcc0000000000000000000000000000000000000000000000000000000000", "label": "desk" }),
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
    oi.call("/keys/authorise", json!({ "fingerprint": "aabbcc0000000000000000000000000000000000000000000000000000000000" }))
        .unwrap();
    oi.call("/keys/revoke", json!({ "fingerprint": "aabbcc0000000000000000000000000000000000000000000000000000000000" }))
        .unwrap();
    let keys = oi.call("/keys/list", json!({})).unwrap();
    assert!(keys.as_array().unwrap().is_empty());

    let (code, _) = oi
        .call("/keys/revoke", json!({ "fingerprint": "aabbcc0000000000000000000000000000000000000000000000000000000000" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify key.authorize]
// A fingerprint the verifier can never match is not a near miss: storing it
// tells an operator they granted access they did not.
#[test]
fn authorise_refuses_a_fingerprint_that_could_never_match() {
    let oi = TestOi::new();
    for bad in ["aabbcc", "sha256:", "not a fingerprint", "zz"] {
        let (code, _) = oi
            .call("/keys/authorise", json!({ "fingerprint": bad }))
            .unwrap_err();
        assert_eq!(code, "requirements_invalid", "should refuse {bad:?}");
    }
    assert!(
        oi.call("/keys/list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
}

// i[verify key.authorize]
#[test]
fn authorise_normalises_case_and_a_sha256_prefix() {
    let oi = TestOi::new();
    let canonical = format!("aabbcc{}", "0".repeat(58));
    oi.call(
        "/keys/authorise",
        json!({ "fingerprint": format!("SHA256:{}", canonical.to_uppercase()) }),
    )
    .unwrap();
    let keys = oi.call("/keys/list", json!({})).unwrap();
    assert_eq!(keys[0]["fingerprint"], canonical);
}

// i[verify key.revoke]
// The same key must be revocable with the same string it was authorised
// with, whatever case or prefix that was in.
#[test]
fn revoke_accepts_the_form_the_key_was_authorised_with() {
    let oi = TestOi::new();
    let canonical = format!("aabbcc{}", "0".repeat(58));
    oi.call(
        "/keys/authorise",
        json!({ "fingerprint": format!("SHA256:{}", canonical.to_uppercase()) }),
    )
    .unwrap();
    oi.call(
        "/keys/revoke",
        json!({ "fingerprint": format!("SHA256:{}", canonical.to_uppercase()) }),
    )
    .unwrap();
    assert!(
        oi.call("/keys/list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
}
