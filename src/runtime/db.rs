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
                display_name TEXT    NOT NULL UNIQUE,
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
        // Simple connectivity check
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .expect("schema_version should exist");
        assert_eq!(n, 1);
    }

    // r[verify history.persistence]
    #[test]
    fn migrate_twice_is_idempotent() {
        let db = Db::open_in_memory().expect("open");
        // Running migrate again should not error
        db.migrate().expect("second migration should not error");
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
