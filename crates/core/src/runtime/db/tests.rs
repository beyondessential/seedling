use super::*;

// spec: HIST#durability
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
    assert_eq!(version, 84);
}

// spec: HIST#durability
#[test]
fn migrate_twice_is_idempotent() {
    let db = Db::open_in_memory().expect("open");
    // Running migrate again should not error
    db.migrate().expect("second migration should not error");
}

// spec: PRM#parameter-storage
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
    assert_eq!(version, 84);
}

// spec: APPS#persistence
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

// spec: HIST#observations
// spec: HIST#autonomous-operations
// spec: HIST#action-calls
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

// spec: INFRA#file-permissions
#[test]
fn open_creates_database_with_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("seedling.db");
    let _db = Db::open(&path).expect("open db");
    let mode = path.metadata().unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "db file should be 0600, got 0{mode:o}");
    for side in ["seedling.db-wal", "seedling.db-shm"] {
        let mode = dir
            .path()
            .join(side)
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{side} should be 0600, got 0{mode:o}");
    }
}

// spec: INFRA#file-permissions
#[test]
fn open_does_not_create_the_database_through_a_planted_symlink() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("seedling.db");
    let target = dir.path().join("elsewhere.db");
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(Db::open(&path).is_err());
    assert!(
        !target.exists(),
        "the database must not be created through the link"
    );
}

// spec: INFRA#file-permissions
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

// spec: INFRA#database
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

// spec: RCN#reconciliation-loop
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

// spec: GEN#bundle-storage
// spec: APPS#definition-retrieval
// spec: DEFN#provenance-reporting
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

