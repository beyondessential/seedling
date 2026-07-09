use serde_json::{Value, json};

use crate::oi::test_support::TestOi;

fn register_app(oi: &TestOi, name: &str, script: &str) {
    oi.call("/apps/create", json!({ "app": name, "script": script }))
        .expect("app registration succeeds");
}

fn site_volume_names(oi: &TestOi) -> Vec<String> {
    let list = oi.call("/volumes/site/list", json!({})).unwrap();
    list.as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap().to_owned())
        .collect()
}

fn held_volumes(oi: &TestOi) -> Vec<Value> {
    oi.call("/volumes/held/list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

// r[verify volume.site.lifecycle]
#[test]
fn managed_site_volume_create_list_delete_holds_data() {
    let oi = TestOi::new();
    let created = oi
        .call(
            "/volumes/site/create",
            json!({ "name": "app-data", "kind": "managed" }),
        )
        .unwrap();
    assert_eq!(created["created"], true);

    let list = oi.call("/volumes/site/list", json!({})).unwrap();
    let entry = &list.as_array().unwrap()[0];
    assert_eq!(entry["name"], "app-data");
    assert_eq!(entry["kind"], "managed");

    // r[verify actuate.volume.hold]
    let deleted = oi
        .call("/volumes/site/delete", json!({ "name": "app-data" }))
        .unwrap();
    assert_eq!(deleted["deleted"], true);
    assert!(site_volume_names(&oi).is_empty());

    let held = held_volumes(&oi);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0]["app"], "_site");
    assert_eq!(held[0]["volume_name"], "app-data");
}

// r[verify volume.site.lifecycle]
#[test]
fn bind_site_volume_deletion_skips_hold() {
    let oi = TestOi::new();
    oi.call(
        "/volumes/site/create",
        json!({ "name": "host-mount", "kind": "bind", "host_path": "/srv/data" }),
    )
    .unwrap();

    let list = oi.call("/volumes/site/list", json!({})).unwrap();
    let entry = &list.as_array().unwrap()[0];
    assert_eq!(entry["kind"], "bind");
    assert_eq!(entry["host_path"], "/srv/data");

    oi.call("/volumes/site/delete", json!({ "name": "host-mount" }))
        .unwrap();
    assert!(site_volume_names(&oi).is_empty());
    assert!(held_volumes(&oi).is_empty());
}

