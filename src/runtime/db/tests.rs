use super::*;

// r[verify history.persistence]
// r[verify history.storage]
#[test]
fn open_in_memory_succeeds() {
    let db = Db::open_in_memory().expect("in-memory DB should open");
    let version: i64 = db
        .conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .expect("schema_version should exist");
    assert_eq!(version, 11);
}

// r[verify history.persistence]
#[test]
fn migrate_twice_is_idempotent() {
    let db = Db::open_in_memory().expect("open");
    // Running migrate again should not error
    db.migrate().expect("second migration should not error");
}

// i[verify param.store]
#[test]
fn params_table_exists() {
    let db = Db::open_in_memory().expect("open");
    let count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='params'",
            [],
            |r| r.get(0),
        )
        .expect("query should succeed");
    assert_eq!(count, 1, "params table should exist after migration");
    let version: i64 = db
        .conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .expect("schema_version should exist");
    assert_eq!(version, 11);
}

// i[verify app.persist]
#[test]
fn registered_apps_table_exists() {
    let db = Db::open_in_memory().expect("open");
    let count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='registered_apps'",
            [],
            |r| r.get(0),
        )
        .expect("query should succeed");
    assert_eq!(
        count, 1,
        "registered_apps table should exist after migration"
    );
}

// r[verify history.world.entries]
// r[verify history.operations.entries]
// r[verify history.action-log.entries]
#[test]
fn all_tables_exist_after_migration() {
    let db = Db::open_in_memory().expect("open");

    let tables = [
        "schema_version",
        "world_observations",
        "autonomous_operations",
        "action_log",
    ];
    for table in &tables {
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert_eq!(count, 1, "table '{}' should exist", table);
    }
}

// r[verify reconciliation.idempotency]
#[test]
fn action_log_has_unique_constraint() {
    let db = Db::open_in_memory().expect("open");

    db.conn
        .execute(
            "INSERT INTO action_log
                 (recorded_at, operation_id, app, action_name, call_index,
                  call_kind, resources)
                 VALUES (1, 'op1', 'app', 'start', 0, 'Start', '[]')",
            [],
        )
        .expect("first insert");

    // Second insert with same (operation_id, call_index) should fail
    let result = db.conn.execute(
        "INSERT INTO action_log
             (recorded_at, operation_id, app, action_name, call_index,
              call_kind, resources)
             VALUES (2, 'op1', 'app', 'start', 0, 'Start', '[]')",
        [],
    );
    assert!(
        result.is_err(),
        "duplicate (operation_id, call_index) should be rejected"
    );
}
