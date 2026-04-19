use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

use rusqlite::{Connection, Result as SqlResult};
use sha2::{Digest, Sha256};

mod migrations {
    pub mod v14;
    pub mod v20;
}

// Canonical SQL for each migration version, used for tamper detection.
//
// NEVER modify these constants or the SQL files they point to after they have
// been deployed. Each constant is hashed when the migration first runs and the
// hash is stored in schema_version. On every subsequent startup the stored hash
// is compared against the current file content; a mismatch means someone edited
// an existing migration — which is forbidden. Add a new version block instead.
const SQL_V2: &str = include_str!("db/migrations/v02.sql");
const SQL_V3: &str = include_str!("db/migrations/v03.sql");
const SQL_V4: &str = include_str!("db/migrations/v04.sql");
const SQL_V5: &str = include_str!("db/migrations/v05.sql");
const SQL_V6: &str = include_str!("db/migrations/v06.sql");
const SQL_V7: &str = include_str!("db/migrations/v07.sql");
const SQL_V8: &str = include_str!("db/migrations/v08.sql");
const SQL_V9: &str = include_str!("db/migrations/v09.sql");
const SQL_V10: &str = include_str!("db/migrations/v10.sql");
const SQL_V11: &str = include_str!("db/migrations/v11.sql");
const SQL_V12: &str = include_str!("db/migrations/v12.sql");
const SQL_V13: &str = include_str!("db/migrations/v13.sql");
const SQL_V14: &str = migrations::v14::SQL;
const SQL_V15: &str = include_str!("db/migrations/v15.sql");
const SQL_V16: &str = include_str!("db/migrations/v16.sql");
const SQL_V17: &str = include_str!("db/migrations/v17.sql");
const SQL_V18: &str = include_str!("db/migrations/v18.sql");
const SQL_V19: &str = include_str!("db/migrations/v19.sql");
const SQL_V20: &str = migrations::v20::SQL;
const SQL_V21: &str = include_str!("db/migrations/v21.sql");
const SQL_V22: &str = include_str!("db/migrations/v22.sql");
const SQL_V23: &str = include_str!("db/migrations/v23.sql");
const SQL_V24: &str = include_str!("db/migrations/v24.sql");
const SQL_V25: &str = include_str!("db/migrations/v25.sql");
const SQL_V26: &str = include_str!("db/migrations/v26.sql");

const MIGRATIONS: &[(i64, &str)] = &[
    (2, SQL_V2),
    (3, SQL_V3),
    (4, SQL_V4),
    (5, SQL_V5),
    (6, SQL_V6),
    (7, SQL_V7),
    (8, SQL_V8),
    (9, SQL_V9),
    (10, SQL_V10),
    (11, SQL_V11),
    (12, SQL_V12),
    (13, SQL_V13),
    (14, SQL_V14),
    (15, SQL_V15),
    (16, SQL_V16),
    (17, SQL_V17),
    (18, SQL_V18),
    (19, SQL_V19),
    (20, SQL_V20),
    (21, SQL_V21),
    (22, SQL_V22),
    (23, SQL_V23),
    (24, SQL_V24),
    (25, SQL_V25),
    (26, SQL_V26),
];

fn migration_hash(sql: &str) -> String {
    let digest = Sha256::digest(sql.as_bytes());
    let mut s = String::with_capacity(digest.len() * 2);
    use std::fmt::Write as _;
    for b in digest.as_slice() {
        write!(s, "{b:02x}").expect("write to String is infallible");
    }
    s
}

// r[impl history.persistence]
// r[impl history.storage]
pub struct Db {
    pub conn: Connection,
}

impl Db {
    // r[infra.db.file-permissions]
    pub fn open(path: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if path.exists() {
            let mode = path.metadata()?.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(format!(
                    "database file {} has insecure permissions (0{:o}); expected 0600",
                    path.display(),
                    mode
                )
                .into());
            }
        }

        let conn = Connection::open(path)?;

        // r[infra.db.busy-timeout]
        conn.busy_timeout(Duration::from_secs(5))?;

