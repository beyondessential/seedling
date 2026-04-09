use rusqlite::{Connection, Result as SqlResult};
use std::path::Path;

// r[impl history.persistence]
// r[impl history.storage]
pub struct Db {
    pub conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL
            );",
        )?;

        let version: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        if version < 2 {
            // Identity overhaul: drop old tables that used (app, kind, name, ordinal)
            // columns; they will be recreated below with instance_id references.
            self.conn.execute_batch(
                "DROP TABLE IF EXISTS world_observations;
                 DROP TABLE IF EXISTS autonomous_operations;
                 DELETE FROM schema_version;",
            )?;
        }

        // r[impl identity.stable]
        // r[impl identity.components]
        // The instance registry is the authoritative list of all resource instances.
        // Each row is created once; the id (32-char hex InstanceId) never changes.
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS resource_instances (
                id           TEXT    PRIMARY KEY,
                app          TEXT    NOT NULL,
                kind         TEXT    NOT NULL,
                name         TEXT,
                is_scaled    INTEGER NOT NULL DEFAULT 0,
                display_name TEXT    NOT NULL,
                created_at   INTEGER NOT NULL
            );",
        )?;

        // r[impl history.world.entries]
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS world_observations (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                recorded_at INTEGER NOT NULL,
                instance_id TEXT    NOT NULL,
                obs_kind    TEXT    NOT NULL,
                payload     TEXT    NOT NULL
            );",
        )?;

        // r[impl history.operations.entries]
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS autonomous_operations (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                recorded_at  INTEGER NOT NULL,
                instance_id  TEXT    NOT NULL,
                operation    TEXT    NOT NULL,
                provenance   TEXT    NOT NULL,
                outcome      TEXT,
                completed_at INTEGER
            );",
        )?;

        // r[impl history.action-log.entries]
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS action_log (
                id                 INTEGER PRIMARY KEY AUTOINCREMENT,
                recorded_at        INTEGER NOT NULL,
                operation_id       TEXT    NOT NULL,
                app                TEXT    NOT NULL,
                action_name        TEXT    NOT NULL,
                call_index         INTEGER NOT NULL,
                call_kind          TEXT    NOT NULL,
                resources          TEXT    NOT NULL,
                barrier_state      TEXT,
                barrier_deadline   INTEGER,
                barrier_satisfied  INTEGER,
                barrier_started_at INTEGER,
                UNIQUE (operation_id, call_index)
            );",
        )?;

        // r[impl operation.lifecycle.events]
        // r[impl barrier.replay]
        // Singleton row (id=1) records the one in-progress lifecycle operation so
        // that a restart can detect it and replay rather than starting fresh.
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS current_operation (
                singleton    INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
                operation_id TEXT    NOT NULL,
                app          TEXT    NOT NULL,
                action_name  TEXT    NOT NULL
            );",
        )?;

        if version < 2 {
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (2);")?;
        }

        if version < 3 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS caddy_state (
                    singleton        INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
                    active_container TEXT    NOT NULL
                );
                CREATE TABLE IF NOT EXISTS caddy_proxy_config (
                    singleton    INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
                    config_json  TEXT    NOT NULL
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (3);")?;
        }

        if version < 4 {
            // i[app.persist]
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS registered_apps (
                    name      TEXT    PRIMARY KEY,
                    script    TEXT    NOT NULL,
                    installed INTEGER NOT NULL DEFAULT 0
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (4);")?;
        }

        if version < 5 {
            // i[key.authorize]
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS authorized_keys (
                    fingerprint TEXT    PRIMARY KEY,
                    label       TEXT    NOT NULL,
                    added_at    INTEGER NOT NULL
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (5);")?;
        }

        if version < 6 {
            // i[param.store]
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS params (
                    app_name   TEXT NOT NULL,
                    param_name TEXT NOT NULL,
                    value      TEXT NOT NULL,
                    PRIMARY KEY (app_name, param_name)
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (6);")?;
        }

        if version < 7 {
            // Add uninstalling column to registered_apps so the reconciler can
            // persist cleanup state across restarts.
            self.conn.execute_batch(
                "ALTER TABLE registered_apps ADD COLUMN uninstalling INTEGER NOT NULL DEFAULT 0;",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (7);")?;
        }

        if version < 8 {
            // Remove UNIQUE constraint on display_name in resource_instances.
            // The constraint was overly broad: Deployment/Job display names are
            // unique because Podman enforces it externally, but Service, Ingress,
            // Volume etc. may share a name with a resource of a different kind
            // (e.g. an Ingress and a Service both named "public"). The silent
            // INSERT OR IGNORE failure caused those resources to never persist
            // a stable instance ID.
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS resource_instances_new (
                    id           TEXT    PRIMARY KEY,
                    app          TEXT    NOT NULL,
                    kind         TEXT    NOT NULL,
                    name         TEXT,
                    is_scaled    INTEGER NOT NULL DEFAULT 0,
                    display_name TEXT    NOT NULL,
                    created_at   INTEGER NOT NULL
                );
                INSERT OR IGNORE INTO resource_instances_new
                    SELECT id, app, kind, name, is_scaled, display_name, created_at
                    FROM resource_instances;
                DROP TABLE resource_instances;
                ALTER TABLE resource_instances_new RENAME TO resource_instances;",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (8);")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(version, 8);
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
        assert_eq!(version, 8);
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
}
