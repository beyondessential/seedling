use serde_json::{Value, json};

use crate::oi::test_support::TestOi;

fn register_app(oi: &TestOi, name: &str, script: &str) {
    oi.call("/apps/create", json!({ "app": name, "script": script }))
        .expect("app registration succeeds");
}

fn make_site_volume(oi: &TestOi, name: &str) {
    oi.call(
        "/volumes/site/create",
        json!({ "name": name, "kind": "managed" }),
    )
    .unwrap();
}

fn site_volume_names(oi: &TestOi) -> Vec<String> {
    let list = oi.call("/volumes/site/list", json!({})).unwrap();
    list.as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap().to_owned())
        .collect()
}

/// A snapshot site volume's record, for a host that cannot make a real one.
fn record_snapshot(oi: &TestOi, name: &str) {
    use crate::runtime::site_volumes::{self, SiteVolumeDef, SiteVolumeKind};
    use seedling_protocol::names::{SiteVolumeName, VolumeRef};

    let def = SiteVolumeDef {
        name: SiteVolumeName::new(name).unwrap(),
        kind: SiteVolumeKind::Snapshot {
            source: VolumeRef::Site {
                name: SiteVolumeName::new("origin").unwrap(),
            },
        },
        created_at: jiff::Timestamp::now().to_string(),
    };
    oi.state
        .db
        .call(move |db| site_volumes::create(db, &def))
        .unwrap();
}