// spec: TCAN#applicability-and-availability
// spec: TCAN#canopy-only-policy
// spec: CHAIN#fall-through
// spec: TLS#certificate-signing-requests
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
        .prepare("SELECT id, origin, requested_hostnames FROM tls_certificates ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        certs,
        vec![
            // A single requested hostname is carried on as a set of one.
            (7, "csr".to_owned(), Some(r#"["a.example.com"]"#.to_owned())),
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

/// Every stored certificate is indexed by the migration. One whose PEM will not
/// parse is indexed as covering nothing rather than stopping the upgrade, and a
/// row with no certificate yet is left out.
// spec: TLS#serving
#[test]
fn san_index_migration_indexes_every_stored_certificate() {
    let db = Db::open_in_memory_through(66).expect("open at v66");
    let pem = crate::runtime::tls::test_support::self_signed_pem(&["Example.com", "*.example.com"]);
    db.conn
        .execute(
            "INSERT INTO tls_certificates
                 (id, hostname, state, cert_pem, key_ciphertext, key_type, created_at, updated_at, origin)
                 VALUES (1, 'example.com', 'superseded', ?1, x'01', 'ecdsa_p256', 1, 1, 'manual'),
                        (2, 'broken.example.com', 'active', 'not a certificate', x'01', 'ecdsa_p256', 1, 1, 'manual'),
                        (3, 'pending.example.com', 'csr_pending', NULL, x'01', 'ecdsa_p256', 1, 1, 'csr')",
            [&pem],
        )
        .expect("seed v66 data");

    db.finish_migrations().expect("migrate to the latest");

    let index: Vec<(i64, Option<bool>)> = db
        .conn
        .prepare("SELECT cert_id, self_issued FROM tls_cert_index ORDER BY cert_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(index, vec![(1, Some(true)), (2, None)]);

    let sans: Vec<(i64, String)> = db
        .conn
        .prepare("SELECT cert_id, san FROM tls_cert_sans ORDER BY cert_id, san")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        sans,
        vec![
            (1, "*.example.com".to_owned()),
            (1, "example.com".to_owned()),
        ]
    );
}

// spec: WIRE#resource-type-names
// Records written before the wire named resource types by their wire names
// hold the Rust names lowercased.
#[test]
fn resource_type_records_are_renamed_to_the_wire_names() {
    let db = Db::open_in_memory().expect("open");
    db.conn
        .execute_batch(
            "INSERT INTO stopped_resources (app, kind, name) VALUES ('a', 'externalvolume', 'v');
             INSERT INTO stopped_resources (app, kind, name) VALUES ('a', 'deployment', 'd');
             INSERT INTO faults (id, app, resource_type, resource_name, kind, timestamp, description)
                VALUES ('f1', 'a', 'externalservice', 's', 'k', 0, 'd');",
        )
        .expect("insert old records");
    db.conn
        .execute_batch(SQL_V75)
        .expect("the migration applies to old records");
    let kinds: Vec<String> = db
        .conn
        .prepare("SELECT kind FROM stopped_resources ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(kinds, ["deployment", "external_volume"]);
    let fault_type: String = db
        .conn
        .query_row("SELECT resource_type FROM faults", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fault_type, "external_service");
}

// spec: CNR#server-identity
#[test]
fn a_cached_canopy_server_id_with_no_recorded_key_is_dropped() {
    let db = Db::open_in_memory_through(75).expect("open");
    db.conn
        .execute("UPDATE canopy_settings SET server_id = 'cached'", [])
        .expect("cache an identifier before keys were recorded");

    db.finish_migrations().expect("the later migrations apply");

    let (id, key): (Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT server_id, server_id_key FROM canopy_settings",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!((id, key), (None, None));
}

// spec: GEN#history-entries
// spec: SEC#generation-history
#[test]
fn single_parameter_history_rows_move_to_the_per_parameter_table() {
    let db = Db::open_in_memory_through(81).expect("open at v81");
    db.conn
        .execute_batch(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation)
                 VALUES ('web', 0, 0, 5);
             INSERT INTO generations (app, generation, created_at, kind, bundle_hash)
                 VALUES ('web', 1, 't', 'register', 'h');
             INSERT INTO generations (app, generation, created_at, kind, param_name, previous_value, new_value, bundle_hash)
                 VALUES ('web', 2, 't', 'param_set', 'mode', NULL, 'x', 'h');
             INSERT INTO generations (app, generation, created_at, kind, param_name, previous_value, new_value, bundle_hash)
                 VALUES ('web', 3, 't', 'script_update', 'mode', 'x', 'y', 'h');
             INSERT INTO generations (app, generation, created_at, kind, param_name, previous_value, new_value, bundle_hash)
                 VALUES ('web', 4, 't', 'param_unset', 'mode', 'y', NULL, 'h');
             INSERT INTO generations (app, generation, created_at, kind, param_name, new_value_ciphertext, bundle_hash)
                 VALUES ('web', 5, 't', 'param_set', 'key', x'0102', 'h');",
        )
        .expect("seed v81 data");

    db.finish_migrations().expect("migrate to the latest");

    let kinds: Vec<String> = db
        .conn
        .prepare("SELECT kind FROM generations WHERE app = 'web' ORDER BY generation")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        kinds,
        [
            "register",
            "param_change",
            "script_update",
            "param_change",
            "param_change"
        ]
    );

    type Row = (i64, String, Option<String>, Option<String>, Option<Vec<u8>>);
    let rows: Vec<Row> = db
        .conn
        .prepare(
            "SELECT generation, param_name, previous_value, new_value, new_value_ciphertext
             FROM generation_params WHERE app = 'web' ORDER BY generation",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (2, "mode".into(), None, Some("x".into()), None),
            (3, "mode".into(), Some("x".into()), Some("y".into()), None),
            (4, "mode".into(), Some("y".into()), None, None),
            (5, "key".into(), None, None, Some(vec![1, 2])),
        ]
    );

    let dropped: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('generations')
             WHERE name IN ('param_name', 'previous_value', 'new_value',
                            'previous_value_ciphertext', 'new_value_ciphertext')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dropped, 0, "the old per-generation columns are gone");
}

// spec: ACI#install
#[test]
fn an_app_part_way_through_an_install_is_installed_and_the_removed_actions_leave_the_queue() {
    let db = Db::open_in_memory_through(82).expect("open at v82");
    db.conn
        .execute_batch(
            "INSERT INTO registered_apps (name, installed, uninstalling, installing, current_generation)
                 VALUES ('half', 0, 0, 1, 1), ('fresh', 0, 0, 0, 1), ('live', 1, 0, 0, 1);
             INSERT INTO queued_operations
                 (operation_id, app, action, trigger, source_generation, target_generation, params_ciphertext)
                 VALUES ('boot', 'live', 'start', 'boot', 1, 1, x'00'),
                        ('after-install', 'live', 'start', 'install', 1, 1, x'00'),
                        ('install', 'fresh', 'install', 'operator', 1, 1, x'00'),
                        ('action', 'live', 'backup', 'operator', 1, 1, x'00');",
        )
        .expect("seed v82 data");

    db.finish_migrations().expect("migrate to the latest");

    let phases: Vec<(String, bool)> = db
        .conn
        .prepare("SELECT name, installed FROM registered_apps ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        phases,
        [
            ("fresh".to_owned(), false),
            ("half".to_owned(), true),
            ("live".to_owned(), true),
        ]
    );
    let has_column: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('registered_apps') WHERE name = 'installing'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_column, 0, "the installing column is gone");

    let queued: Vec<String> = db
        .conn
        .prepare("SELECT operation_id FROM queued_operations ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(queued, ["action"]);
}

// spec: OPS#changes-to-an-app
#[test]
fn requests_queued_for_a_parameter_change_leave_the_queue() {
    let db = Db::open_in_memory_through(83).expect("open at v83");
    db.conn
        .execute_batch(
            "INSERT INTO queued_operations
                 (operation_id, app, action, trigger, source_generation, target_generation, params_ciphertext)
                 VALUES ('action', 'live', 'backup', 'operator', 1, 1, x'00'),
                        ('change', 'live', 'mode', 'param_change', 1, 2, x'00');",
        )
        .expect("seed v83 data");

    db.finish_migrations().expect("migrate to the latest");

    let queued: Vec<String> = db
        .conn
        .prepare("SELECT operation_id FROM queued_operations ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(queued, ["action"]);
}
