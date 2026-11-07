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
    assert_eq!(version, 62);
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
    assert_eq!(version, 62);
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

#[test]
fn backup_tables_are_dropped_and_their_active_faults_retired() {
    let db = Db::open_in_memory_through(58).expect("open at v58");
    db.conn
        .execute_batch(
            "INSERT INTO backup_apps (app) VALUES ('kopia');
             INSERT INTO backup_strategies (name, via, schedule, volumes)
                 VALUES ('nightly', 'kopia', 'every day', '[\"web/data\"]');
             INSERT INTO faults (id, app, kind, timestamp, description, subject)
                 VALUES ('f1', 'kopia', 'backup_failed', 't', 'x', 'web/data'),
                        ('f2', 'kopia', 'backup_source_unavailable', 't', 'x', 'web/data'),
                        ('f3', 'kopia', 'backup_app_unavailable', 't', 'x', ''),
                        ('f4', 'web', 'container_start_failed', 't', 'x', 'web-1');",
        )
        .expect("seed v58 data");

    db.finish_migrations().expect("migrate to the latest");

    for table in ["backup_apps", "backup_strategies"] {
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table} should be dropped");
    }

    let active: Vec<String> = db
        .conn
        .prepare("SELECT id FROM faults WHERE cleared_at IS NULL")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(active, vec!["f4".to_owned()]);
}

// r[verify tls.strategy.canopy]
// r[verify tls.strategy.canopy-only]
// r[verify tls.chain.ladder]
#[test]
fn canopy_migration_keeps_policies_certificates_and_attempt_links() {
    let db = Db::open_in_memory_through(60).expect("open at v60");
    db.conn
        .execute_batch(
            "INSERT INTO tls_dns_providers (name, kind, config_ciphertext, created_at, updated_at)
                 VALUES ('p', 'route53', x'00', 1, 1);
             INSERT INTO tls_certificates
                 (id, hostname, state, key_ciphertext, key_type, created_at, updated_at,
                  origin, requested_hostname)
                 VALUES (7, 'a.example.com', 'active', x'01', 'ecdsa_p256', 1, 1, 'csr', 'a.example.com'),
                        (9, 'b.example.com', 'superseded', x'02', 'ecdsa_p256', 1, 1, 'acme_dns', NULL);
             INSERT INTO tls_policies (hostname, strategy, dns_provider, cert_id, updated_at)
                 VALUES ('*.example.com', 'acme_dns', 'p', NULL, 1),
                        ('legacy.example.com', 'manual', NULL, 7, 1);
             INSERT INTO tls_cert_attempts
                 (id, hostname, triggered_by, started_at, outcome, cert_id)
                 VALUES (1, 'a.example.com', 'manual', 1, 'success', 7),
                        (2, 'b.example.com', 'on_demand', 2, 'failure', NULL);",
        )
        .expect("seed v60 data");

    db.finish_migrations().expect("migrate to the latest");

    let policies: Vec<(String, String, Option<i64>)> = db
        .conn
        .prepare("SELECT hostname, strategy, cert_id FROM tls_policies ORDER BY hostname")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        policies,
        vec![
            ("*.example.com".to_owned(), "acme_dns".to_owned(), None),
            (
                "legacy.example.com".to_owned(),
                "manual".to_owned(),
                Some(7)
            ),
        ]
    );

    let certs: Vec<(i64, String, Option<String>)> = db
        .conn
        .prepare("SELECT id, origin, requested_hostname FROM tls_certificates ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        certs,
        vec![
            (7, "csr".to_owned(), Some("a.example.com".to_owned())),
            (9, "acme_dns".to_owned(), None),
        ]
    );

    let links: Vec<(i64, Option<i64>)> = db
        .conn
        .prepare("SELECT id, cert_id FROM tls_cert_attempts ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        links,
        vec![(1, Some(7)), (2, None)],
        "the attempt log must still say which certificate each attempt produced"
    );

    db.conn
        .execute_batch(
            "INSERT INTO tls_policies (hostname, strategy, dns_provider, cert_id, updated_at)
                 VALUES ('c.example.com', 'canopy', NULL, NULL, 1);
             INSERT INTO tls_certificates
                 (hostname, state, key_ciphertext, key_type, created_at, updated_at, origin)
                 VALUES ('c.example.com', 'csr_pending', x'03', 'ecdsa_p256', 1, 1, 'canopy'),
                        ('d.example.com', 'revoked', x'', 'ecdsa_p256', 1, 1, 'canopy');
             INSERT INTO tls_chain_state
                 (hostname, replacing_cert_id, replacing_not_after, step, step_since, updated_at)
                 VALUES ('c.example.com', NULL, NULL, 0, 1, 1);",
        )
        .expect("the Canopy policy, origin, revoked state and chain row are accepted");

    assert!(
        db.conn
            .execute(
                "INSERT INTO tls_policies (hostname, strategy, dns_provider, cert_id, updated_at)
                     VALUES ('e.example.com', 'canopy', 'p', NULL, 1)",
                [],
            )
            .is_err(),
        "a Canopy policy carries no DNS provider"
    );
}
