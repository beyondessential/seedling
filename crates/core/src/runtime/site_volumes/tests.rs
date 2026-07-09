use seedling_protocol::names::{AppName, AppVolumeName, SiteVolumeName};

use super::*;
use crate::runtime::db::Db;

fn site(s: &str) -> SiteVolumeName {
    SiteVolumeName::new_unchecked(s)
}

fn def(name: &str, kind: SiteVolumeKind) -> SiteVolumeDef {
    SiteVolumeDef {
        name: site(name),
        kind,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

// r[verify volume.site]
#[test]
fn create_and_get_round_trips_all_kinds() {
    let db = Db::open_in_memory().expect("open");
    create(&db, &def("managed-vol", SiteVolumeKind::Managed)).expect("create managed");
    create(
        &db,
        &def(
            "bind-vol",
            SiteVolumeKind::Bind {
                host_path: "/srv/data".to_owned(),
            },
        ),
    )
    .expect("create bind");
    create(
        &db,
        &def(
            "snap-app",
            SiteVolumeKind::Snapshot {
                source: VolumeRef::App {
                    app: AppName::new("myapp").unwrap(),
                    volume: AppVolumeName::new_unchecked("data"),
                },
            },
        ),
    )
    .expect("create app snapshot");
    create(
        &db,
        &def(
            "snap-site",
            SiteVolumeKind::Snapshot {
                source: VolumeRef::Site {
                    name: site("managed-vol"),
                },
            },
        ),
    )
    .expect("create site snapshot");

    let managed = get(&db, &site("managed-vol")).expect("get").unwrap();
    assert_eq!(managed.kind, SiteVolumeKind::Managed);
    assert_eq!(managed.created_at, "2026-01-01T00:00:00Z");

    let bind = get(&db, &site("bind-vol")).expect("get").unwrap();
    assert_eq!(
        bind.kind,
        SiteVolumeKind::Bind {
            host_path: "/srv/data".to_owned()
        }
    );

    let snap_app = get(&db, &site("snap-app")).expect("get").unwrap();
    assert_eq!(
        snap_app.kind,
        SiteVolumeKind::Snapshot {
            source: VolumeRef::App {
                app: AppName::new("myapp").unwrap(),
                volume: AppVolumeName::new_unchecked("data"),
            }
        }
    );

    let snap_site = get(&db, &site("snap-site")).expect("get").unwrap();
    assert_eq!(
        snap_site.kind,
        SiteVolumeKind::Snapshot {
            source: VolumeRef::Site {
                name: site("managed-vol")
            }
        }
    );
}

// r[verify volume.site.snapshot]
#[test]
fn only_snapshot_volumes_are_read_only() {
    let managed = def("m", SiteVolumeKind::Managed);
    let bind = def(
        "b",
        SiteVolumeKind::Bind {
            host_path: "/srv".to_owned(),
        },
    );
    let snap = def(
        "s",
        SiteVolumeKind::Snapshot {
            source: VolumeRef::Site { name: site("m") },
        },
    );
    assert!(!managed.is_read_only());
    assert!(!bind.is_read_only());
    assert!(snap.is_read_only());
}

// r[verify volume.site]
#[test]
fn list_returns_volumes_sorted_by_name() {
    let db = Db::open_in_memory().expect("open");
    for name in ["zeta", "alpha", "mu"] {
        create(&db, &def(name, SiteVolumeKind::Managed)).expect("create");
    }
    let names: Vec<String> = list(&db)
        .expect("list")
        .into_iter()
        .map(|v| v.name.as_str().to_owned())
        .collect();
    assert_eq!(names, ["alpha", "mu", "zeta"]);
}

// r[verify volume.site.lifecycle]
#[test]
fn delete_removes_volume_and_reports_whether_row_existed() {
    let db = Db::open_in_memory().expect("open");
    create(&db, &def("doomed", SiteVolumeKind::Managed)).expect("create");

    assert!(delete(&db, &site("doomed")).expect("delete"));
    assert!(get(&db, &site("doomed")).expect("get").is_none());
    assert!(!delete(&db, &site("doomed")).expect("delete again"));
}

#[test]
fn get_returns_none_for_unknown_volume() {
    let db = Db::open_in_memory().expect("open");
    assert!(get(&db, &site("ghost")).expect("get").is_none());
}
