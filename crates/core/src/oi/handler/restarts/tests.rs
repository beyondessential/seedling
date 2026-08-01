use serde_json::json;

use crate::{
    oi::test_support::TestOi,
    runtime::restarts::{ExitKind, ExitStatus, Initiator, RestartSubject, record},
};
use seedling_protocol::names::AppName;

fn seed(oi: &TestOi, app: &str, instance: &str, initiator: Initiator, exit: Option<ExitStatus>) {
    let subject = RestartSubject {
        app: AppName::new(app).unwrap(),
        instance_id: instance.to_owned(),
        resource_type: Some("deployment".to_owned()),
        resource_name: Some("web".to_owned()),
        generation: Some(1),
    };
    let at = jiff::Timestamp::now().as_millisecond();
    oi.state
        .db
        .call(move |db| record(db, &subject, initiator, exit, at))
        .expect("record");
}

// i[verify restart.list]
// i[verify restart.record]
#[test]
fn list_returns_records_with_their_exit_and_initiator() {
    let oi = TestOi::new();
    seed(
        &oi,
        "demo",
        "aa",
        Initiator::Supervisor,
        Some(ExitStatus {
            kind: ExitKind::Signalled,
            code: 9,
        }),
    );

    let rows = oi.call("/restarts/list", json!({})).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["app"], "demo");
    assert_eq!(rows[0]["instance_id"], "aa");
    assert_eq!(rows[0]["resource_type"], "deployment");
    assert_eq!(rows[0]["resource_name"], "web");
    assert_eq!(rows[0]["generation"], 1);
    assert_eq!(rows[0]["initiator"], "supervisor");
    assert_eq!(rows[0]["exit_code"], 9);
    assert_eq!(rows[0]["exit_kind"], "signalled");
    assert!(!rows[0]["timestamp"].as_str().unwrap().is_empty());
}

// i[verify restart.record]
#[test]
fn an_unknown_exit_is_reported_as_null_rather_than_invented() {
    let oi = TestOi::new();
    seed(&oi, "demo", "aa", Initiator::Runtime, None);

    let rows = oi.call("/restarts/list", json!({})).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows[0]["initiator"], "runtime");
    assert!(rows[0]["exit_code"].is_null());
    assert!(rows[0]["exit_kind"].is_null());
}

// i[verify restart.list]
#[test]
fn list_filters_by_app_and_instance_and_honours_limit() {
    let oi = TestOi::new();
    seed(&oi, "demo", "aa", Initiator::Supervisor, None);
    seed(&oi, "demo", "aa", Initiator::Supervisor, None);
    seed(&oi, "other", "bb", Initiator::Supervisor, None);

    let by_app = oi.call("/restarts/list", json!({ "app": "demo" })).unwrap();
    assert_eq!(by_app.as_array().unwrap().len(), 2);

    let by_instance = oi
        .call("/restarts/list", json!({ "instance": "bb" }))
        .unwrap();
    let by_instance = by_instance.as_array().unwrap();
    assert_eq!(by_instance.len(), 1);
    assert_eq!(by_instance[0]["app"], "other");

    let both = oi
        .call("/restarts/list", json!({ "app": "demo", "instance": "bb" }))
        .unwrap();
    assert!(both.as_array().unwrap().is_empty());

    let limited = oi.call("/restarts/list", json!({ "limit": 1 })).unwrap();
    assert_eq!(limited.as_array().unwrap().len(), 1);
}

// i[verify restart.settings]
#[test]
fn settings_round_trip_and_reject_out_of_bounds() {
    let oi = TestOi::new();

    let s = oi.call("/restarts/settings/get", json!({})).unwrap();
    assert_eq!(s["threshold"], 5);
    assert_eq!(s["window_secs"], 1800);

    let s = oi
        .call("/restarts/settings/set", json!({ "threshold": 3 }))
        .unwrap();
    assert_eq!(s["threshold"], 3);
    assert_eq!(s["window_secs"], 1800);

    let s = oi
        .call("/restarts/settings/set", json!({ "window_secs": 600 }))
        .unwrap();
    assert_eq!(s["threshold"], 3);
    assert_eq!(s["window_secs"], 600);

    let err = oi
        .call("/restarts/settings/set", json!({ "threshold": 1 }))
        .unwrap_err();
    assert!(err.1.contains("at least"), "{}", err.1);

    let err = oi
        .call("/restarts/settings/set", json!({ "window_secs": 10 }))
        .unwrap_err();
    assert!(err.1.contains("at least"), "{}", err.1);

    // A rejected update leaves the stored settings alone.
    let s = oi.call("/restarts/settings/get", json!({})).unwrap();
    assert_eq!(s["threshold"], 3);
    assert_eq!(s["window_secs"], 600);
}
