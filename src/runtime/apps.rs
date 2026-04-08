use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::SystemTime,
};

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::{
    defs::app::App,
    runtime::{db::Db, desired::OperationProgress},
    setup_language,
};

#[derive(Debug)]
pub struct ScriptError(pub String);

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The installation phase of an app. Stored in `registered_apps` and shared
/// with the reconciler via Arc so the reconciler can transition it on cleanup.
#[derive(Debug, Clone, PartialEq)]
pub enum AppPhase {
    NotInstalled,
    Installed,
    Uninstalling,
}

// i[app.status]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStatus {
    NotInstalled,
    Uninstalling,
    Operating { action_name: String },
    Running,
    Degraded,
    Faulted,
}

impl AppStatus {
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Uninstalling => "uninstalling",
            Self::Operating { .. } => "operating",
            Self::Running => "running",
            Self::Degraded => "degraded",
            Self::Faulted => "faulted",
        }
    }
}

pub struct AppEntry {
    pub name: String,
    pub script: String,
    pub app: App,
    /// Shared with the reconciler so it can transition the phase when cleanup completes.
    pub phase: Arc<Mutex<AppPhase>>,
    /// Shared with the reconciler and operation runner to track in-progress ops.
    pub active_progress: Arc<RwLock<Option<OperationProgress>>>,
    /// Wakes the reconciler for an immediate tick.
    pub tick_notify: Arc<Notify>,
    /// Active script-evaluation fault, if the last reload failed.
    /// Cleared on the next successful evaluation.
    pub script_error: Option<(String, DateTime<Utc>)>,
}

pub struct AppRegistry {
    entries: HashMap<String, AppEntry>,
}

