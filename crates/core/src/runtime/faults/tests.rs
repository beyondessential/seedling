use super::*;
use crate::runtime::db::Db;

fn init_test_events() {
    // In tests the OnceLock may already be set from a prior test in the
    // same process; ignore the error.
    let _ = EVENT_TX.set(seedling_protocol::events::new_event_channel());
}

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

// r[verify fault.definition]
// i[verify fault.record]
#[test]
fn file_and_list_fault() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let id = file_fault(
        &db,
        &app("myapp"),
        None,
        None,
        None,
        "script_error",
        "parse failed",
    )
    .expect("file_fault");
    assert!(!id.is_empty());

    let faults = list_active_faults(&db, Some(&app("myapp"))).expect("list");
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].id, id);
    assert_eq!(faults[0].app, "myapp");
    assert_eq!(faults[0].kind, "script_error");
    assert_eq!(faults[0].description, "parse failed");
    assert!(faults[0].resource_type.is_none());
}

// i[verify fault.record]
#[test]
fn file_fault_with_resource_fields() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let id = file_fault(
        &db,
        &app("myapp"),
        Some("deployment"),
        Some("web"),
        Some("abcd1234"),
        "crash_loop",
        "container keeps restarting",
    )
    .expect("file_fault");

    let faults = list_active_faults(&db, Some(&app("myapp"))).expect("list");
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].id, id);
    assert_eq!(faults[0].resource_type.as_deref(), Some("deployment"));
    assert_eq!(faults[0].resource_name.as_deref(), Some("web"));
    assert_eq!(faults[0].instance_id.as_deref(), Some("abcd1234"));
}

// i[verify fault.derived]
#[test]
fn clear_fault_sets_cleared_at() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let id = file_fault(&db, &app("myapp"), None, None, None, "script_error", "err")
        .expect("file_fault");

    clear_fault(&db, &id, &app("myapp")).expect("clear");

    let active = list_active_faults(&db, Some(&app("myapp"))).expect("list");
    assert!(active.is_empty());
}

// i[verify fault.derived]
#[test]
fn clear_faults_by_kind_clears_matching() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    file_fault(&db, &app("myapp"), None, None, None, "script_error", "err1").expect("file1");
    // Distinct subjects: one active fault per (app, kind, subject) now, so
    // two same-subject script errors would be one fault, not two.
    file_fault(
        &db,
        &app("myapp"),
        None,
        Some("second"),
        None,
        "script_error",
        "err2",
    )
    .expect("file2");
    file_fault(
        &db,
        &app("myapp"),
        Some("deployment"),
        Some("web"),
        None,
        "crash_loop",
        "boom",
    )
    .expect("file3");

    let cleared = clear_faults_by_kind(&db, &app("myapp"), "script_error").expect("clear");
    assert_eq!(cleared, 2);

    let remaining = list_active_faults(&db, Some(&app("myapp"))).expect("list");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].kind, "crash_loop");
}

// i[verify fault.list]
#[test]
fn list_active_faults_filters_by_app() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    file_fault(
        &db,
        &app("app-a"),
        None,
        None,
        None,
        "script_error",
        "a err",
    )
    .expect("file a");
    file_fault(
        &db,
        &app("app-b"),
        None,
        None,
        None,
        "script_error",
        "b err",
    )
    .expect("file b");

    let a_faults = list_active_faults(&db, Some(&app("app-a"))).expect("list a");
    assert_eq!(a_faults.len(), 1);
    assert_eq!(a_faults[0].app, "app-a");

    let all_faults = list_active_faults(&db, None).expect("list all");
    assert_eq!(all_faults.len(), 2);
}

// i[verify fault.list]
#[test]
fn list_active_faults_excludes_cleared() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let id = file_fault(&db, &app("myapp"), None, None, None, "script_error", "err")
        .expect("file_fault");
    file_fault(
        &db,
        &app("myapp"),
        None,
        None,
        None,
        "other",
        "still active",
    )
    .expect("file2");

    clear_fault(&db, &id, &app("myapp")).expect("clear");

    let faults = list_active_faults(&db, None).expect("list");
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].kind, "other");
}

#[test]
fn clear_all_faults_for_app_clears_only_that_app() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    file_fault(
        &db,
        &app("app-a"),
        None,
        None,
        None,
        "script_error",
        "a err",
    )
    .expect("a");
    file_fault(
        &db,
        &app("app-a"),
        Some("deployment"),
        Some("web"),
        None,
        "crash",
        "a crash",
    )
    .expect("a2");
    file_fault(
        &db,
        &app("app-b"),
        None,
        None,
        None,
        "script_error",
        "b err",
    )
    .expect("b");

    clear_all_faults_for_app(&db, &app("app-a")).expect("clear");

    let a = list_active_faults(&db, Some(&app("app-a"))).expect("list a");
    assert!(a.is_empty());

    let b = list_active_faults(&db, Some(&app("app-b"))).expect("list b");
    assert_eq!(b.len(), 1);
}

