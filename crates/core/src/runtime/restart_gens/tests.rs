use seedling_protocol::names::AppName;

use super::*;
use crate::runtime::db::Db;

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

// i[verify deployment.restart]
#[test]
fn load_defaults_to_zero_and_bump_increments() {
    let db = Db::open_in_memory().expect("open");
    assert_eq!(
        load_restart_gen(&db, &app("myapp"), "web").expect("load"),
        0
    );

    assert_eq!(
        bump_restart_gen(&db, &app("myapp"), "web").expect("bump"),
        1
    );
    assert_eq!(
        bump_restart_gen(&db, &app("myapp"), "web").expect("bump again"),
        2
    );
    assert_eq!(
        load_restart_gen(&db, &app("myapp"), "web").expect("load"),
        2
    );
}

// i[verify deployment.restart]
#[test]
fn generations_are_scoped_per_app_and_deployment() {
    let db = Db::open_in_memory().expect("open");
    bump_restart_gen(&db, &app("app-a"), "web").expect("bump a/web");
    bump_restart_gen(&db, &app("app-a"), "web").expect("bump a/web");
    bump_restart_gen(&db, &app("app-a"), "worker").expect("bump a/worker");
    bump_restart_gen(&db, &app("app-b"), "web").expect("bump b/web");

    assert_eq!(
        load_restart_gen(&db, &app("app-a"), "web").expect("load"),
        2
    );
    assert_eq!(
        load_restart_gen(&db, &app("app-a"), "worker").expect("load"),
        1
    );
    assert_eq!(
        load_restart_gen(&db, &app("app-b"), "web").expect("load"),
        1
    );
}

#[test]
fn delete_restart_gens_clears_only_that_app() {
    let db = Db::open_in_memory().expect("open");
    bump_restart_gen(&db, &app("app-a"), "web").expect("bump");
    bump_restart_gen(&db, &app("app-a"), "worker").expect("bump");
    bump_restart_gen(&db, &app("app-b"), "web").expect("bump");

    delete_restart_gens_for_app(&db, &app("app-a")).expect("delete");

    assert_eq!(
        load_restart_gen(&db, &app("app-a"), "web").expect("load"),
        0
    );
    assert_eq!(
        load_restart_gen(&db, &app("app-a"), "worker").expect("load"),
        0
    );
    assert_eq!(
        load_restart_gen(&db, &app("app-b"), "web").expect("load"),
        1
    );
}
