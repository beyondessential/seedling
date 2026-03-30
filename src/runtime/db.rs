use rusqlite::{Connection, Result as SqlResult};
use std::path::Path;

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
            "
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS world_observations (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                recorded_at INTEGER NOT NULL,
                app         TEXT    NOT NULL,
                kind        TEXT    NOT NULL,
                name        TEXT,
                ordinal     INTEGER NOT NULL DEFAULT 0,
                obs_kind    TEXT    NOT NULL,
                payload     TEXT    NOT NULL
            );

            CREATE TABLE IF NOT EXISTS autonomous_operations (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                recorded_at  INTEGER NOT NULL,
                app          TEXT    NOT NULL,
                kind         TEXT    NOT NULL,
                name         TEXT,
                ordinal      INTEGER NOT NULL DEFAULT 0,
                operation    TEXT    NOT NULL,
                provenance   TEXT    NOT NULL,
                outcome      TEXT,
                completed_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS action_log (
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
            );
            ",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_succeeds() {
        let db = Db::open_in_memory().expect("in-memory DB should open");
        // Simple connectivity check
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .expect("schema_version should exist");
        assert_eq!(n, 0);
    }

    #[test]
    fn migrate_twice_is_idempotent() {
        let db = Db::open_in_memory().expect("open");
        // Running migrate again should not error
        db.migrate().expect("second migration should not error");
    }

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