#[test]
fn has_active_faults_reflects_state() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    assert!(!has_active_faults(&db, &app("myapp")).expect("check"));

    let id = file_fault(&db, &app("myapp"), None, None, None, "script_error", "err").expect("file");
    assert!(has_active_faults(&db, &app("myapp")).expect("check"));

    clear_fault(&db, &id, &app("myapp")).expect("clear");
    assert!(!has_active_faults(&db, &app("myapp")).expect("check"));
}

#[test]
fn count_active_faults_for_app_counts_only_uncleared() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    assert_eq!(
        count_active_faults_for_app(&db, &app("myapp")).expect("count"),
        0
    );

    let id1 = file_fault(&db, &app("myapp"), None, Some("one"), None, "err", "1").expect("1");
    file_fault(&db, &app("myapp"), None, Some("two"), None, "err", "2").expect("2");
    file_fault(&db, &app("other"), None, None, None, "err", "3").expect("3");
    assert_eq!(
        count_active_faults_for_app(&db, &app("myapp")).expect("count"),
        2
    );
    assert_eq!(
        count_active_faults_for_app(&db, &app("other")).expect("count"),
        1
    );

    clear_fault(&db, &id1, &app("myapp")).expect("clear");
    assert_eq!(
        count_active_faults_for_app(&db, &app("myapp")).expect("count"),
        1
    );
}

#[test]
fn count_active_faults_counts_all_apps() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    assert_eq!(count_active_faults(&db).expect("count"), 0);

    file_fault(&db, &app("app-a"), None, None, None, "err", "a").expect("a");
    file_fault(&db, &app("app-b"), None, None, None, "err", "b").expect("b");
    assert_eq!(count_active_faults(&db).expect("count"), 2);

    clear_all_faults_for_app(&db, &app("app-a")).expect("clear");
    assert_eq!(count_active_faults(&db).expect("count"), 1);
}

