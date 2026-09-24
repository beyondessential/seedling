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
    assert_eq!(version, 58);
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
    assert_eq!(version, 58);
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

// r[verify infra.db.file-permissions]
#[test]
fn open_creates_database_with_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("seedling.db");
    let _db = Db::open(&path).expect("open db");
    let mode = path.metadata().unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "db file should be 0600, got 0{mode:o}");
}

// r[verify infra.db.file-permissions]
#[test]
fn open_rejects_database_with_group_readable_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("loose.db");
    // Pre-create the file with permissive mode so open() refuses.
    std::fs::write(&path, b"").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    let err = match Db::open(&path) {
        Ok(_) => panic!("should refuse 0640 mode"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("insecure permissions"),
        "error should mention permissions: {msg}",
    );
}

// r[verify infra.db.busy-timeout]
#[test]
fn open_in_memory_sets_busy_timeout() {
    let db = Db::open_in_memory().expect("open");
    // sqlite exposes the busy timeout via PRAGMA busy_timeout.
    let timeout_ms: i64 = db
        .conn
        .query_row("PRAGMA busy_timeout;", [], |r| r.get(0))
        .expect("pragma succeeds");
    assert!(
        timeout_ms >= 5000,
        "busy_timeout should be at least 5s, got {timeout_ms}ms",
    );
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

// r[verify generation.script-storage]
// i[verify app.script]
// i[verify definition.provenance]
#[test]
fn stored_scripts_become_one_file_bundles_pushed_by_nobody_known() {
    use crate::runtime::definition::{Bundle, Source};

    let db = Db::open_in_memory_through(57).expect("open at v57");
    let script = r#"app.deployment("web").image("docker.io/library/nginx:1.29");"#;
    db.conn
        .execute_batch(&format!(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation)
                 VALUES ('web', 0, 0, 2);
             INSERT INTO script_bodies (hash, body) VALUES ('oldhash', '{s}');
             INSERT INTO generations (app, generation, created_at, kind, script_hash)
                 VALUES ('web', 1, 't', 'register', 'oldhash');
             INSERT INTO generations (app, generation, created_at, kind, param_name, new_value, script_hash)
                 VALUES ('web', 2, 't', 'param_set', 'mode', 'x', 'oldhash');
             INSERT INTO templates (name, body, description, created_at)
                 VALUES ('tmpl', '{s}', NULL, 't');",
            s = script.replace('\'', "''")
        ))
        .expect("seed v57 data");

    db.finish_migrations().expect("migrate to the latest");

    let expected = Bundle::from_stored_script(script);
    let app = seedling_protocol::names::AppName::new("web").unwrap();
    for generation in [1, 2] {
        let (hash, source) =
            crate::runtime::generations::definition_at(&db, &app, generation).unwrap();
        assert_eq!(hash, expected.hash());
        assert_eq!(source, Source::unknown_push());
    }
    let bundle = crate::runtime::generations::load_bundle(&db, expected.hash()).unwrap();
    assert_eq!(bundle.script().unwrap().text(), script);

    let t = crate::runtime::templates::get(
        &db,
        &seedling_protocol::names::TemplateName::new_unchecked("tmpl"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(t.bundle.hash(), expected.hash());
    assert_eq!(t.source, Source::unknown_push());
}