// r[verify volume.site.lifecycle]
#[test]
fn create_site_volume_rejects_invalid_params() {
    let oi = TestOi::new();

    let (code, msg) = oi
        .call(
            "/volumes/site/create",
            json!({ "name": "bad-bind", "kind": "bind" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("host_path"), "{msg}");

    let (code, msg) = oi
        .call(
            "/volumes/site/create",
            json!({ "name": "bad-bind", "kind": "bind", "host_path": "relative/path" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("absolute"), "{msg}");

    let (code, msg) = oi
        .call(
            "/volumes/site/create",
            json!({ "name": "bad-kind", "kind": "floating" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("floating"), "{msg}");
}

// r[verify volume.site.lifecycle]
#[test]
fn delete_missing_site_volume_is_rejected() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call("/volumes/site/delete", json!({ "name": "no-such" }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
}

// r[verify actuate.volume.hold.restore]
#[test]
fn held_site_volume_restores_to_managed_volume() {
    let oi = TestOi::new();
    oi.call(
        "/volumes/site/create",
        json!({ "name": "keep-me", "kind": "managed" }),
    )
    .unwrap();
    oi.call("/volumes/site/delete", json!({ "name": "keep-me" }))
        .unwrap();
    let held_id = held_volumes(&oi)[0]["id"].clone();

    let restored = oi
        .call("/volumes/held/restore", json!({ "id": held_id }))
        .unwrap();
    assert_eq!(restored["restored"], true);
    assert_eq!(restored["name"], "keep-me");
    assert!(held_volumes(&oi).is_empty());
    assert_eq!(site_volume_names(&oi), vec!["keep-me"]);
}

// r[verify actuate.volume.hold.restore]
#[test]
fn restore_held_rejects_missing_id_and_name_collision() {
    let oi = TestOi::new();
    let (code, msg) = oi
        .call(
            "/volumes/held/restore",
            json!({ "id": "00000000-0000-0000-0000-000000000000" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("no held volume"), "{msg}");

    oi.call(
        "/volumes/site/create",
        json!({ "name": "clash", "kind": "managed" }),
    )
    .unwrap();
    oi.call("/volumes/site/delete", json!({ "name": "clash" }))
        .unwrap();
    // Recreate under the same name, then try restoring the held copy.
    oi.call(
        "/volumes/site/create",
        json!({ "name": "clash", "kind": "managed" }),
    )
    .unwrap();
    let held_id = held_volumes(&oi)[0]["id"].clone();
    let (code, msg) = oi
        .call("/volumes/held/restore", json!({ "id": held_id }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("already exists"), "{msg}");
}

// r[verify actuate.volume.hold.confirm]
#[test]
fn held_volume_delete_confirms_removal() {
    let oi = TestOi::new();
    oi.call(
        "/volumes/site/create",
        json!({ "name": "doomed", "kind": "managed" }),
    )
    .unwrap();
    oi.call("/volumes/site/delete", json!({ "name": "doomed" }))
        .unwrap();
    let held_id = held_volumes(&oi)[0]["id"].clone();

    let deleted = oi
        .call("/volumes/held/delete", json!({ "id": held_id }))
        .unwrap();
    assert_eq!(deleted["deleted"], true);
    assert!(held_volumes(&oi).is_empty());

    let (code, _) = oi
        .call(
            "/volumes/held/delete",
            json!({ "id": "00000000-0000-0000-0000-000000000000" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
}

// r[verify volume.site.snapshot]
#[test]
fn snapshot_source_validation() {
    let oi = TestOi::new();

    let (code, msg) = oi
        .call(
            "/volumes/site/snapshot",
            json!({ "name": "snap-one", "source": "noslash" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("expected"), "{msg}");

    let (code, _) = oi
        .call(
            "/volumes/site/snapshot",
            json!({ "name": "snap-one", "source": "_site/" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    let (code, msg) = oi
        .call(
            "/volumes/site/snapshot",
            json!({ "name": "snap-one", "source": "ghost-app/data" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("ghost-app/data"), "{msg}");
}

// r[verify volume.site.promote]
#[test]
fn promote_requires_existing_snapshot_source() {
    let oi = TestOi::new();

    let (code, msg) = oi
        .call(
            "/volumes/site/promote",
            json!({ "source": "no-such", "name": "fresh" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("no site volume"), "{msg}");

    oi.call(
        "/volumes/site/create",
        json!({ "name": "not-snap", "kind": "managed" }),
    )
    .unwrap();
    let (code, msg) = oi
        .call(
            "/volumes/site/promote",
            json!({ "source": "not-snap", "name": "fresh" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("not a snapshot"), "{msg}");
}

#[test]
fn app_and_exported_volume_lists_reflect_registered_apps() {
    let oi = TestOi::new();
    assert_eq!(oi.call("/volumes/app/list", json!({})).unwrap(), json!([]));
    assert_eq!(
        oi.call("/volumes/exported/list", json!({})).unwrap(),
        json!([])
    );

    register_app(
        &oi,
        "vol-app",
        r#"
        app.volume("shared").exported(#{ description: "public data" });
        app.volume("scratch").tmpfs();
        app.volume("internal");
        "#,
    );

    let app_vols = oi.call("/volumes/app/list", json!({})).unwrap();
    let app_vols = app_vols.as_array().unwrap();
    let names: Vec<&str> = app_vols
        .iter()
        .map(|v| v["volume_name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"shared"));
    assert!(names.contains(&"internal"));
    assert!(!names.contains(&"scratch"), "tmpfs volumes are excluded");
    let shared = app_vols
        .iter()
        .find(|v| v["volume_name"] == "shared")
        .unwrap();
    assert_eq!(shared["exported"], true);
    assert_eq!(shared["description"], "public data");

    let exported = oi.call("/volumes/exported/list", json!({})).unwrap();
    let exported = exported.as_array().unwrap();
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0]["volume_name"], "shared");
    assert_eq!(exported[0]["app"], "vol-app");
}

// r[verify volume.external.mapping.events]
#[test]
fn external_volume_mapping_map_remap_unmap_flow() {
    let oi = TestOi::new();

    let mapped = oi
        .call(
            "/volumes/external/map",
            json!({
                "app": "consumer",
                "external_name": "backing",
                "target": { "kind": "site", "name": "store" },
                "read_only": true,
            }),
        )
        .unwrap();
    assert_eq!(mapped["mapped"], true);

    let list = oi.call("/volumes/external/list", json!({})).unwrap();
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["app"], "consumer");
    assert_eq!(list[0]["external_name"], "backing");
    assert_eq!(list[0]["read_only"], true);
    assert_eq!(list[0]["target"]["kind"], "site");

    let filtered = oi
        .call("/volumes/external/list", json!({ "app": "other-app" }))
        .unwrap();
    assert_eq!(filtered, json!([]));

    let remapped = oi
        .call(
            "/volumes/external/remap",
            json!({
                "app": "consumer",
                "external_name": "backing",
                "target": { "kind": "app", "app": "producer", "volume": "data" },
            }),
        )
        .unwrap();
    assert_eq!(remapped["remapped"], true);
    let list = oi.call("/volumes/external/list", json!({})).unwrap();
    assert_eq!(list[0]["target"]["kind"], "app");
    assert_eq!(list[0]["read_only"], false);

    let unmapped = oi
        .call(
            "/volumes/external/unmap",
            json!({ "app": "consumer", "external_name": "backing" }),
        )
        .unwrap();
    assert_eq!(unmapped["unmapped"], true);

    let (code, _) = oi
        .call(
            "/volumes/external/unmap",
            json!({ "app": "consumer", "external_name": "backing" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    let (code, _) = oi
        .call(
            "/volumes/external/remap",
            json!({
                "app": "consumer",
                "external_name": "backing",
                "target": { "kind": "site", "name": "store" },
            }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
}

#[test]
fn declared_external_volumes_guard_unmap() {
    let oi = TestOi::new();
    assert_eq!(
        oi.call("/volumes/external/declared", json!({})).unwrap(),
        json!([])
    );

    register_app(&oi, "db-user", r#"app.external_volume("db-files");"#);

    let declared = oi.call("/volumes/external/declared", json!({})).unwrap();
    let declared = declared.as_array().unwrap();
    assert_eq!(declared.len(), 1);
    assert_eq!(declared[0]["app"], "db-user");
    assert_eq!(declared[0]["name"], "db-files");

    oi.call(
        "/volumes/external/map",
        json!({
            "app": "db-user",
            "external_name": "db-files",
            "target": { "kind": "site", "name": "pgdata" },
        }),
    )
    .unwrap();

    let (code, msg) = oi
        .call(
            "/volumes/external/unmap",
            json!({ "app": "db-user", "external_name": "db-files" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("declared by app"), "{msg}");
}
