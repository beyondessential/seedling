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
                    read_only  INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT    NOT NULL
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (16);")?;
        }
        if version < 17 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS external_volume_mappings (
                    app             TEXT NOT NULL,
                    external_name   TEXT NOT NULL,
                    target_kind     TEXT NOT NULL,
                    target_app      TEXT,
                    target_volume   TEXT NOT NULL,
                    PRIMARY KEY (app, external_name)
                );",
            )?;
            self.conn
                .execute_batch("INSERT INTO schema_version VALUES (17);")?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