fn held_volumes(oi: &TestOi) -> Vec<Value> {
    oi.call("/volumes/held/list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

// spec: SITE#site-volumes
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

    // spec: ACTU#held-volumes
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

// spec: SITE#site-volumes
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

// spec: SITE#site-volumes
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

// spec: SITE#site-volumes
#[test]
fn delete_missing_site_volume_is_rejected() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call("/volumes/site/delete", json!({ "name": "no-such" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// spec: ACTU#held-volumes
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

// spec: ACTU#held-volumes
#[test]
fn restore_held_rejects_missing_id_and_name_collision() {
    let oi = TestOi::new();
    let (code, msg) = oi
        .call(
            "/volumes/held/restore",
            json!({ "id": "00000000-0000-0000-0000-000000000000" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
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

// spec: ACTU#held-volumes
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
    assert_eq!(code, "not_found");
}

// spec: SITE#site-volume-snapshots
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
    assert_eq!(code, "not_found");
    assert!(msg.contains("ghost-app/data"), "{msg}");
}

// spec: SITE#snapshot-promotion
#[test]
fn promote_requires_existing_snapshot_source() {
    let oi = TestOi::new();

    let (code, msg) = oi
        .call(
            "/volumes/site/promote",
            json!({ "source": "no-such", "name": "fresh" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
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

// spec: IDN#reserved-names
#[test]
fn a_mapping_cannot_be_made_under_a_name_operation_volumes_use() {
    let oi = TestOi::new();
    register_app(&oi, "consumer", r#"app.external_volume("backing");"#);
    make_site_volume(&oi, "store");
    let (code, _) = oi
        .call(
            "/volumes/external/map",
            json!({
                "app": "consumer",
                "external_name": "seedling-op-abcd1234-data",
                "target": { "kind": "site", "name": "store" },
            }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
}

// spec: SITE#external-mappings
#[test]
fn external_volume_mapping_map_remap_unmap_flow() {
    let oi = TestOi::new();
    register_app(&oi, "consumer", "");
    register_app(
        &oi,
        "producer",
        r#"app.volume("data").exported(#{ description: "shared" }); app.volume("private");"#,
    );
    make_site_volume(&oi, "store");

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
    assert_eq!(code, "not_found");

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
    assert_eq!(code, "not_found");
}

#[test]
fn declared_external_volumes_guard_unmap() {
    let oi = TestOi::new();
    assert_eq!(
        oi.call("/volumes/external/declared", json!({})).unwrap(),
        json!([])
    );

    register_app(&oi, "db-user", r#"app.external_volume("db-files");"#);
    make_site_volume(&oi, "pgdata");

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

/// btrfs snapshots *into* a destination that already exists rather than
/// refusing it, so the name must be refused before btrfs is reached. The
/// error distinguishes the two: reaching btrfs on a non-btrfs test host
/// would report an internal failure instead.
// spec: SITE#site-volume-names
#[test]
fn snapshotting_onto_a_name_already_taken_is_refused_before_any_btrfs_call() {
    let oi = TestOi::new();
    oi.call(
        "/volumes/site/create",
        json!({ "name": "data", "kind": "managed" }),
    )
    .unwrap();

    let (code, msg) = oi
        .call(
            "/volumes/site/snapshot",
            json!({ "name": "data", "source": "_site/data" }),
        )
        .unwrap_err();

    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("already exists"), "{msg}");
    assert_eq!(site_volume_names(&oi), vec!["data"]);
}

// spec: SITE#site-volume-names
#[test]
fn promoting_onto_a_name_already_taken_is_refused() {
    let oi = TestOi::new();
    oi.call(
        "/volumes/site/create",
        json!({ "name": "data", "kind": "managed" }),
    )
    .unwrap();

    record_snapshot(&oi, "some-snapshot");

    let (code, msg) = oi
        .call(
            "/volumes/site/promote",
            json!({ "source": "some-snapshot", "name": "data" }),
        )
        .unwrap_err();

    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("already exists"), "{msg}");
}

// spec: SITE#site-volume-names
#[test]
fn creating_a_site_volume_under_a_name_already_taken_is_refused() {
    let oi = TestOi::new();
    oi.call(
        "/volumes/site/create",
        json!({ "name": "data", "kind": "managed" }),
    )
    .unwrap();

    let (code, msg) = oi
        .call(
            "/volumes/site/create",
            json!({ "name": "data", "kind": "managed" }),
        )
        .unwrap_err();

    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("already exists"), "{msg}");
}

/// Storage with no record could be a leftover or an operator's volume whose
/// record was never written, and the runtime cannot tell which.
// spec: SITE#site-volume-names
#[test]
fn storage_standing_at_the_name_with_no_record_is_refused_rather_than_reused() {
    let oi = TestOi::new();
    let path = oi.state.driver.volume_store.site_path("unrecorded");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("data.db"), b"somebody's data").unwrap();

    let (code, msg) = oi
        .call(
            "/volumes/site/create",
            json!({ "name": "unrecorded", "kind": "managed" }),
        )
        .unwrap_err();

    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("no site volume recorded"), "{msg}");
    assert!(
        path.join("data.db").exists(),
        "the data must be left where it is"
    );
}

/// Storage removed by hand or by a disk recovery leaves a row with nothing
/// behind it. There is nothing
/// to hold, so there is nothing stopping the deletion.
// spec: ACTU#held-volumes
#[test]
fn deleting_a_managed_site_volume_whose_storage_is_gone_removes_the_record() {
    let oi = TestOi::new();
    oi.call(
        "/volumes/site/create",
        json!({ "name": "mydata", "kind": "managed" }),
    )
    .unwrap();
    std::fs::remove_dir_all(oi.state.driver.volume_store.site_path("mydata")).unwrap();

    let deleted = oi
        .call("/volumes/site/delete", json!({ "name": "mydata" }))
        .unwrap();

    assert_eq!(deleted["deleted"], true);
    assert!(site_volume_names(&oi).is_empty());
    assert!(
        held_volumes(&oi).is_empty(),
        "nothing was preserved because there was nothing to preserve"
    );
}

/// The data is real and would be orphaned, so a hold that could not be
/// carried out must still stop the delete.
// spec: ACTU#held-volumes
#[test]
fn deleting_a_managed_site_volume_that_cannot_be_held_leaves_the_record_alone() {
    let oi = TestOi::new();
    oi.call(
        "/volumes/site/create",
        json!({ "name": "relocated", "kind": "managed" }),
    )
    .unwrap();

    // Storage an operator moved aside and linked back into place: the
    // runtime does not own it, so it refuses to move it.
    let path = oi.state.driver.volume_store.site_path("relocated");
    let elsewhere = path.with_file_name("elsewhere");
    std::fs::rename(&path, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();

    let (code, msg) = oi
        .call("/volumes/site/delete", json!({ "name": "relocated" }))
        .unwrap_err();

    assert_eq!(code, "internal");
    assert!(msg.contains("failed to hold site volume"), "{msg}");
    assert_eq!(site_volume_names(&oi), vec!["relocated"]);
    assert!(elsewhere.exists(), "the data must be left where it is");
}

/// Operator commands are served concurrently, so finding the name free and
/// taking it has to be one step. Standing in as the other request, this
/// holds the claim and shows that a create cannot get past the check while
/// it is held — the point at which, unguarded, it would have gone on to
/// make storage under a name someone else was already taking.
// spec: SITE#site-volume-names
#[test]
fn taking_a_site_volume_name_excludes_another_request_taking_the_same_one() {
    let oi = TestOi::new();
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();

    let claim = oi.state.driver.volume_store.claim_site_name();

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let created = oi
                .call(
                    "/volumes/site/create",
                    json!({ "name": "contended", "kind": "managed" }),
                )
                .unwrap();
            reached_tx.send(created).unwrap();
        });

        assert!(
            reached_rx
                .recv_timeout(std::time::Duration::from_millis(250))
                .is_err(),
            "the create must wait for the claim rather than check the name beside its holder"
        );

        drop(claim);
        let created = reached_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the create proceeds once the claim is released");
        assert_eq!(created["created"], true);
    });

    assert_eq!(site_volume_names(&oi), vec!["contended".to_owned()]);
}

// spec: VOLM#restoring-a-held-volume
// spec: SITE#site-volumes
// A restore brings the site volume into being with its own event, and the
// runtime does not emit a creation event for it as well.
#[test]
fn restoring_a_held_volume_emits_no_site_volume_creation_event() {
    let oi = TestOi::new();
    make_site_volume(&oi, "keep-me");
    oi.call("/volumes/site/delete", json!({ "name": "keep-me" }))
        .unwrap();
    let held_id = held_volumes(&oi)[0]["id"].clone();

    let mut events = oi.state.event_tx.subscribe();
    oi.call("/volumes/held/restore", json!({ "id": held_id }))
        .unwrap();

    let mut seen = Vec::new();
    while let Ok(e) = events.try_recv() {
        seen.push(
            serde_json::to_value(&e).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert!(seen.contains(&"HeldVolumeRestored".to_owned()), "{seen:?}");
    assert!(!seen.contains(&"SiteVolumeCreated".to_owned()), "{seen:?}");
}

// spec: VOLM#requests
// A restore whose data cannot be moved leaves the volume held and nothing
// recorded under the target name.
#[test]
fn a_restore_that_cannot_move_the_data_leaves_the_volume_held_and_unrecorded() {
    let oi = TestOi::new();
    make_site_volume(&oi, "keep-me");
    oi.call("/volumes/site/delete", json!({ "name": "keep-me" }))
        .unwrap();
    let held = held_volumes(&oi);
    let id: seedling_protocol::names::HeldVolumeId =
        held[0]["id"].as_str().unwrap().parse().unwrap();

    // The held data is gone from where its record says, so the move fails.
    let data = oi.state.driver.volume_store.held_path(&id).unwrap();
    std::fs::remove_dir_all(data).unwrap();

    let (code, _) = oi
        .call("/volumes/held/restore", json!({ "id": id }))
        .unwrap_err();
    assert_eq!(code, "internal");
    assert_eq!(held_volumes(&oi).len(), 1, "still held");
    assert!(site_volume_names(&oi).is_empty(), "no record was left");
    assert!(!oi.state.driver.volume_store.site_exists("keep-me"));
}

// spec: VOLM#listing-held-volumes
#[test]
fn a_held_volume_record_that_cannot_be_read_fails_the_list_and_is_not_skipped() {
    let oi = TestOi::new();
    make_site_volume(&oi, "alpha");
    oi.call("/volumes/site/delete", json!({ "name": "alpha" }))
        .unwrap();
    let id = held_volumes(&oi)[0]["id"].as_str().unwrap().to_owned();
    let held_dir = oi
        .state
        .driver
        .volume_store
        .volumes_dir()
        .parent()
        .unwrap()
        .join("held-volumes");
    std::fs::write(held_dir.join(format!("{id}.meta.json")), "{ not json").unwrap();

    let (code, _) = oi.call("/volumes/held/list", json!({})).unwrap_err();
    assert_eq!(code, "internal");
    let (code, _) = oi
        .call("/volumes/held/delete", json!({ "id": id }))
        .unwrap_err();
    assert_eq!(code, "internal", "a record that exists is not 'not found'");
}

// spec: VOLM#listing-held-volumes
#[test]
fn held_volumes_are_listed_by_app_then_volume_name_then_id() {
    let oi = TestOi::new();
    for name in ["zeta", "alpha", "mid"] {
        make_site_volume(&oi, name);
        oi.call("/volumes/site/delete", json!({ "name": name }))
            .unwrap();
    }
    let names: Vec<String> = held_volumes(&oi)
        .iter()
        .map(|h| h["volume_name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["alpha", "mid", "zeta"]);
}

// spec: VOLM#creating-a-site-volume
#[test]
fn a_managed_site_volume_with_a_host_path_is_refused() {
    let oi = TestOi::new();
    let (code, msg) = oi
        .call(
            "/volumes/site/create",
            json!({ "name": "store", "kind": "managed", "host_path": "/srv/store" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("host_path"), "{msg}");
    assert!(site_volume_names(&oi).is_empty());
}

// spec: VOLM#creating-a-site-volume
// spec: VOLM#requests
// Storage that cannot be made leaves no record behind.
#[test]
fn a_managed_site_volume_whose_storage_cannot_be_made_leaves_no_record() {
    let oi = TestOi::new();
    let volumes = oi.state.driver.volume_store.volumes_dir().to_owned();
    // A file where the volumes directory belongs makes the storage fail.
    std::fs::remove_dir_all(&volumes).unwrap();
    std::fs::write(&volumes, "").unwrap();

    let (code, _) = oi
        .call(
            "/volumes/site/create",
            json!({ "name": "store", "kind": "managed" }),
        )
        .unwrap_err();
    assert_eq!(code, "internal");
    assert!(site_volume_names(&oi).is_empty());
}

// spec: VOLM#snapshotting-a-volume
#[test]
fn snapshot_sources_are_checked_before_the_host_is() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call(
            "/volumes/site/snapshot",
            json!({ "name": "snap", "source": "_site/ghost" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    oi.call(
        "/volumes/site/create",
        json!({ "name": "bound", "kind": "bind", "host_path": "/srv/bound" }),
    )
    .unwrap();
    let (code, _) = oi
        .call(
            "/volumes/site/snapshot",
            json!({ "name": "snap", "source": "_site/bound" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    record_snapshot(&oi, "snapped");
    let (code, _) = oi
        .call(
            "/volumes/site/snapshot",
            json!({ "name": "snap", "source": "_site/snapped" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    // A managed site volume on a host that is not on BTRFS.
    make_site_volume(&oi, "managed");
    let (code, msg) = oi
        .call(
            "/volumes/site/snapshot",
            json!({ "name": "snap", "source": "_site/managed" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("BTRFS"), "{msg}");
    assert!(!oi.state.driver.volume_store.site_exists("snap"));
}

// spec: VOLM#promoting-a-snapshot
#[test]
fn promotion_on_a_host_not_on_btrfs_is_refused_without_touching_storage() {
    let oi = TestOi::new();
    record_snapshot(&oi, "snapped");
    let (code, msg) = oi
        .call(
            "/volumes/site/promote",
            json!({ "source": "snapped", "name": "fresh" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("BTRFS"), "{msg}");
    assert!(!oi.state.driver.volume_store.site_exists("fresh"));
    assert_eq!(site_volume_names(&oi), vec!["snapped"]);
}

// spec: VOLM#snapshotting-a-volume
// A history that cannot be read is not "no volume".
#[test]
fn an_unreadable_volume_history_is_internal_not_not_found() {
    let oi = TestOi::new();
    oi.state
        .db
        .call(|db| db.conn.execute_batch("DROP TABLE resource_instances;"))
        .unwrap();
    let (code, _) = oi
        .call(
            "/volumes/site/snapshot",
            json!({ "name": "snap", "source": "ghost-app/data" }),
        )
        .unwrap_err();
    assert_eq!(code, "internal");
}

// spec: VOLM#deleting-a-site-volume
// spec: SITE#site-volumes
#[test]
fn a_site_volume_a_mapping_targets_cannot_be_deleted() {
    let oi = TestOi::new();
    register_app(&oi, "consumer", "");
    make_site_volume(&oi, "store");
    oi.call(
        "/volumes/external/map",
        json!({
            "app": "consumer",
            "external_name": "backing",
            "target": { "kind": "site", "name": "store" },
        }),
    )
    .unwrap();

    let (code, msg) = oi
        .call("/volumes/site/delete", json!({ "name": "store" }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("consumer") && msg.contains("backing"), "{msg}");
    assert_eq!(site_volume_names(&oi), vec!["store"]);
    assert!(held_volumes(&oi).is_empty(), "nothing was held");

    oi.call(
        "/volumes/external/unmap",
        json!({ "app": "consumer", "external_name": "backing" }),
    )
    .unwrap();
    oi.call("/volumes/site/delete", json!({ "name": "store" }))
        .unwrap();
}

// spec: VOLM#mapping-an-external-volume
#[test]
fn mapping_checks_the_app_the_target_and_duplicates() {
    let oi = TestOi::new();
    let map = |app: &str, target: Value| {
        oi.call(
            "/volumes/external/map",
            json!({ "app": app, "external_name": "backing", "target": target }),
        )
    };
    let site = |name: &str| json!({ "kind": "site", "name": name });

    let (code, _) = map("ghost", site("store")).unwrap_err();
    assert_eq!(code, "not_found", "an app that is not registered");

    register_app(&oi, "consumer", "");
    let (code, _) = map("consumer", site("store")).unwrap_err();
    assert_eq!(
        code, "requirements_invalid",
        "a site volume that is not there"
    );

    register_app(
        &oi,
        "producer",
        r#"app.volume("data").exported(#{ description: "shared" }); app.volume("private");"#,
    );
    for (volume, why) in [("missing", "undeclared"), ("private", "not exported")] {
        let (code, msg) = map(
            "consumer",
            json!({ "kind": "app", "app": "producer", "volume": volume }),
        )
        .unwrap_err();
        assert_eq!(code, "requirements_invalid", "{why}: {msg}");
    }
    let (code, _) = map(
        "consumer",
        json!({ "kind": "app", "app": "ghost", "volume": "data" }),
    )
    .unwrap_err();
    assert_eq!(
        code, "requirements_invalid",
        "a target app that is not registered"
    );

    make_site_volume(&oi, "store");
    map("consumer", site("store")).unwrap();
    let (code, _) = map("consumer", site("store")).unwrap_err();
    assert_eq!(code, "requirements_invalid", "a name the app already maps");

    // A name the definition does not declare is accepted.
    assert_eq!(
        oi.call("/volumes/external/list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

// spec: VOLM#remapping-an-external-volume
#[test]
fn remapping_checks_its_target() {
    let oi = TestOi::new();
    register_app(&oi, "consumer", "");
    make_site_volume(&oi, "store");
    oi.call(
        "/volumes/external/map",
        json!({
            "app": "consumer",
            "external_name": "backing",
            "target": { "kind": "site", "name": "store" },
        }),
    )
    .unwrap();

    let (code, _) = oi
        .call(
            "/volumes/external/remap",
            json!({
                "app": "consumer",
                "external_name": "backing",
                "target": { "kind": "site", "name": "ghost" },
            }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    let list = oi.call("/volumes/external/list", json!({})).unwrap();
    assert_eq!(list[0]["target"]["name"], "store", "unchanged");
}

// spec: VOLM#unmapping-an-external-volume
// spec: SITE#external-mappings
#[test]
fn unmapping_begins_a_reconciliation_iteration() {
    let oi = TestOi::new();
    register_app(&oi, "consumer", "");
    make_site_volume(&oi, "store");
    oi.call(
        "/volumes/external/map",
        json!({
            "app": "consumer",
            "external_name": "backing",
            "target": { "kind": "site", "name": "store" },
        }),
    )
    .unwrap();
    // Drain the wake-up the map left.
    oi.block_on(oi.state.tick_notify.notified());

    oi.call(
        "/volumes/external/unmap",
        json!({ "app": "consumer", "external_name": "backing" }),
    )
    .unwrap();
    oi.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            oi.state.tick_notify.notified(),
        )
        .await
        .expect("an iteration was asked for")
    });
}
