use serde_json::json;

use crate::oi::test_support::TestOi;

const BACKUP_APP_SCRIPT: &str = r#"
    app.on_action("save-snapshot", |rt, param| {});
    app.on_action("list-snapshots", |rt, param| {});
    app.on_action("restore-snapshot", |rt, param| {});
"#;

fn register_backup_app(oi: &TestOi, name: &str) {
    oi.call(
        "/apps/create",
        json!({ "app": name, "script": BACKUP_APP_SCRIPT }),
    )
    .expect("app registration succeeds");
    oi.call("/backups/apps/register", json!({ "app": name }))
        .expect("backup app registration succeeds");
}

// i[verify backup.app.register]
// i[verify backup.app.list]
// i[verify backup.app.deregister]
#[test]
fn backup_app_register_list_deregister_roundtrip() {
    let oi = TestOi::new();
    assert_eq!(oi.call("/backups/apps/list", json!({})).unwrap(), json!([]));

    register_backup_app(&oi, "kopia-s3");
    let list = oi.call("/backups/apps/list", json!({})).unwrap();
    assert_eq!(list, json!([{ "app": "kopia-s3" }]));

    let deregistered = oi
        .call("/backups/apps/deregister", json!({ "app": "kopia-s3" }))
        .unwrap();
    assert_eq!(deregistered["deregistered"], true);
    assert_eq!(oi.call("/backups/apps/list", json!({})).unwrap(), json!([]));

    let (code, _) = oi
        .call("/backups/apps/deregister", json!({ "app": "kopia-s3" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify backup.app.validation]
#[test]
fn backup_app_registration_requires_app_and_actions() {
    let oi = TestOi::new();

    let (code, _) = oi
        .call("/backups/apps/register", json!({ "app": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");

    oi.call(
        "/apps/create",
        json!({
            "app": "not-a-backup",
            "script": r#"app.on_action("save-snapshot", |rt, param| {});"#,
        }),
    )
    .unwrap();
    let (code, msg) = oi
        .call("/backups/apps/register", json!({ "app": "not-a-backup" }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("list-snapshots"), "{msg}");
    assert!(msg.contains("restore-snapshot"), "{msg}");
}

// i[verify backup.app.deregister]
#[test]
fn deregister_blocked_while_strategy_references_app() {
    let oi = TestOi::new();
    register_backup_app(&oi, "kopia-s3");
    oi.call(
        "/backups/strategies/create",
        json!({
            "name": "nightly",
            "via": "kopia-s3",
            "schedule": "every day",
            "volumes": ["_site/data"],
        }),
    )
    .unwrap();

    let (code, _) = oi
        .call("/backups/apps/deregister", json!({ "app": "kopia-s3" }))
        .unwrap_err();
    assert_eq!(code, "backup_app_in_use");

    oi.call("/backups/strategies/delete", json!({ "name": "nightly" }))
        .unwrap();
    assert_eq!(
        oi.call("/backups/apps/deregister", json!({ "app": "kopia-s3" }))
            .unwrap()["deregistered"],
        true
    );
}

// i[verify backup.strategy.create]
// i[verify backup.strategy.list]
// i[verify backup.strategy.show]
// i[verify backup.strategy.update]
// i[verify backup.strategy.delete]
#[test]
fn strategy_crud_roundtrip() {
    let oi = TestOi::new();
    register_backup_app(&oi, "kopia-s3");

    oi.call(
        "/backups/strategies/create",
        json!({
            "name": "nightly",
            "via": "kopia-s3",
            "schedule": "every day",
            "volumes": ["_site/data", "my-app/uploads"],
        }),
    )
    .unwrap();

    let shown = oi
        .call("/backups/strategies/show", json!({ "name": "nightly" }))
        .unwrap();
    assert_eq!(shown["via"], "kopia-s3");
    assert_eq!(shown["schedule"], "every day");
    assert_eq!(shown["volumes"], json!(["_site/data", "my-app/uploads"]));
    assert_eq!(shown["last_fired_at"], json!(null));
    assert!(shown["next_fire_at"].is_string(), "{shown}");

    let list = oi.call("/backups/strategies/list", json!({})).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    assert_eq!(
        oi.call(
            "/backups/strategies/update",
            json!({ "name": "nightly", "schedule": "every hour" }),
        )
        .unwrap()["updated"],
        true
    );
    let shown = oi
        .call("/backups/strategies/show", json!({ "name": "nightly" }))
        .unwrap();
    assert_eq!(shown["schedule"], "every hour");

    assert_eq!(
        oi.call("/backups/strategies/delete", json!({ "name": "nightly" }))
            .unwrap()["deleted"],
        true
    );
    let (code, _) = oi
        .call("/backups/strategies/show", json!({ "name": "nightly" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
    let (code, _) = oi
        .call("/backups/strategies/delete", json!({ "name": "nightly" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify backup.strategy.create]
// i[verify backup.strategy.update]
#[test]
fn strategy_validation_errors() {
    let oi = TestOi::new();
    register_backup_app(&oi, "kopia-s3");

    let (code, msg) = oi
        .call(
            "/backups/strategies/create",
            json!({
                "name": "odd",
                "via": "kopia-s3",
                "schedule": "fortnightly",
                "volumes": [],
            }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("every hour"), "{msg}");

    let (code, _) = oi
        .call(
            "/backups/strategies/create",
            json!({
                "name": "odd",
                "via": "unregistered",
                "schedule": "every day",
                "volumes": [],
            }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    let (code, _) = oi
        .call(
            "/backups/strategies/update",
            json!({ "name": "ghost", "schedule": "every day" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    oi.call(
        "/backups/strategies/create",
        json!({
            "name": "real",
            "via": "kopia-s3",
            "schedule": "every day",
            "volumes": [],
        }),
    )
    .unwrap();
    let (code, _) = oi
        .call(
            "/backups/strategies/update",
            json!({ "name": "real", "schedule": "yearly" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    let (code, _) = oi
        .call(
            "/backups/strategies/update",
            json!({ "name": "real", "via": "unregistered" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify backup.run]
#[test]
fn manual_run_stamps_last_fired_and_returns_operations() {
    let oi = TestOi::new();

    let (code, _) = oi
        .call("/backups/run", json!({ "strategy": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");

    register_backup_app(&oi, "kopia-s3");
    oi.call(
        "/backups/strategies/create",
        json!({
            "name": "nightly",
            "via": "kopia-s3",
            "schedule": "every day",
            "volumes": ["_site/absent-one", "_site/absent-two"],
        }),
    )
    .unwrap();

    let ops = oi
        .call("/backups/run", json!({ "strategy": "nightly" }))
        .unwrap();
    let ops = ops.as_array().unwrap().clone();
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0]["volume"], "_site/absent-one");
    assert!(ops[0]["operation_id"].is_string(), "{ops:?}");

    let shown = oi
        .call("/backups/strategies/show", json!({ "name": "nightly" }))
        .unwrap();
    assert!(shown["last_fired_at"].is_string(), "{shown}");
}

// i[verify backup.snapshots.list]
// i[verify backup.restore]
#[test]
fn snapshots_list_and_restore_validate_strategy_and_app() {
    let oi = TestOi::new();

    let (code, _) = oi
        .call(
            "/backups/snapshots/list",
            json!({ "strategy": "ghost", "volume": "_site/data" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    let (code, _) = oi
        .call(
            "/backups/restore",
            json!({ "strategy": "ghost", "volume": "_site/data", "snapshot": "s1" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    // A strategy whose backing app has vanished from the registry fails
    // fire-time validation rather than dispatching the action.
    register_backup_app(&oi, "kopia-s3");
    oi.call(
        "/backups/strategies/create",
        json!({
            "name": "nightly",
            "via": "kopia-s3",
            "schedule": "every day",
            "volumes": ["_site/data"],
        }),
    )
    .unwrap();
    oi.call("/apps/remove", json!({ "app": "kopia-s3" }))
        .expect("BSL app deregistration succeeds");

    let (code, msg) = oi
        .call(
            "/backups/snapshots/list",
            json!({ "strategy": "nightly", "volume": "_site/data" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("missing required backup actions"), "{msg}");
}
