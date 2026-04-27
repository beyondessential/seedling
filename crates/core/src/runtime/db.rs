use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

use rusqlite::{Connection, Result as SqlResult};
use seedling_protocol::names::{ActionName, AppName};
use sha2::{Digest, Sha256};

mod migrations {
    pub mod v14;
    pub mod v20;
}

struct Migration {
    version: i64,
    sql: &'static str,
    /// Override for migrations that need Rust logic alongside the SQL.
    /// If None, `execute_batch(sql)` is used.
    custom_run: Option<fn(&Connection) -> SqlResult<()>>,
}

// Canonical SQL for each migration version, used for tamper detection.
//
// NEVER modify these constants or the SQL files they point to after they have
// been deployed. Each is hashed when the migration first runs and the hash is
// stored in schema_version. A mismatch on startup means someone edited an
// existing migration — which is forbidden. Add a new version block instead.

// r[impl identity.stable]
// r[impl identity.components]
// r[impl history.world.entries]
// r[impl history.operations.entries]
// r[impl history.action-log.entries]
// r[impl operation.lifecycle.events]
// r[impl barrier.replay]
const SQL_V2: &str = include_str!("db/migrations/v02.sql");
const SQL_V3: &str = include_str!("db/migrations/v03.sql");
// i[app.persist]
const SQL_V4: &str = include_str!("db/migrations/v04.sql");
// i[key.authorize]
const SQL_V5: &str = include_str!("db/migrations/v05.sql");
// i[param.store]
const SQL_V6: &str = include_str!("db/migrations/v06.sql");
const SQL_V7: &str = include_str!("db/migrations/v07.sql");
const SQL_V8: &str = include_str!("db/migrations/v08.sql");
// i[fault.record]
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
// r[impl generation.history]
// r[impl generation.script-storage]
const SQL_V20: &str = migrations::v20::SQL;
// r[impl operation.lifecycle.generations]
const SQL_V21: &str = include_str!("db/migrations/v21.sql");
// r[impl schedule.state]
const SQL_V22: &str = include_str!("db/migrations/v22.sql");
// i[impl backup.app.register]
const SQL_V23: &str = include_str!("db/migrations/v23.sql");
// i[impl backup.strategy.create]
const SQL_V24: &str = include_str!("db/migrations/v24.sql");
// r[impl backup.execution]
const SQL_V25: &str = include_str!("db/migrations/v25.sql");
const SQL_V26: &str = include_str!("db/migrations/v26.sql");
// i[impl deployment.restart]
const SQL_V27: &str = include_str!("db/migrations/v27.sql");
// i[impl resource.stop]
const SQL_V28: &str = include_str!("db/migrations/v28.sql");
// r[impl secret.storage]
// r[impl secret.history]
const SQL_V29: &str = include_str!("db/migrations/v29.sql");
// i[impl action.invoke.install] r[impl operation.params]
const SQL_V30: &str = include_str!("db/migrations/v30.sql");
// i[impl backup.app.register]
const SQL_V31: &str = include_str!("db/migrations/v31.sql");
// r[impl operation.params]
const SQL_V32: &str = include_str!("db/migrations/v32.sql");
// r[impl template.persist]
const SQL_V33: &str = include_str!("db/migrations/v33.sql");
// r[impl operation.cancel]
const SQL_V34: &str = include_str!("db/migrations/v34.sql");
// r[impl image.pin] r[impl image.track]
const SQL_V35: &str = include_str!("db/migrations/v35.sql");
// r[impl image.track]
const SQL_V36: &str = include_str!("db/migrations/v36.sql");
// r[impl image.pin.expiry]
const SQL_V37: &str = include_str!("db/migrations/v37.sql");
// r[impl service.site] r[impl service.external.mapping.events]
const SQL_V38: &str = include_str!("db/migrations/v38.sql");
// r[impl service.external.mapping.events]
const SQL_V39: &str = include_str!("db/migrations/v39.sql");
// r[impl service.site]
const SQL_V40: &str = include_str!("db/migrations/v40.sql");
// l[impl rt.signal]
const SQL_V41: &str = include_str!("db/migrations/v41.sql");
// r[impl tls.dns-provider.lifecycle]
// r[impl tls.csr.flow]
// r[impl tls.strategy.manual]
// r[impl tls.strategy.acme-dns]
// r[impl tls.policy.apply]
const SQL_V42: &str = include_str!("db/migrations/v42.sql");
// r[impl tls.acme.account.persist]
// r[impl tls.cert.metadata]
const SQL_V43: &str = include_str!("db/migrations/v43.sql");
// r[impl tls.settings.contact-email]
const SQL_V44: &str = include_str!("db/migrations/v44.sql");
// r[impl tls.cert.attempt-log]
// r[impl tls.cert.retry-block]
const SQL_V45: &str = include_str!("db/migrations/v45.sql");
// r[impl tls.cert.force-retry]
const SQL_V46: &str = include_str!("db/migrations/v46.sql");
// r[impl tls.cert.ari]
const SQL_V47: &str = include_str!("db/migrations/v47.sql");

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 2,
        sql: SQL_V2,
        custom_run: None,
    },
    Migration {
        version: 3,
        sql: SQL_V3,
        custom_run: None,
    },
    Migration {
        version: 4,
        sql: SQL_V4,
        custom_run: None,
    },
    Migration {
        version: 5,
        sql: SQL_V5,
        custom_run: None,
    },
    Migration {
        version: 6,
        sql: SQL_V6,
        custom_run: None,
    },
    Migration {
        version: 7,
        sql: SQL_V7,
        custom_run: None,
    },
    Migration {
        version: 8,
        sql: SQL_V8,
        custom_run: None,
    },
    Migration {
        version: 9,
        sql: SQL_V9,
        custom_run: None,
    },
    Migration {
        version: 10,
        sql: SQL_V10,
        custom_run: None,
    },
    Migration {
        version: 11,
        sql: SQL_V11,
        custom_run: None,
    },
    Migration {
        version: 12,
        sql: SQL_V12,
        custom_run: None,
    },
    Migration {
        version: 13,
        sql: SQL_V13,
        custom_run: None,
    },
    Migration {
        version: 14,
        sql: SQL_V14,
        custom_run: Some(migrations::v14::run),
    },
    Migration {
        version: 15,
        sql: SQL_V15,
        custom_run: None,
    },
    Migration {
        version: 16,
        sql: SQL_V16,
        custom_run: None,
    },
    Migration {
        version: 17,
        sql: SQL_V17,
        custom_run: None,
    },
    Migration {
        version: 18,
        sql: SQL_V18,
        custom_run: None,
    },
    Migration {
        version: 19,
        sql: SQL_V19,
        custom_run: None,
    },
    Migration {
        version: 20,
        sql: SQL_V20,
        custom_run: Some(migrations::v20::run),
    },
    Migration {
        version: 21,
        sql: SQL_V21,
        custom_run: None,
    },
    Migration {
        version: 22,
        sql: SQL_V22,
        custom_run: None,
    },
    Migration {
        version: 23,
        sql: SQL_V23,
        custom_run: None,
    },
    Migration {
        version: 24,
        sql: SQL_V24,
        custom_run: None,
    },
    Migration {
        version: 25,
        sql: SQL_V25,
        custom_run: None,
    },
    Migration {
        version: 26,
        sql: SQL_V26,
        custom_run: None,
    },
    Migration {
        version: 27,
        sql: SQL_V27,
        custom_run: None,
    },
    Migration {
        version: 28,
        sql: SQL_V28,
        custom_run: None,
    },
    Migration {
        version: 29,
        sql: SQL_V29,
        custom_run: None,
    },
    Migration {
        version: 30,
        sql: SQL_V30,
        custom_run: None,
    },
    Migration {
        version: 31,
        sql: SQL_V31,
        custom_run: None,
    },
    Migration {
        version: 32,
        sql: SQL_V32,
        custom_run: None,
    },
    Migration {
        version: 33,
        sql: SQL_V33,
        custom_run: None,
    },
    Migration {
        version: 34,
        sql: SQL_V34,
        custom_run: None,
    },
    Migration {
        version: 35,
        sql: SQL_V35,
        custom_run: None,
    },
    Migration {
        version: 36,
        sql: SQL_V36,
        custom_run: None,
    },
    Migration {
        version: 37,
        sql: SQL_V37,
        custom_run: None,
    },
    Migration {
        version: 38,
        sql: SQL_V38,
        custom_run: None,
    },
    Migration {
        version: 39,
        sql: SQL_V39,
        custom_run: None,
    },
    Migration {
        version: 40,
        sql: SQL_V40,
        custom_run: None,
    },
    Migration {
        version: 41,
        sql: SQL_V41,
        custom_run: None,
    },
    Migration {
        version: 42,
        sql: SQL_V42,
        custom_run: None,
    },
    Migration {
        version: 43,
        sql: SQL_V43,
        custom_run: None,
    },
    Migration {
        version: 44,
        sql: SQL_V44,
        custom_run: None,
    },
    Migration {
        version: 45,
        sql: SQL_V45,
        custom_run: None,
    },
    Migration {
        version: 46,
        sql: SQL_V46,
        custom_run: None,
    },
    Migration {
        version: 47,
        sql: SQL_V47,
        custom_run: None,
    },
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
        // Foreign keys are a connection-scoped pragma in SQLite and are off
        // by default. Enable them so REFERENCES clauses (currently only on
        // site-service tables; other tables will adopt them in a follow-up)
        // are actually enforced.
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    // NEVER edit or delete an existing migration block, and NEVER modify an
    // existing SQL file under db/migrations/. Once a migration has shipped the
    // schema_version row for it exists in production databases, and the stored
    // hash will no longer match the edited content — causing a panic on startup.
    // Always add a new Migration entry and a new SQL/RS file instead.
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

        for m in MIGRATIONS {
            if version < m.version {
                match m.custom_run {
                    Some(f) => f(&self.conn)?,
                    None => self.conn.execute_batch(m.sql)?,
                }
                self.record_migration(m.version, m.sql)?;
            }
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
        for m in MIGRATIONS {
            let expected = migration_hash(m.sql);
            match self.conn.query_row(
                "SELECT hash FROM schema_version WHERE version = ?1",
                [m.version],
                |r| r.get::<_, Option<String>>(0),
            ) {
                Ok(Some(stored)) => {
                    if stored != expected {
                        panic!(
                            "Migration {} has been tampered with!\n\
                             Stored hash:   {stored}\n\
                             Expected hash: {expected}\n\
                             Never edit existing migration files — \
                             add a new version block instead.",
                            m.version
                        );
                    }
                }
                Ok(None) => {
                    // Applied before hash tracking was introduced; seal it now.
                    self.conn.execute(
                        "UPDATE schema_version \
                         SET hash = ?1, migrated_at = COALESCE(migrated_at, '(pre-hash-tracking)') \
                         WHERE version = ?2",
                        rusqlite::params![expected, m.version],
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
    pub app: AppName,
    pub action: ActionName,
    pub cronexpr: String,
    pub last_fired_at: Option<String>,
}

// r[impl schedule.state]
pub fn upsert_schedule_fired(
    db: &Db,
    app: &AppName,
    action: &ActionName,
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

pub fn list_schedules(db: &Db, app: &AppName) -> rusqlite::Result<Vec<ScheduleRow>> {
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
    app: &AppName,
    valid_pairs: &[(ActionName, String)],
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
pub fn ensure_schedules(
    db: &Db,
    app: &AppName,
    pairs: &[(ActionName, String)],
) -> rusqlite::Result<()> {
    for (action, cronexpr) in pairs {
        db.conn.execute(
            "INSERT OR IGNORE INTO action_schedules (app, action, cronexpr)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![app, action, cronexpr],
        )?;
    }
    Ok(())
}

pub fn delete_schedules_for_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM action_schedules WHERE app = ?1",
        rusqlite::params![app],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// DbHandle — single-threaded DB actor
// ---------------------------------------------------------------------------

type DbJob = Box<dyn FnOnce(&Db) + Send + 'static>;

/// A cheaply-cloneable handle to a dedicated SQLite database thread.
///
/// All DB access is serialised through a single background `std::thread`.
/// Callers submit closures that receive `&Db` and block synchronously until
/// the closure returns. This eliminates the explicit mutex and the deadlock
/// risks that come with mixing db and other lock orderings.
#[derive(Clone)]
pub struct DbHandle {
    tx: std::sync::mpsc::SyncSender<DbJob>,
}

impl DbHandle {
    pub fn open(path: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let db = Db::open(path)?;
        Ok(Self::from_db(db))
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let db = Db::open_in_memory()?;
        Ok(Self::from_db(db))
    }

    pub fn from_db(db: Db) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<DbJob>(64);
        std::thread::Builder::new()
            .name("seedling-db".into())
            .spawn(move || {
                while let Ok(f) = rx.recv() {
                    f(&db);
                }
            })
            .expect("failed to spawn database thread");
        Self { tx }
    }

    /// Submit `f` to the database thread and block until it returns.
    pub fn call<R>(&self, f: impl FnOnce(&Db) -> R + Send + 'static) -> R
    where
        R: Send + 'static,
    {
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel::<R>(0);
        let job: DbJob = Box::new(move |db| {
            let _ = result_tx.send(f(db));
        });
        self.tx.send(job).expect("database thread has exited");
        result_rx
            .recv()
            .expect("database thread exited without returning result")
    }
}

#[cfg(test)]
mod tests;
