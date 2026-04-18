use super::*;
use crate::runtime::db::Db;

fn init_test_events() {
    // In tests the OnceLock may already be set from a prior test in the
    // same process; ignore the error.
    let _ = EVENT_TX.set(seedling_protocol::events::new_event_channel());
}

// i[verify fault.record]
#[test]
fn file_and_list_fault() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let id = file_fault(
        &db,
        "myapp",
        None,
        None,
        None,
        "script_error",
        "parse failed",
    )
    .expect("file_fault");
    assert!(!id.is_empty());

    let faults = list_active_faults(&db, Some("myapp")).expect("list");
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
        "myapp",
        Some("deployment"),
        Some("web"),
        Some("abcd1234"),
        "crash_loop",
        "container keeps restarting",
    )
    .expect("file_fault");

    let faults = list_active_faults(&db, Some("myapp")).expect("list");
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
    let id = file_fault(&db, "myapp", None, None, None, "script_error", "err").expect("file_fault");

    clear_fault(&db, &id, "myapp").expect("clear");

    let active = list_active_faults(&db, Some("myapp")).expect("list");
    assert!(active.is_empty());
}

// i[verify fault.derived]
#[test]
fn clear_faults_by_kind_clears_matching() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    file_fault(&db, "myapp", None, None, None, "script_error", "err1").expect("file1");
    file_fault(&db, "myapp", None, None, None, "script_error", "err2").expect("file2");
    file_fault(
        &db,
        "myapp",
        Some("deployment"),
        Some("web"),
        None,
        "crash_loop",
        "boom",
    )
    .expect("file3");

    let cleared = clear_faults_by_kind(&db, "myapp", "script_error").expect("clear");
    assert_eq!(cleared, 2);

    let remaining = list_active_faults(&db, Some("myapp")).expect("list");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].kind, "crash_loop");
}

// i[verify fault.list]
#[test]
fn list_active_faults_filters_by_app() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    file_fault(&db, "app-a", None, None, None, "script_error", "a err").expect("file a");
    file_fault(&db, "app-b", None, None, None, "script_error", "b err").expect("file b");

    let a_faults = list_active_faults(&db, Some("app-a")).expect("list a");
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
    let id = file_fault(&db, "myapp", None, None, None, "script_error", "err").expect("file_fault");
    file_fault(&db, "myapp", None, None, None, "other", "still active").expect("file2");

    clear_fault(&db, &id, "myapp").expect("clear");

    let faults = list_active_faults(&db, None).expect("list");
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].kind, "other");
}

#[test]
fn clear_all_faults_for_app_clears_only_that_app() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    file_fault(&db, "app-a", None, None, None, "script_error", "a err").expect("a");
    file_fault(
        &db,
        "app-a",
        Some("deployment"),
        Some("web"),
        None,
        "crash",
        "a crash",
    )
    .expect("a2");
    file_fault(&db, "app-b", None, None, None, "script_error", "b err").expect("b");

    clear_all_faults_for_app(&db, "app-a").expect("clear");

    let a = list_active_faults(&db, Some("app-a")).expect("list a");
    assert!(a.is_empty());

    let b = list_active_faults(&db, Some("app-b")).expect("list b");
    assert_eq!(b.len(), 1);
}

#[test]
fn has_active_faults_reflects_state() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    assert!(!has_active_faults(&db, "myapp").expect("check"));

    let id = file_fault(&db, "myapp", None, None, None, "script_error", "err").expect("file");
    assert!(has_active_faults(&db, "myapp").expect("check"));

    clear_fault(&db, &id, "myapp").expect("clear");
    assert!(!has_active_faults(&db, "myapp").expect("check"));
}

#[test]
fn count_active_faults_counts_all_apps() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    assert_eq!(count_active_faults(&db).expect("count"), 0);

    file_fault(&db, "app-a", None, None, None, "err", "a").expect("a");
    file_fault(&db, "app-b", None, None, None, "err", "b").expect("b");
    assert_eq!(count_active_faults(&db).expect("count"), 2);

    clear_all_faults_for_app(&db, "app-a").expect("clear");
    assert_eq!(count_active_faults(&db).expect("count"), 1);
}

// i[verify fault.derived]
#[test]
fn file_fault_emits_fault_filed_event() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let mut rx = EVENT_TX.get().unwrap().subscribe();

    file_fault(&db, "myapp", None, None, None, "script_error", "boom").expect("file");

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

// i[verify fault.derived]
#[test]
fn clear_fault_emits_fault_cleared_event() {
    let db = Db::open_in_memory().expect("open");
    init_test_events();
    let mut rx = EVENT_TX.get().unwrap().subscribe();

    let id = file_fault(&db, "myapp", None, None, None, "script_error", "boom").expect("file");

    // Drain all pending events — parallel tests share the global sender,
    // so there may be stray events ahead of the ones we care about.
    while rx.try_recv().is_ok() {}

    clear_fault(&db, &id, "myapp").expect("clear");

    // Drain again looking for our FaultCleared, skipping any interleaved
    // events from other parallel tests.
    let mut found = false;
    loop {
        match rx.try_recv() {
            Ok(seedling_protocol::events::OiEvent::FaultCleared { id: eid, app, .. }) => {
                assert_eq!(eid, id);
                assert_eq!(app, "myapp");
                found = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(found, "expected a FaultCleared event");
}