        if let Ok(meta) = path.metadata()
            && meta.permissions().mode() & 0o777 != 0o600
        {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }

        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    // NEVER edit or delete an existing migration block, and NEVER modify an
    // existing SQL file under db/migrations/. Once a migration has shipped the
    // schema_version row for it exists in production databases, and the stored
    // hash will no longer match the edited content — causing a panic on startup.
    // Always add a new version block and a new SQL/RS file instead.
    fn migrate(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version     INTEGER NOT NULL,
                migrated_at TEXT,
                hash        TEXT
            );",
        )?;

        // Upgrade existing schema_version tables that predate hash tracking.
        // SQLite errors if a column already exists; we silently ignore those.
        let _ = self
            .conn
            .execute_batch("ALTER TABLE schema_version ADD COLUMN migrated_at TEXT;");
        let _ = self
            .conn
            .execute_batch("ALTER TABLE schema_version ADD COLUMN hash TEXT;");

        let version: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let tx = self.conn.unchecked_transaction()?;

        if version < 2 {
            // r[impl identity.stable]
            // r[impl identity.components]
            // r[impl history.world.entries]
            // r[impl history.operations.entries]
            // r[impl history.action-log.entries]
            // r[impl operation.lifecycle.events]
            // r[impl barrier.replay]
            self.conn.execute_batch(SQL_V2)?;
            self.record_migration(2, SQL_V2)?;
        }

        if version < 3 {
            self.conn.execute_batch(SQL_V3)?;
            self.record_migration(3, SQL_V3)?;
        }

        if version < 4 {
            // i[app.persist]
            self.conn.execute_batch(SQL_V4)?;
            self.record_migration(4, SQL_V4)?;
        }

        if version < 5 {
            // i[key.authorize]
            self.conn.execute_batch(SQL_V5)?;
            self.record_migration(5, SQL_V5)?;
        }

        if version < 6 {
            // i[param.store]
            self.conn.execute_batch(SQL_V6)?;
            self.record_migration(6, SQL_V6)?;
        }

        if version < 7 {
            self.conn.execute_batch(SQL_V7)?;
            self.record_migration(7, SQL_V7)?;
        }

        if version < 8 {
            self.conn.execute_batch(SQL_V8)?;
            self.record_migration(8, SQL_V8)?;
        }

        if version < 9 {
            // i[fault.record]
            self.conn.execute_batch(SQL_V9)?;
            self.record_migration(9, SQL_V9)?;
        }

        if version < 10 {
            self.conn.execute_batch(SQL_V10)?;
            self.record_migration(10, SQL_V10)?;
        }

        if version < 11 {
            self.conn.execute_batch(SQL_V11)?;
            self.record_migration(11, SQL_V11)?;
        }

        if version < 12 {
            self.conn.execute_batch(SQL_V12)?;
            self.record_migration(12, SQL_V12)?;
        }

        if version < 13 {
            self.conn.execute_batch(SQL_V13)?;
            self.record_migration(13, SQL_V13)?;
        }

        if version < 14 {
            migrations::v14::run(&self.conn)?;
            self.record_migration(14, SQL_V14)?;
        }

        if version < 15 {
            self.conn.execute_batch(SQL_V15)?;
            self.record_migration(15, SQL_V15)?;
        }

        if version < 16 {
            self.conn.execute_batch(SQL_V16)?;
            self.record_migration(16, SQL_V16)?;
        }

        if version < 17 {
            self.conn.execute_batch(SQL_V17)?;
            self.record_migration(17, SQL_V17)?;
        }

        if version < 18 {
            self.conn.execute_batch(SQL_V18)?;
            self.record_migration(18, SQL_V18)?;
        }

        if version < 19 {
            self.conn.execute_batch(SQL_V19)?;
            self.record_migration(19, SQL_V19)?;
        }

        if version < 20 {
            // r[impl generation.history]
            // r[impl generation.script-storage]
            migrations::v20::run(&self.conn)?;
            self.record_migration(20, SQL_V20)?;
        }

        if version < 21 {
            // r[impl operation.lifecycle.generations]
            self.conn.execute_batch(SQL_V21)?;
            self.record_migration(21, SQL_V21)?;
        }

        if version < 22 {
            // r[impl schedule.state]
            self.conn.execute_batch(SQL_V22)?;
            self.record_migration(22, SQL_V22)?;
        }

        if version < 23 {
            // i[impl backup.app.register]
            self.conn.execute_batch(SQL_V23)?;
            self.record_migration(23, SQL_V23)?;
        }

        if version < 24 {
            // i[impl backup.strategy.create]
            self.conn.execute_batch(SQL_V24)?;
            self.record_migration(24, SQL_V24)?;
        }

        if version < 25 {
            // r[impl backup.execution]
            self.conn.execute_batch(SQL_V25)?;
            self.record_migration(25, SQL_V25)?;
        }

        if version < 26 {
            self.conn.execute_batch(SQL_V26)?;
            self.record_migration(26, SQL_V26)?;
        }

        tx.commit()?;

        self.verify_migrations()?;

        Ok(())
    }

    fn record_migration(&self, version: i64, sql: &str) -> SqlResult<()> {
        let hash = migration_hash(sql);
        let now = jiff::Timestamp::now().to_string();
        self.conn.execute(
            "INSERT INTO schema_version (version, migrated_at, hash) VALUES (?1, ?2, ?3)",
            rusqlite::params![version, now, hash],
        )?;
        Ok(())
    }

    fn verify_migrations(&self) -> SqlResult<()> {
        for &(ver, sql) in MIGRATIONS {
            let expected = migration_hash(sql);
            match self.conn.query_row(
                "SELECT hash FROM schema_version WHERE version = ?1",
                [ver],
                |r| r.get::<_, Option<String>>(0),
            ) {
                Ok(Some(stored)) => {
                    if stored != expected {
                        panic!(
                            "Migration {ver} has been tampered with!\n\
                             Stored hash:   {stored}\n\
                             Expected hash: {expected}\n\
                             Never edit existing migration files — \
                             add a new version block instead."
                        );
                    }
                }
                Ok(None) => {
                    // Applied before hash tracking was introduced; seal it now.
                    self.conn.execute(
                        "UPDATE schema_version \
                         SET hash = ?1, migrated_at = COALESCE(migrated_at, '(pre-hash-tracking)') \
                         WHERE version = ?2",
                        rusqlite::params![expected, ver],
                    )?;
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    // Not yet applied — nothing to verify.
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Schedule helpers
// ---------------------------------------------------------------------------

pub struct ScheduleRow {
    pub app: String,
    pub action: String,
    pub cronexpr: String,
    pub last_fired_at: Option<String>,
}

// r[impl schedule.state]
pub fn upsert_schedule_fired(
    db: &Db,
    app: &str,
    action: &str,
    cronexpr: &str,
    fired_at: &str,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO action_schedules (app, action, cronexpr, last_fired_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (app, action, cronexpr)
         DO UPDATE SET last_fired_at = excluded.last_fired_at",
        rusqlite::params![app, action, cronexpr, fired_at],
    )?;
    Ok(())
}

pub fn list_schedules(db: &Db, app: &str) -> rusqlite::Result<Vec<ScheduleRow>> {
    let mut stmt = db.conn.prepare(
        "SELECT app, action, cronexpr, last_fired_at
         FROM action_schedules WHERE app = ?1",
    )?;
    let rows = stmt.query_map([app], |row| {
        Ok(ScheduleRow {
            app: row.get(0)?,
            action: row.get(1)?,
            cronexpr: row.get(2)?,
            last_fired_at: row.get(3)?,
        })
    })?;
    rows.collect()
}

pub fn list_all_schedules(db: &Db) -> rusqlite::Result<Vec<ScheduleRow>> {
    let mut stmt = db
        .conn
        .prepare("SELECT app, action, cronexpr, last_fired_at FROM action_schedules")?;
    let rows = stmt.query_map([], |row| {
        Ok(ScheduleRow {
            app: row.get(0)?,
            action: row.get(1)?,
            cronexpr: row.get(2)?,
            last_fired_at: row.get(3)?,
        })
    })?;
    rows.collect()
}

// r[impl schedule.prune]
pub fn prune_schedules(
    db: &Db,
    app: &str,
    valid_pairs: &[(String, String)],
) -> rusqlite::Result<()> {
    let current = list_schedules(db, app)?;
    for row in current {
        let key = (row.action.clone(), row.cronexpr.clone());
        if !valid_pairs.contains(&key) {
            db.conn.execute(
                "DELETE FROM action_schedules
                 WHERE app = ?1 AND action = ?2 AND cronexpr = ?3",
                rusqlite::params![app, row.action, row.cronexpr],
            )?;
        }
    }
    Ok(())
}

/// Ensure all declared schedules have rows (inserts missing ones without
/// overwriting existing `last_fired_at`).
pub fn ensure_schedules(db: &Db, app: &str, pairs: &[(String, String)]) -> rusqlite::Result<()> {
    for (action, cronexpr) in pairs {
        db.conn.execute(
            "INSERT OR IGNORE INTO action_schedules (app, action, cronexpr)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![app, action, cronexpr],
        )?;
    }
    Ok(())
}

pub fn delete_schedules_for_app(db: &Db, app: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM action_schedules WHERE app = ?1",
        rusqlite::params![app],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
