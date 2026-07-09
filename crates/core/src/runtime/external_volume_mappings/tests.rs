use seedling_protocol::names::{AppName, AppVolumeName, ExternalVolumeName, SiteVolumeName};

use super::*;
use crate::runtime::db::Db;

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

fn ext(s: &str) -> ExternalVolumeName {
    ExternalVolumeName::new_unchecked(s)
}

fn app_target(owner: &str, volume: &str) -> VolumeRef {
    VolumeRef::App {
        app: app(owner),
        volume: AppVolumeName::new_unchecked(volume),
    }
}

fn site_target(name: &str) -> VolumeRef {
    VolumeRef::Site {
        name: SiteVolumeName::new_unchecked(name),
    }
}

#[test]
fn create_and_get_round_trips_app_and_site_targets() {
    let db = Db::open_in_memory().expect("open");
    create(
        &db,
        &ExternalVolumeMapping {
            app: app("consumer"),
            external_name: ext("shared"),
            target: app_target("producer", "data"),
            read_only: true,
        },
    )
    .expect("create app-target");
    create(
        &db,
        &ExternalVolumeMapping {
            app: app("consumer"),
            external_name: ext("backups"),
            target: site_target("site-backups"),
            read_only: false,
        },
    )
    .expect("create site-target");

    let shared = get(&db, &app("consumer"), &ext("shared"))
        .expect("get")
        .expect("mapping present");
    assert_eq!(shared.target, app_target("producer", "data"));
    assert!(shared.read_only);

    let backups = get(&db, &app("consumer"), &ext("backups"))
        .expect("get")
        .expect("mapping present");
    assert_eq!(backups.target, site_target("site-backups"));
    assert!(!backups.read_only);
}

#[test]
fn get_returns_none_for_missing_mapping() {
    let db = Db::open_in_memory().expect("open");
    assert!(
        get(&db, &app("consumer"), &ext("ghost"))
            .expect("get")
            .is_none()
    );
}

#[test]
fn update_changes_target_and_reports_whether_row_existed() {
    let db = Db::open_in_memory().expect("open");
    create(
        &db,
        &ExternalVolumeMapping {
            app: app("consumer"),
            external_name: ext("shared"),
            target: app_target("producer", "data"),
            read_only: false,
        },
    )
    .expect("create");

    let updated = update(
        &db,
        &ExternalVolumeMapping {
            app: app("consumer"),
            external_name: ext("shared"),
            target: site_target("site-data"),
            read_only: true,
        },
    )
    .expect("update");
    assert!(updated);

    let mapping = get(&db, &app("consumer"), &ext("shared"))
        .expect("get")
        .expect("mapping present");
    assert_eq!(mapping.target, site_target("site-data"));
    assert!(mapping.read_only);

    let missing = update(
        &db,
        &ExternalVolumeMapping {
            app: app("consumer"),
            external_name: ext("ghost"),
            target: site_target("site-data"),
            read_only: false,
        },
    )
    .expect("update missing");
    assert!(!missing);
}

#[test]
fn delete_removes_mapping_and_reports_whether_row_existed() {
    let db = Db::open_in_memory().expect("open");
    create(
        &db,
        &ExternalVolumeMapping {
            app: app("consumer"),
            external_name: ext("shared"),
            target: site_target("site-data"),
            read_only: false,
        },
    )
    .expect("create");

    assert!(delete(&db, &app("consumer"), &ext("shared")).expect("delete"));
    assert!(
        get(&db, &app("consumer"), &ext("shared"))
            .expect("get")
            .is_none()
    );
    assert!(!delete(&db, &app("consumer"), &ext("shared")).expect("delete again"));
}

#[test]
fn list_for_app_is_scoped_and_sorted_by_external_name() {
    let db = Db::open_in_memory().expect("open");
    for (owner, name) in [("app-a", "zeta"), ("app-a", "alpha"), ("app-b", "other")] {
        create(
            &db,
            &ExternalVolumeMapping {
                app: app(owner),
                external_name: ext(name),
                target: site_target("site-data"),
                read_only: false,
            },
        )
        .expect("create");
    }

    let names: Vec<String> = list_for_app(&db, &app("app-a"))
        .expect("list")
        .into_iter()
        .map(|m| m.external_name.as_str().to_owned())
        .collect();
    assert_eq!(names, ["alpha", "zeta"]);

    let all = list_all(&db).expect("list all");
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].app, "app-a");
    assert_eq!(all[2].app, "app-b");
}

#[test]
fn legacy_exported_kind_decodes_as_app_target() {
    let db = Db::open_in_memory().expect("open");
    db.conn
        .execute(
            "INSERT INTO external_volume_mappings \
                 (app, external_name, target_kind, target_app, target_volume, read_only) \
             VALUES ('consumer', 'shared', 'exported', 'producer', 'data', 0)",
            [],
        )
        .expect("insert legacy row");

    let mapping = get(&db, &app("consumer"), &ext("shared"))
        .expect("get")
        .expect("mapping present");
    assert_eq!(mapping.target, app_target("producer", "data"));
}
