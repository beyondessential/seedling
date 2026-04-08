use std::{collections::HashMap, sync::Arc};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

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

// i[app.status]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStatus {
    NotInstalled,
    Deregistering,
    Operating { action_name: String },
    Running,
    Degraded,
    Faulted,
}

impl AppStatus {
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Deregistering => "deregistering",
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
    pub installed: bool,
    pub deregistering: bool,
    /// Shared with the reconciler and operation runner to track in-progress ops.
    pub active_progress: Arc<RwLock<Option<OperationProgress>>>,
    /// Wakes the reconciler for an immediate tick.
    pub tick_notify: Arc<tokio::sync::Notify>,
    pub reconciler_handle: Option<tokio::task::JoinHandle<()>>,
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

    pub fn is_registered(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    // i[app.register]
    pub fn register(&mut self, name: String, script: String) -> Result<(), ScriptError> {
        let app = evaluate_script(&name, &script)?;
        self.entries.insert(
            name.clone(),
            AppEntry {
                name,
                script,
                app,
                installed: false,
                deregistering: false,
                active_progress: Arc::new(RwLock::new(None)),
                tick_notify: Arc::new(tokio::sync::Notify::new()),
                reconciler_handle: None,
            },
        );
        Ok(())
    }

    pub fn deregister(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    pub fn reload(&mut self, name: &str, script: String) -> Result<(), ScriptError> {
        let app = evaluate_script(name, &script)?;
        if let Some(entry) = self.entries.get_mut(name) {
            entry.script = script;
            entry.app = app;
        }
        Ok(())
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
    pub fn load_from_db(db: &Db) -> rusqlite::Result<Self> {
        let mut registry = Self::new();
        let mut stmt = db
            .conn
            .prepare("SELECT name, script, installed FROM registered_apps ORDER BY name")?;
        let rows: Vec<(String, String, bool)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;

        for (name, script, installed) in rows {
            let app = match evaluate_script(&name, &script) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("warning: failed to reload script for app '{name}': {e}");
                    App::default()
                }
            };
            registry.entries.insert(
                name.clone(),
                AppEntry {
                    name,
                    script,
                    app,
                    installed,
                    deregistering: false,
                    active_progress: Arc::new(RwLock::new(None)),
                    tick_notify: Arc::new(tokio::sync::Notify::new()),
                    reconciler_handle: None,
                },
            );
        }

        Ok(registry)
    }

    // i[app.persist]
    pub fn persist_app(db: &Db, entry: &AppEntry) -> rusqlite::Result<()> {
        db.conn.execute(
            "INSERT OR REPLACE INTO registered_apps (name, script, installed) \
             VALUES (?1, ?2, ?3)",
            rusqlite::params![entry.name, entry.script, entry.installed as i64],
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
    if entry.deregistering {
        return AppStatus::Deregistering;
    }
    if !entry.installed {
        AppStatus::NotInstalled
    } else if entry.active_progress.read().is_some() {
        AppStatus::Operating {
            action_name: String::new(),
        }
    } else {
        AppStatus::Running
    }
}

fn evaluate_script(name: &str, script: &str) -> Result<App, ScriptError> {
    let (engine, mut scope, app) = setup_language();
    engine
        .run_with_scope(&mut scope, script)
        .map_err(|e| ScriptError(e.to_string()))?;
    app.def.lock().name = name.to_owned();
    Ok(app)
}
