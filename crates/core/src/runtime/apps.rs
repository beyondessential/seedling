use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use jiff::Timestamp;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::{
    defs::app::App,
    runtime::{db::Db, desired::OperationProgress, generations},
    setup_language,
};

mod params;
mod registry_faults;

pub use params::{
    delete_app_params, delete_one_param, load_params_for_app, sync_script_error_fault, upsert_param,
};
pub use registry_faults::sync_registry_faults;

#[derive(Debug)]
pub struct ScriptError(pub String);

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScriptError {}

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
    pub script_error: Option<(String, Timestamp)>,
    /// Current app generation (0 if not yet bumped).
    // i[impl app.generation]
    pub current_generation: generations::Generation,
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
        limits: &crate::ScriptLimits,
    ) -> Result<(), ScriptError> {
        let (app, raw_error) = evaluate_script(&name, &script, &BTreeMap::new(), limits);
        let script_error = raw_error.map(|e| {
            tracing::warn!(app = %name, error = %e, "script has errors at registration; params may need to be set");
            (e.to_string(), Timestamp::now())
        });
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
                current_generation: 0,
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
    pub fn reload(
        &mut self,
        name: &str,
        script: String,
        params: &BTreeMap<String, String>,
        limits: &crate::ScriptLimits,
    ) {
        let (app, raw_error) = evaluate_script(name, &script, params, limits);
        if let Some(entry) = self.entries.get_mut(name) {
            entry.script = script;
            entry.app = app;
            entry.script_error = raw_error.map(|e| (e.to_string(), Timestamp::now()));
        }
    }

    pub fn get(&self, name: &str) -> Option<&AppEntry> {
        self.entries.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &AppEntry> {
        self.entries.values()
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
    pub fn load_from_db(
        db: &Db,
        tick_notify: Arc<Notify>,
        limits: &crate::ScriptLimits,
    ) -> rusqlite::Result<Self> {
        let mut registry = Self::new();
        let mut stmt = db.conn.prepare(
            "SELECT name, installed, uninstalling, current_generation FROM registered_apps ORDER BY name",
        )?;
        let rows: Vec<(String, bool, bool, i64)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;

        for (name, installed, uninstalling, current_gen) in rows {
            let phase = match (installed, uninstalling) {
                (_, true) => AppPhase::Uninstalling,
                (true, _) => AppPhase::Installed,
                (false, _) => AppPhase::NotInstalled,
            };
            if current_gen <= 0 {
                tracing::warn!(app = %name, "skipping app with no current generation");
                continue;
            }
            let current_generation = current_gen as generations::Generation;

            let hash = match generations::script_hash_at(db, &name, current_generation) {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!(app = %name, generation = current_generation, "failed to resolve script hash: {e}");
                    continue;
                }
            };
            let script: String = match generations::script_body(db, &hash) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    tracing::warn!(app = %name, hash = %hash, "missing script body");
                    continue;
                }
                Err(e) => {
                    tracing::warn!(app = %name, "failed to load script body: {e}");
                    continue;
                }
            };
            let stored = match load_params_for_app(db, &name) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("failed to load params for app '{name}': {e}");
                    BTreeMap::new()
                }
            };
            let (app, raw_error) = evaluate_script(&name, &script, &stored, limits);
            let script_error = raw_error.map(|e| {
                tracing::warn!("failed to reload script for app '{name}': {e}");
                (e.to_string(), Timestamp::now())
            });
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
                    current_generation,
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
            "INSERT OR REPLACE INTO registered_apps (name, installed, uninstalling, current_generation) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                entry.name,
                installed as i64,
                uninstalling as i64,
                entry.current_generation as i64,
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

/// Retrieve the script text active at a specific generation, along with the app
/// name (for cross-checking selection in the OI handler).
pub fn get_script_at_generation(
    db: &Db,
    app: &str,
    generation: generations::Generation,
) -> rusqlite::Result<Option<String>> {
    let hash = match generations::script_hash_at(db, app, generation) {
        Ok(h) => h,
        Err(generations::Error::NotFound { .. }) => return Ok(None),
        Err(generations::Error::Db(e)) => return Err(e),
        Err(e) => {
            tracing::warn!(app, generation, "script_hash_at failed: {e}");
            return Ok(None);
        }
    };
    generations::script_body(db, &hash)
}

/// Retrieve the script for the current generation of an app.
pub fn get_current_script(
    db: &Db,
    app: &str,
) -> rusqlite::Result<Option<(generations::Generation, String)>> {
    let Some(current_gen) = generations::current(db, app)? else {
        return Ok(None);
    };
    match get_script_at_generation(db, app, current_gen)? {
        Some(s) => Ok(Some((current_gen, s))),
        None => Ok(None),
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
    if let Err(e) = db.conn.execute(
        "UPDATE registered_apps SET installed = ?1, uninstalling = ?2 WHERE name = ?3",
        rusqlite::params![installed as i64, uninstalling as i64, app_name],
    ) {
        tracing::error!(app = %app_name, "failed to persist phase transition: {e}");
    }
}

pub fn evaluate_script(
    name: &str,
    script: &str,
    params: &BTreeMap<String, String>,
    limits: &crate::ScriptLimits,
) -> (App, Option<ScriptError>) {
    let (engine, mut scope, app) = setup_language(limits);
    // i[param.store] — pre-populate stored values so is_set()/value() work
    // during script evaluation. AppDef.params (the BSL-declared set) is
    // populated by the script itself via app.param() calls.
    *app.stored.lock() = params.clone();
    app.def.lock().name = name.to_owned();
    crate::defs::app::set_appdef_holder(&app.def);
    let err = engine
        .run_with_scope(&mut scope, script)
        .err()
        .map(|e| ScriptError(e.to_string()));
    crate::defs::app::clear_appdef_holder();
    (app, err)
}

#[cfg(test)]
mod tests;
