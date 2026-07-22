use serde_json::json;

use crate::oi::test_support::TestOi;

// i[verify canopy.status]
#[test]
fn status_reports_not_enrolled() {
    let oi = TestOi::new();
    let result = oi.call("/canopy/status", json!({})).unwrap();
    assert_eq!(result["enrolled"], json!(false));
    assert!(result.get("server_id").is_none());
    assert!(result.get("last_push_at").is_none());
}