// r[verify fault.surfacing]
// i[verify fault.derived]
#[test]
fn file_fault_emits_fault_filed_event() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let mut rx = EVENT_TX.get().unwrap().subscribe();

    file_fault(&db, &app("myapp"), None, None, None, "script_error", "boom").expect("file");

    // Parallel tests share the global sender; drain looking for our event.
    let mut found = false;
    loop {
        match rx.try_recv() {
            Ok(seedling_protocol::events::OiEvent::FaultFiled {
                app,
                kind,
                description,
                ..
            }) if app == "myapp" && kind == "script_error" && description == "boom" => {
                found = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(
        found,
        "expected a FaultFiled event for myapp/script_error/boom"
    );
}

// r[verify fault.surfacing]
// i[verify fault.derived]
#[test]
fn clear_fault_emits_fault_cleared_event() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let mut rx = EVENT_TX.get().unwrap().subscribe();

    let id =
        file_fault(&db, &app("myapp"), None, None, None, "script_error", "boom").expect("file");

    // Drain all pending events — parallel tests share the global sender,
    // so there may be stray events ahead of the ones we care about.
    while rx.try_recv().is_ok() {}

    clear_fault(&db, &id, &app("myapp")).expect("clear");

    // Drain again looking for our FaultCleared, skipping any interleaved
    // events from other parallel tests. The id guard is load-bearing: the
    // sender is global, several sibling tests clear faults of their own, and
    // without it the first FaultCleared from any of them is asserted against
    // this test's id.
    let mut found = false;
    loop {
        match rx.try_recv() {
            Ok(seedling_protocol::events::OiEvent::FaultCleared {
                id: eid, app, kind, ..
            }) if eid == id => {
                assert_eq!(app, "myapp");
                assert_eq!(kind, "script_error");
                found = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(found, "expected a FaultCleared event");
}

// r[verify fault.image-pull]
#[test]
fn clear_faults_for_instance_only_removes_matching_instance() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    file_fault(
        &db,
        &app("myapp"),
        Some("job"),
        None,
        Some("instance-a"),
        "image_pull_failed",
        "bad a",
    )
    .expect("file a");
    file_fault(
        &db,
        &app("myapp"),
        Some("job"),
        None,
        Some("instance-b"),
        "container_start_failed",
        "bad b",
    )
    .expect("file b");
    // A fault with a different instance_id and a fault with no instance
    // should both survive.
    file_fault(
        &db,
        &app("myapp"),
        None,
        None,
        None,
        "operation_failed",
        "no instance",
    )
    .expect("file c");

    clear_faults_for_instance(&db, &app("myapp"), "instance-a").expect("clear a");

    let remaining = list_active_faults(&db, Some(&app("myapp"))).expect("list");
    let kinds: Vec<_> = remaining.iter().map(|f| f.kind.clone()).collect();
    assert!(!kinds.contains(&"image_pull_failed".to_string()));
    assert!(kinds.contains(&"container_start_failed".to_string()));
    assert!(kinds.contains(&"operation_failed".to_string()));
}

fn meta() -> FaultMeta {
    FaultMeta::default()
}

fn desc(key: &FaultKey) -> std::collections::BTreeMap<FaultKey, (FaultMeta, String)> {
    let mut map = std::collections::BTreeMap::new();
    map.insert(key.clone(), (meta(), format!("{} is faulty", key.subject)));
    map
}

// r[verify fault.lifecycle]
// At most one active fault per key: `audit_lag` filed one per lag event
// without bound, and GC prunes only cleared faults.
#[test]
fn file_once_does_not_duplicate() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let key = FaultKey::app_wide(&app("seedling"), "audit_lag");

    assert!(file_once(&db, &key, &meta(), "42 events dropped").unwrap());
    assert!(!file_once(&db, &key, &meta(), "7 more events dropped").unwrap());

    let active = list_active_faults(&db, None).unwrap();
    assert_eq!(active.len(), 1, "{active:#?}");
}

// r[verify fault.lifecycle]
// Clearing keyed no more broadly than filing: this is H8 — a successful
// backup of one volume cleared every other volume's failure.
#[test]
fn faults_for_different_subjects_are_independent() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let app = app("backups");
    let ok = FaultKey::new(&app, "backup_failed", "site/data");
    let broken = FaultKey::new(&app, "backup_failed", "site/archive");

    file_once(&db, &ok, &meta(), "data failed").unwrap();
    file_once(&db, &broken, &meta(), "archive failed").unwrap();

    // "data" succeeds on a later run and clears only its own key.
    let to_clear: Vec<_> = list_active_faults(&db, Some(&app))
        .unwrap()
        .into_iter()
        .filter(|f| f.kind == "backup_failed" && f.subject == "site/data")
        .collect();
    for fault in &to_clear {
        clear_fault(&db, &fault.id, &app).unwrap();
    }

    let active = list_active_faults(&db, Some(&app)).unwrap();
    assert_eq!(active.len(), 1, "{active:#?}");
    assert_eq!(active[0].subject, "site/archive");
}

// r[verify fault.lifecycle]
// A condition fault is active exactly while its condition holds — including
// when the condition stopped holding before this process started. The
// reconciler used to diff against an in-memory prior set that empties on
// every daemon start, so a fault filed before a restart could never clear.
#[test]
fn sync_clears_a_fault_filed_by_a_previous_process() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let system = AppName::new_unchecked("_system");
    let key = FaultKey::new(&system, "ingress_conflict", "example.com:443");

    // Pre-seed as if a previous daemon lifetime filed it.
    file_once(&db, &key, &meta(), "conflict on example.com:443").unwrap();

    // This tick sees no conflicts at all, with no memory of the previous one.
    let outcome = sync_faults(
        &db,
        &FaultScope::Kind("ingress_conflict".to_owned()),
        &std::collections::BTreeMap::new(),
    )
    .unwrap();

    assert_eq!(outcome.cleared, 1);
    assert!(list_active_faults(&db, None).unwrap().is_empty());
}

// r[verify fault.lifecycle]
#[test]
fn sync_is_idempotent_and_converges_from_any_state() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let system = AppName::new_unchecked("_system");
    let key = FaultKey::new(&system, "ingress_conflict", "a.example:443");
    let scope = FaultScope::Kind("ingress_conflict".to_owned());

    let first = sync_faults(&db, &scope, &desc(&key)).unwrap();
    assert_eq!(
        first,
        SyncOutcome {
            filed: 1,
            cleared: 0
        }
    );

    let second = sync_faults(&db, &scope, &desc(&key)).unwrap();
    assert_eq!(
        second,
        SyncOutcome {
            filed: 0,
            cleared: 0
        }
    );

    assert_eq!(list_active_faults(&db, None).unwrap().len(), 1);
}

// r[verify fault.lifecycle]
// A sweep may only clear within its declared scope; otherwise converging one
// kind to empty would wipe every other kind's faults.
#[test]
fn sync_never_touches_kinds_outside_its_scope() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let system = AppName::new_unchecked("_system");
    let other = FaultKey::new(&system, "resolver_failed", "");
    file_once(&db, &other, &meta(), "resolver down").unwrap();

    sync_faults(
        &db,
        &FaultScope::Kind("ingress_conflict".to_owned()),
        &std::collections::BTreeMap::new(),
    )
    .unwrap();

    let active = list_active_faults(&db, None).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].kind, "resolver_failed");
}