impl AppRegistry {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl Default for AppRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AppRegistry {
    pub fn is_registered(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    // i[app.register]
    pub fn register(
        &mut self,
        name: String,
        script: String,
        tick_notify: Arc<Notify>,
    ) -> Result<(), ScriptError> {
        let (app, script_error) = match evaluate_script(&name, &script, &BTreeMap::new()) {
            Ok(a) => (a, None),
            Err(e) => {
                tracing::warn!(app = %name, error = %e, "script has errors at registration; params may need to be set");
                (
                    App::default(),
                    Some((e.to_string(), SystemTime::now().into())),
                )
            }
        };
        self.entries.insert(
            name.clone(),
            AppEntry {
                name,
                script,
                app,
                phase: Arc::new(Mutex::new(AppPhase::NotInstalled)),
                active_progress: Arc::new(RwLock::new(None)),
                tick_notify,
                script_error,
            },
        );
        Ok(())
    }

    pub fn deregister(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    // i[app.update]
    // i[param.set]
    // i[param.unset]
    /// Re-evaluate the script with updated stored params.
    ///
    /// On success the entry's app and script are updated and any active
    /// script-error fault is cleared. On failure the existing AppDef keeps
    /// running and the fault is recorded — the caller always succeeds.
    pub fn reload(&mut self, name: &str, script: String, params: &BTreeMap<String, String>) {
        match evaluate_script(name, &script, params) {
            Ok(app) => {
                if let Some(entry) = self.entries.get_mut(name) {
                    entry.script = script;
                    entry.app = app;
                    entry.script_error = None;
                }
            }
            Err(e) => {
                if let Some(entry) = self.entries.get_mut(name) {
                    entry.script = script;
                    entry.script_error = Some((e.to_string(), SystemTime::now().into()));
                }
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&AppEntry> {
        self.entries.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut AppEntry> {
        self.entries.get_mut(name)
    }

    // i[app.list]
    pub fn list(&self) -> Vec<(String, AppStatus)> {
        let mut result: Vec<_> = self
            .entries
            .values()
            .map(|e| (e.name.clone(), derive_status(e)))
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }

    pub fn status_of(&self, name: &str) -> Option<AppStatus> {
        self.entries.get(name).map(derive_status)
    }

    // i[app.persist]
    pub fn load_from_db(db: &Db, tick_notify: Arc<Notify>) -> rusqlite::Result<Self> {
        let mut registry = Self::new();
        let mut stmt = db.conn.prepare(
            "SELECT name, script, installed, uninstalling FROM registered_apps ORDER BY name",
        )?;
        let rows: Vec<(String, String, bool, bool)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;

        for (name, script, installed, uninstalling) in rows {
            let phase = match (installed, uninstalling) {
                (_, true) => AppPhase::Uninstalling,
                (true, _) => AppPhase::Installed,
                (false, _) => AppPhase::NotInstalled,
            };
            let stored = match load_params_for_app(db, &name) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("failed to load params for app '{name}': {e}");
                    BTreeMap::new()
                }
            };
            let (app, script_error) = match evaluate_script(&name, &script, &stored) {
                Ok(a) => (a, None),
                Err(e) => {
                    tracing::warn!("failed to reload script for app '{name}': {e}");
                    (
                        App::default(),
                        Some((e.to_string(), SystemTime::now().into())),
                    )
                }
            };
            registry.entries.insert(
                name.clone(),
                AppEntry {
                    name,
                    script,
                    app,
                    phase: Arc::new(Mutex::new(phase)),
                    active_progress: Arc::new(RwLock::new(None)),
                    tick_notify: Arc::clone(&tick_notify),
                    script_error,
                },
            );
        }

        Ok(registry)
    }

    // i[app.persist]
    pub fn persist_app(db: &Db, entry: &AppEntry) -> rusqlite::Result<()> {
        let phase = entry.phase.lock();
        let installed = matches!(*phase, AppPhase::Installed | AppPhase::Uninstalling);
        let uninstalling = matches!(*phase, AppPhase::Uninstalling);
        db.conn.execute(
            "INSERT OR REPLACE INTO registered_apps (name, script, installed, uninstalling) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                entry.name,
                entry.script,
                installed as i64,
                uninstalling as i64
            ],
        )?;
        Ok(())
    }

    pub fn remove_app(db: &Db, name: &str) -> rusqlite::Result<()> {
        db.conn
            .execute("DELETE FROM registered_apps WHERE name = ?1", [name])?;
        Ok(())
    }
}

fn derive_status(entry: &AppEntry) -> AppStatus {
    let phase = entry.phase.lock();
    match *phase {
        AppPhase::NotInstalled => AppStatus::NotInstalled,
        AppPhase::Uninstalling => AppStatus::Uninstalling,
        AppPhase::Installed => {
            if entry.active_progress.read().is_some() {
                AppStatus::Operating {
                    action_name: String::new(),
                }
            } else {
                AppStatus::Running
            }
        }
    }
}

/// Update the phase both in the shared Arc and in the database.
pub fn transition_phase(
    phase_arc: &Mutex<AppPhase>,
    new_phase: AppPhase,
    db: &Db,
    app_name: &str,
    _script: &str,
) {
    *phase_arc.lock() = new_phase.clone();
    let installed = matches!(new_phase, AppPhase::Installed | AppPhase::Uninstalling);
    let uninstalling = matches!(new_phase, AppPhase::Uninstalling);
    let _ = db.conn.execute(
        "UPDATE registered_apps SET installed = ?1, uninstalling = ?2 WHERE name = ?3",
        rusqlite::params![installed as i64, uninstalling as i64, app_name],
    );
}

fn evaluate_script(
    name: &str,
    script: &str,
    params: &BTreeMap<String, String>,
) -> Result<App, ScriptError> {
    let (engine, mut scope, app) = setup_language();
    // i[param.store] — pre-populate stored values so is_set()/value() work
    // during script evaluation. AppDef.params (the BSL-declared set) is
    // populated by the script itself via app.param() calls.
    *app.stored.lock() = params.clone();
    engine
        .run_with_scope(&mut scope, script)
        .map_err(|e| ScriptError(e.to_string()))?;
    app.def.lock().name = name.to_owned();
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::db::Db;

    // i[verify param.store]
    #[test]
    fn upsert_and_load_params_round_trip() {
        let db = Db::open_in_memory().expect("open");
        upsert_param(&db, "myapp", "host", "example.com").expect("upsert");
        upsert_param(&db, "myapp", "port", "8080").expect("upsert");

        let params = load_params_for_app(&db, "myapp").expect("load");
        assert_eq!(params.get("host").map(String::as_str), Some("example.com"));
        assert_eq!(params.get("port").map(String::as_str), Some("8080"));
    }

    // i[verify param.store]
    #[test]
    fn upsert_param_replaces_existing_value() {
        let db = Db::open_in_memory().expect("open");
        upsert_param(&db, "myapp", "host", "old.example.com").expect("first upsert");
        upsert_param(&db, "myapp", "host", "new.example.com").expect("second upsert");

        let params = load_params_for_app(&db, "myapp").expect("load");
        assert_eq!(
            params.get("host").map(String::as_str),
            Some("new.example.com")
        );
        assert_eq!(params.len(), 1);
    }

    // i[verify param.store]
    #[test]
    fn load_params_scoped_to_app() {
        let db = Db::open_in_memory().expect("open");
        upsert_param(&db, "app-a", "key", "val-a").expect("upsert a");
        upsert_param(&db, "app-b", "key", "val-b").expect("upsert b");

        let params_a = load_params_for_app(&db, "app-a").expect("load a");
        let params_b = load_params_for_app(&db, "app-b").expect("load b");

        assert_eq!(params_a.get("key").map(String::as_str), Some("val-a"));
        assert_eq!(params_b.get("key").map(String::as_str), Some("val-b"));
    }

    // i[verify param.store]
    #[test]
    fn load_params_returns_empty_for_unknown_app() {
        let db = Db::open_in_memory().expect("open");
        let params = load_params_for_app(&db, "nonexistent").expect("load");
        assert!(params.is_empty());
    }

    // i[verify param.store]
    #[test]
    fn delete_app_params_removes_only_that_apps_params() {
        let db = Db::open_in_memory().expect("open");
        upsert_param(&db, "app-a", "key", "val-a").expect("upsert a");
        upsert_param(&db, "app-b", "key", "val-b").expect("upsert b");

        delete_app_params(&db, "app-a").expect("delete");

        assert!(
            load_params_for_app(&db, "app-a")
                .expect("load a")
                .is_empty()
        );
        assert_eq!(
            load_params_for_app(&db, "app-b")
                .expect("load b")
                .get("key")
                .map(String::as_str),
            Some("val-b")
        );
    }

    // i[verify param.store]
    #[test]
    fn evaluate_script_injects_params_into_stored() {
        let mut params = BTreeMap::new();
        params.insert("hostname".to_owned(), "prod.example.com".to_owned());

        let app = evaluate_script("test-app", r#"let h = app.param("hostname");"#, &params)
            .expect("script should evaluate");

        // The declared-param set tracks the name.
        assert!(
            app.def.lock().params.contains("hostname"),
            "hostname should be in declared params"
        );
        // The stored map holds the value.
        assert_eq!(
            app.stored.lock().get("hostname").map(String::as_str),
            Some("prod.example.com"),
            "stored param value should be accessible via app.stored"
        );
    }

    // i[verify param.store]
    #[test]
    fn evaluate_script_absent_param_has_no_stored_value() {
        let params = BTreeMap::new();
        let app = evaluate_script("test-app", r#"let h = app.param("hostname");"#, &params)
            .expect("script should evaluate");

        assert!(
            app.def.lock().params.contains("hostname"),
            "hostname should be recorded as declared"
        );
        assert!(
            app.stored.lock().get("hostname").is_none(),
            "absent param should have no stored value"
        );
    }

    // i[verify param.store]
    #[test]
    fn registry_load_from_db_restores_params() {
        let db = Db::open_in_memory().expect("open");

        // Register an app directly in the DB.
        db.conn
            .execute(
                "INSERT INTO registered_apps (name, script, installed) VALUES (?1, ?2, 0)",
                rusqlite::params!["myapp", r#"let h = app.param("hostname");"#],
            )
            .expect("insert app");

        // Store a param for it.
        upsert_param(&db, "myapp", "hostname", "restored.example.com").expect("upsert");

        let registry =
            AppRegistry::load_from_db(&db, Arc::new(Notify::new())).expect("load registry");
        let entry = registry.get("myapp").expect("app should be registered");

        assert!(
            entry.app.def.lock().params.contains("hostname"),
            "hostname should be in declared params after load"
        );
        assert_eq!(
            entry.app.stored.lock().get("hostname").map(String::as_str),
            Some("restored.example.com"),
            "param value should be restored into app.stored on startup"
        );
    }
}

// i[param.store]
pub fn load_params_for_app(db: &Db, app_name: &str) -> rusqlite::Result<BTreeMap<String, String>> {
    let mut stmt = db
        .conn
        .prepare("SELECT param_name, value FROM params WHERE app_name = ?1 ORDER BY param_name")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([app_name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().collect())
}

// i[param.store]
// i[param.set]
pub fn upsert_param(
    db: &Db,
    app_name: &str,
    param_name: &str,
    value: &str,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR REPLACE INTO params (app_name, param_name, value) VALUES (?1, ?2, ?3)",
        rusqlite::params![app_name, param_name, value],
    )?;
    Ok(())
}

pub fn delete_app_params(db: &Db, app_name: &str) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM params WHERE app_name = ?1", [app_name])?;
    Ok(())
}

// i[param.unset]
pub fn delete_one_param(db: &Db, app_name: &str, param_name: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM params WHERE app_name = ?1 AND param_name = ?2",
        rusqlite::params![app_name, param_name],
    )?;
    Ok(())
}
