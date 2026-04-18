use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

use rusqlite::{Connection, Result as SqlResult};

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

        let tx = self.conn.unchecked_transaction()?;

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

        if version < 9 {
            // i[fault.record]
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS faults (
                    id            TEXT PRIMARY KEY,
                    app           TEXT NOT NULL,
                    resource_type TEXT,
                    resource_name TEXT,
                    instance_id   TEXT,
                    kind          TEXT NOT NULL,
                    timestamp     TEXT NOT NULL,
                    description   TEXT NOT NULL,
                    cleared_at    TEXT
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (9);")?;
        }

        if version < 10 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS dynamic_resources (
                    instance_id   TEXT PRIMARY KEY,
                    app           TEXT NOT NULL,
                    operation_id  TEXT NOT NULL,
                    kind          TEXT NOT NULL,
                    display_name  TEXT NOT NULL
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (10);")?;
        }

        if version < 11 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS allowed_registries (
                    registry TEXT PRIMARY KEY
                );
                INSERT OR IGNORE INTO allowed_registries (registry) VALUES ('docker.io');
                INSERT OR IGNORE INTO allowed_registries (registry) VALUES ('ghcr.io');",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (11);")?;
        }

        if version < 12 {
            self.conn.execute_batch(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_singleton_unique
                     ON resource_instances (app, kind, name)
                     WHERE is_scaled = 0;",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (12);")?;
        }

        if version < 13 {
            self.conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_world_observations_instance
                     ON world_observations (instance_id, recorded_at);
                 CREATE INDEX IF NOT EXISTS idx_autonomous_operations_instance
                     ON autonomous_operations (instance_id, recorded_at);
                 CREATE INDEX IF NOT EXISTS idx_action_log_operation
                     ON action_log (operation_id, call_index);
                 CREATE INDEX IF NOT EXISTS idx_faults_active_app
                     ON faults (app, cleared_at)
                     WHERE cleared_at IS NULL;",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (13);")?;
        }

        if version < 14 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS app_versions (
                    id         TEXT PRIMARY KEY,
                    app        TEXT NOT NULL,
                    script     TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                ALTER TABLE registered_apps ADD COLUMN current_version_id TEXT;",
            )?;
            // Backfill: create a version row for every existing registered app
            // and point current_version_id at it.
            {
                let mut sel = self
                    .conn
                    .prepare("SELECT name, script FROM registered_apps")?;
                let apps: Vec<(String, String)> = sel
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                drop(sel);
                let now = jiff::Timestamp::now().to_string();
                for (name, script) in apps {
                    let vid = uuid::Uuid::new_v4().to_string();
                    self.conn.execute(
                        "INSERT INTO app_versions (id, app, script, created_at) VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![vid, name, script, now],
                    )?;
                    self.conn.execute(
                        "UPDATE registered_apps SET current_version_id = ?1 WHERE name = ?2",
                        rusqlite::params![vid, name],
                    )?;
                }
            }
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (14);")?;
        }

        if version < 15 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS scaling_decisions (
                    app        TEXT NOT NULL,
                    deployment TEXT NOT NULL,
                    scale      INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (app, deployment)
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (15);")?;
        }

        if version < 16 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS site_volumes (
                    name       TEXT    PRIMARY KEY,
                    kind       TEXT    NOT NULL,
                    host_path  TEXT,
                    created_at TEXT    NOT NULL
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (16);")?;
        }
        if version < 17 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS external_volume_mappings (
                    app             TEXT    NOT NULL,
                    external_name   TEXT    NOT NULL,
                    target_kind     TEXT    NOT NULL,
                    target_app      TEXT,
                    target_volume   TEXT    NOT NULL,
                    read_only       INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (app, external_name)
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (17);")?;
        }
        if version < 18 {
            self.conn
                .execute_batch("ALTER TABLE registered_apps DROP COLUMN script;")?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (18);")?;
        }
        if version < 19 {
            self.conn.execute_batch(
                "ALTER TABLE site_volumes ADD COLUMN source_app TEXT;
                 ALTER TABLE site_volumes ADD COLUMN source_volume TEXT;",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (19);")?;
        }
        if version < 20 {
            // r[impl generation.history]
            // r[impl generation.script-storage]
            // Replace UUID-keyed app_versions with content-addressed script_bodies
            // and a per-app generations history.
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS script_bodies (
                    hash TEXT PRIMARY KEY,
                    body TEXT NOT NULL
                );
                 CREATE TABLE IF NOT EXISTS generations (
                    app             TEXT    NOT NULL,
                    generation      INTEGER NOT NULL,
                    created_at      TEXT    NOT NULL,
                    kind            TEXT    NOT NULL,
                    param_name      TEXT,
                    previous_value  TEXT,
                    new_value       TEXT,
                    script_hash     TEXT    NOT NULL,
                    operation_id    TEXT,
                    outcome         TEXT,
                    outcome_error   TEXT,
                    PRIMARY KEY (app, generation)
                 );
                 CREATE INDEX IF NOT EXISTS idx_generations_app
                     ON generations (app, generation DESC);
                 ALTER TABLE registered_apps ADD COLUMN current_generation INTEGER NOT NULL DEFAULT 0;",
            )?;

            // Backfill: convert each app's current state into a Register entry
            // at generation 1, plus one ParamSet entry per existing param.
            use sha2::{Digest, Sha256};
            let now = jiff::Timestamp::now().to_string();
            fn hex_of(digest: &[u8]) -> String {
                use std::fmt::Write as FmtWrite;
                let mut s = String::with_capacity(digest.len() * 2);
                for b in digest {
                    write!(s, "{b:02x}").expect("write to String is infallible");
                }
                s
            }

            let app_rows: Vec<(String, Option<String>)> = {
                let mut stmt = self
                    .conn
                    .prepare("SELECT name, current_version_id FROM registered_apps")?;
                stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?
            };

            for (app, version_id) in app_rows {
                let Some(vid) = version_id else {
                    // Apps that never had a script row — leave at generation 0;
                    // they would have failed to load anyway. Logged at load time.
                    continue;
                };
                let script: String = match self.conn.query_row(
                    "SELECT script FROM app_versions WHERE id = ?1",
                    [&vid],
                    |row| row.get(0),
                ) {
                    Ok(s) => s,
                    Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                    Err(e) => return Err(e),
                };

                let digest = Sha256::digest(script.as_bytes());
                let hash = hex_of(&digest);
                self.conn.execute(
                    "INSERT OR IGNORE INTO script_bodies (hash, body) VALUES (?1, ?2)",
                    rusqlite::params![hash, script],
                )?;

                self.conn.execute(
                    "INSERT INTO generations
                        (app, generation, created_at, kind, script_hash)
                     VALUES (?1, 1, ?2, 'register', ?3)",
                    rusqlite::params![app, now, hash],
                )?;

                let params: Vec<(String, String)> = {
                    let mut pstmt = self.conn.prepare(
                        "SELECT param_name, value FROM params WHERE app_name = ?1 ORDER BY param_name",
                    )?;
                    pstmt
                        .query_map([&app], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })?
                        .collect::<rusqlite::Result<_>>()?
                };

                let mut current = 1u64;
                for (param_name, value) in params {
                    current += 1;
                    self.conn.execute(
                        "INSERT INTO generations
                            (app, generation, created_at, kind, param_name,
                             previous_value, new_value, script_hash)
                         VALUES (?1, ?2, ?3, 'param_set', ?4, NULL, ?5, ?6)",
                        rusqlite::params![app, current as i64, now, param_name, value, hash],
                    )?;
                }

                self.conn.execute(
                    "UPDATE registered_apps SET current_generation = ?1 WHERE name = ?2",
                    rusqlite::params![current as i64, app],
                )?;
            }

            // Drop the legacy version-tracking columns / table.
            self.conn
                .execute_batch("ALTER TABLE registered_apps DROP COLUMN current_version_id;")?;
            self.conn.execute_batch("DROP TABLE app_versions;")?;

            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (20);")?;
        }
        if version < 21 {
            // r[impl operation.lifecycle.generations]
            // Plumb source/target generation through the operation record so
            // replay can reconstruct the right AppDef and `old`.
            self.conn.execute_batch(
                "ALTER TABLE current_operation
                    ADD COLUMN source_generation INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE current_operation
                    ADD COLUMN target_generation INTEGER NOT NULL DEFAULT 0;",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (21);")?;
        }
        if version < 22 {
            // r[impl schedule.state]
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS action_schedules (
                    app          TEXT NOT NULL,
                    action       TEXT NOT NULL,
                    cronexpr     TEXT NOT NULL,
                    last_fired_at TEXT,
                    PRIMARY KEY (app, action, cronexpr)
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (22);")?;
        }
        if version < 23 {
            // i[impl backup.app.register]
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS backup_apps (
                    name  TEXT PRIMARY KEY,
                    app   TEXT UNIQUE NOT NULL
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (23);")?;
        }
        if version < 24 {
            // i[impl backup.strategy.create]
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS backup_strategies (
                    name     TEXT PRIMARY KEY,
                    via      TEXT NOT NULL,
                    schedule TEXT NOT NULL,
                    volumes  TEXT NOT NULL
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (24);")?;
        }
        if version < 25 {
            // r[impl backup.execution]
            self.conn
                .execute_batch("ALTER TABLE backup_strategies ADD COLUMN last_fired_at TEXT;")?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (25);")?;
        }
        tx.commit()?;
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
