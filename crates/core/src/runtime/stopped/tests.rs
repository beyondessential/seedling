use seedling_protocol::names::AppName;

use super::*;
use crate::runtime::db::Db;

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

// i[verify resource.stop]
#[test]
fn stop_and_load_round_trip_is_idempotent() {
    let db = Db::open_in_memory().expect("open");
    stop_resource(&db, &app("myapp"), ResourceKind::Deployment, "web").expect("stop");
    stop_resource(&db, &app("myapp"), ResourceKind::Deployment, "web").expect("stop again");
    stop_resource(&db, &app("myapp"), ResourceKind::Ingress, "public").expect("stop ingress");

    let stopped = load_stopped(&db, &app("myapp")).expect("load");
    assert_eq!(stopped.len(), 2);
    assert!(stopped.contains(&(ResourceKind::Deployment, "web".to_owned())));
    assert!(stopped.contains(&(ResourceKind::Ingress, "public".to_owned())));
}

// i[verify resource.stop]
#[test]
fn load_stopped_is_scoped_to_app() {
    let db = Db::open_in_memory().expect("open");
    stop_resource(&db, &app("app-a"), ResourceKind::Deployment, "web").expect("stop a");
    stop_resource(&db, &app("app-b"), ResourceKind::Deployment, "web").expect("stop b");

    assert_eq!(load_stopped(&db, &app("app-a")).expect("load a").len(), 1);
    assert_eq!(load_stopped(&db, &app("app-b")).expect("load b").len(), 1);
    assert!(load_stopped(&db, &app("app-c")).expect("load c").is_empty());
}

// i[verify resource.unstop]
#[test]
fn unstop_removes_only_the_named_resource() {
    let db = Db::open_in_memory().expect("open");
    stop_resource(&db, &app("myapp"), ResourceKind::Deployment, "web").expect("stop web");
    stop_resource(&db, &app("myapp"), ResourceKind::Deployment, "worker").expect("stop worker");
    stop_resource(&db, &app("myapp"), ResourceKind::Service, "web").expect("stop svc");

    unstop_resource(&db, &app("myapp"), ResourceKind::Deployment, "web").expect("unstop");

    let stopped = load_stopped(&db, &app("myapp")).expect("load");
    assert!(!stopped.contains(&(ResourceKind::Deployment, "web".to_owned())));
    assert!(stopped.contains(&(ResourceKind::Deployment, "worker".to_owned())));
    assert!(
        stopped.contains(&(ResourceKind::Service, "web".to_owned())),
        "same name under a different kind must survive"
    );
}

// i[verify resource.unstop-all]
#[test]
fn unstop_all_clears_only_that_app() {
    let db = Db::open_in_memory().expect("open");
    stop_resource(&db, &app("app-a"), ResourceKind::Deployment, "web").expect("stop");
    stop_resource(&db, &app("app-a"), ResourceKind::Job, "migrate").expect("stop");
    stop_resource(&db, &app("app-b"), ResourceKind::Deployment, "web").expect("stop");

    unstop_all(&db, &app("app-a")).expect("unstop all");

    assert!(load_stopped(&db, &app("app-a")).expect("load a").is_empty());
    assert_eq!(load_stopped(&db, &app("app-b")).expect("load b").len(), 1);
}

#[test]
fn kind_str_and_parse_kind_round_trip_all_kinds() {
    for kind in [
        ResourceKind::Deployment,
        ResourceKind::Job,
        ResourceKind::Ingress,
        ResourceKind::Service,
        ResourceKind::Volume,
        ResourceKind::ExternalVolume,
        ResourceKind::ExternalService,
        ResourceKind::Parameter,
        ResourceKind::HttpService,
        ResourceKind::Action,
    ] {
        assert_eq!(parse_kind(kind_str(kind)), Some(kind), "{kind:?}");
    }
    assert_eq!(parse_kind("garbage"), None);
}

#[test]
fn load_stopped_skips_rows_with_unknown_kind() {
    let db = Db::open_in_memory().expect("open");
    stop_resource(&db, &app("myapp"), ResourceKind::Deployment, "web").expect("stop");
    db.conn
        .execute(
            "INSERT INTO stopped_resources (app, kind, name) VALUES ('myapp', 'flurble', 'x')",
            [],
        )
        .expect("insert unknown kind row");

    let stopped = load_stopped(&db, &app("myapp")).expect("load");
    assert_eq!(stopped.len(), 1, "unknown kind rows are dropped");
}
