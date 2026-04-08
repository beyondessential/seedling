use std::{
    collections::{BTreeMap, HashMap},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use std::collections::HashSet;

use ipnet::Ipv6Net;
use parking_lot::RwLock;
use serde_json::{Value, json};
use tokio::sync::RwLock as AsyncRwLock;

use crate::{
    defs::install::InstallRequirementKind,
    runtime::{
        apps::{AppRegistry, AppStatus},
        desired::OperationProgress,
    },
};

use super::error::{ErrorCode, OiError};

// ---------------------------------------------------------------------------
// ReconcilerFactory
// ---------------------------------------------------------------------------

/// Spawns per-app reconciler tokio tasks. Constructed in `main.rs` and stored
/// in `OiState`. `spawn_for` is called from within `block_in_place` (a sync
/// context) and uses `tokio::runtime::Handle::current().spawn()` to schedule
/// the async reconciler task.
pub struct ReconcilerFactory {
    pub system: Arc<crate::system::System>,
    pub node_prefix: Ipv6Net,
    pub db_path: PathBuf,
    pub data_dir: PathBuf,
    pub caddy_admin_addr: Arc<AsyncRwLock<SocketAddr>>,
}

impl ReconcilerFactory {
    /// Spawn a reconciler task for `app_name`. Returns the `JoinHandle` so the
    /// caller can store it in `AppEntry.reconciler_handle` for later cancellation.
    pub fn spawn_for(
        &self,
        app_name: String,
        app: crate::defs::app::App,
        active_progress: Arc<parking_lot::RwLock<Option<OperationProgress>>>,
        tick_notify: Arc<tokio::sync::Notify>,
    ) -> tokio::task::JoinHandle<()> {
        use crate::{
            runtime::{InstanceRegistry, db::Db, registry::DbInstanceRegistry},
            system::reconcile::Reconciler,
        };

        let instance_registry: Arc<dyn InstanceRegistry> = match Db::open(&self.db_path) {
            Ok(db) => Arc::new(DbInstanceRegistry::new(db)),
            Err(e) => {
                tracing::error!("cannot open instance db for app {app_name}: {e}");
                return tokio::runtime::Handle::current().spawn(async {});
            }
        };

        let obs_db = match Db::open(&self.db_path) {
            Ok(db) => db,
            Err(e) => {
                tracing::error!("cannot open obs db for app {app_name}: {e}");
                return tokio::runtime::Handle::current().spawn(async {});
            }
        };

        let mut reconciler = Reconciler::new(
            app_name,
            app,
            active_progress,
            Arc::clone(&self.system),
            self.node_prefix,
            instance_registry,
            HashMap::new(),
            Arc::clone(&self.caddy_admin_addr),
            self.data_dir.clone(),
            obs_db,
        );

        tokio::runtime::Handle::current().spawn(async move {
            reconciler.populate_bridge_names().await;
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {},
                    _ = tick_notify.notified() => {},
                }
                reconciler.tick().await;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// OiState
// ---------------------------------------------------------------------------

/// Shared state for all OI request handlers.
pub struct OiState {
    pub registry: Arc<RwLock<AppRegistry>>,
    /// Set once by the server after key generation; never changes after that.
    pub spki_fingerprint: OnceLock<String>,
    pub start_time: Instant,
    pub db: Arc<parking_lot::Mutex<crate::runtime::db::Db>>,
    pub scheduler: Arc<parking_lot::Mutex<crate::runtime::Scheduler>>,
    pub reconciler_factory: Arc<ReconcilerFactory>,
    /// In-memory set of authorized client SPKI fingerprints, shared with the
    /// TLS client cert verifier so additions/removals take effect immediately.
    pub trusted_keys: Arc<parking_lot::RwLock<HashSet<String>>>,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

type HandlerResult = Result<Value, OiError>;

/// Parse the newline-terminated JSON request from `buf`, dispatch to a handler,
/// and return the serialised JSON response (no trailing newline).
pub fn dispatch(state: &OiState, buf: &[u8]) -> Vec<u8> {
    let response = match parse_and_dispatch(state, buf) {
        Ok(result) => json!({ "result": result }),
        Err(e) => json!({
            "error": {
                "code": e.code,
                "message": e.message,
            }
        }),
    };
    serde_json::to_vec(&response).expect("response serialisation never fails")
}

fn parse_and_dispatch(state: &OiState, buf: &[u8]) -> HandlerResult {
    #[derive(serde::Deserialize)]
    struct Request {
        method: String,
        #[serde(default)]
        params: Value,
    }
    let req: Request = serde_json::from_slice(buf)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("invalid request: {e}")))?;

    let result = match req.method.as_str() {
        // i[status.get]
        "GetStatus" => get_status(state),
        // i[app.list]
        "ListApps" => list_apps(state),
        // i[app.describe]
        "DescribeApp" => describe_app(state, req.params),
        "RegisterApp" => register_app(state, req.params),
        "DeregisterApp" => deregister_app(state, req.params),
        "UpdateApp" => update_app(state, req.params),
        // i[key.list]
        "ListKeys" => list_keys(state),
        // i[key.authorize]
        "AuthorizeKey" => authorize_key(state, req.params),
        // i[key.revoke]
        "RevokeKey" => revoke_key(state, req.params),
        other => Err(OiError::not_found(format!("unknown method: {other}"))),
    };

    match &result {
        Ok(_) => tracing::info!(method = %req.method, "ok"),
        Err(e) => tracing::info!(
            method = %req.method,
            code = ?e.code,
            error = %e.message,
            "error",
        ),
    }

    result
}

// ---------------------------------------------------------------------------
// Key management handlers
// ---------------------------------------------------------------------------

// i[key.list]
fn list_keys(state: &OiState) -> HandlerResult {
    let db = state.db.lock();
    let rows = crate::oi::auth::list_keys(&db)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    let result: Vec<Value> = rows
        .into_iter()
        .map(|(fp, label, added_at)| {
            json!({ "fingerprint": fp, "label": label, "added_at": added_at })
        })
        .collect();
    Ok(json!(result))
}

// i[key.authorize]
fn authorize_key(state: &OiState, params: Value) -> HandlerResult {
    let fp = params
        .get("fingerprint")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OiError::new(ErrorCode::RequirementsInvalid, "missing param: fingerprint")
        })?;
    let label = params
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("unnamed");
    let db = state.db.lock();
    crate::oi::auth::authorize_key(&db, &state.trusted_keys, fp, label)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    tracing::info!(fingerprint = %fp, label = %label, "authorized key");
    Ok(json!({}))
}

// i[key.revoke]
fn revoke_key(state: &OiState, params: Value) -> HandlerResult {
    let fp = params
        .get("fingerprint")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::NotFound, "missing param: fingerprint"))?;
    let db = state.db.lock();
    let removed = crate::oi::auth::revoke_key(&db, &state.trusted_keys, fp)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    if removed {
        tracing::info!(fingerprint = %fp, "revoked key");
        Ok(json!({}))
    } else {
        Err(OiError::not_found(format!("key not found: {fp}")))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_name(name: &str) -> Result<(), OiError> {
    // l[bsl.name]: ^[a-zA-Z][a-zA-Z0-9-]{1,60}[a-zA-Z0-9]$
    let ok = name.len() >= 3
        && name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.ends_with(|c: char| c.is_ascii_alphanumeric())
        && name[1..name.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("invalid name '{name}': must match ^[a-zA-Z][a-zA-Z0-9-]{{1,60}}[a-zA-Z0-9]$"),
        ))
    }
}

fn install_requirement_kind_str(kind: InstallRequirementKind) -> &'static str {
    match kind {
        InstallRequirementKind::Text => "text",
        InstallRequirementKind::Email => "email",
        InstallRequirementKind::Password => "password",
        InstallRequirementKind::WeakPassword => "weak-password",
    }
}

// ---------------------------------------------------------------------------
// Phase 1 handlers
// ---------------------------------------------------------------------------

// i[status.get]
fn get_status(state: &OiState) -> HandlerResult {
    let uptime = state.start_time.elapsed().as_secs();
    let reg = state.registry.read();
    let apps = reg.list();
    let apps_total = apps.len();
    let mut apps_by_status: HashMap<&'static str, usize> = HashMap::new();
    for (_, status) in &apps {
        *apps_by_status.entry(status.name()).or_insert(0) += 1;
    }

    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": uptime,
        "spki_fingerprint": state.spki_fingerprint.get().cloned().unwrap_or_default(),
        "apps_total": apps_total,
        "apps_by_status": apps_by_status,
        "active_operations": 0,
        "active_faults": 0,
        "active_shells": 0,
        "active_forwards": 0,
    }))
}

// i[app.list]
fn list_apps(state: &OiState) -> HandlerResult {
    let reg = state.registry.read();
    let apps = reg.list();
    let result: Vec<Value> = apps
        .into_iter()
        .map(|(name, status)| {
            let mut obj = json!({ "name": name, "status": status.name() });
            if let AppStatus::Operating { action_name } = &status {
                obj["action_name"] = json!(action_name);
            }
            obj
        })
        .collect();
    Ok(json!(result))
}

// i[app.describe]
fn describe_app(state: &OiState, params: Value) -> HandlerResult {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::NotFound, "missing param: name"))?;

    let reg = state.registry.read();
    let entry = reg
        .get(name)
        .ok_or_else(|| OiError::not_found(format!("app not found: {name}")))?;

    let status = reg.status_of(name).unwrap();
    let def = entry.app.def.lock();

    // params
    let params_json: Vec<Value> = def
        .params
        .iter()
        .map(|(k, v)| json!({ "name": k, "value": v }))
        .collect();

    // actions (kind: "action")
    let mut actions_json: Vec<Value> = def
        .actions
        .values()
        .map(|a| json!({ "name": a.name, "description": a.description, "kind": "action" }))
        .collect();

    // shells (kind: "shell")
    for s in def.shells.values() {
        actions_json.push(json!({ "name": s.name, "description": s.description, "kind": "shell" }));
    }

    // install action (kind: "install")
    if def.install.is_some() {
        actions_json.push(json!({ "name": "install", "description": null, "kind": "install" }));
    }

    // install_requirements
    let install_requirements: serde_json::Map<String, Value> = def
        .install
        .as_ref()
        .map(|inst| {
            inst.requirements
                .iter()
                .map(|(k, req)| {
                    (
                        k.clone(),
                        json!({
                            "kind": install_requirement_kind_str(req.kind),
                            "required": req.required,
                            "description": req.description,
                            "default_value": req.default_value,
                        }),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    // resources — instances not yet wired (Phase 1 skeleton)
    let resources_json: Vec<Value> = def
        .resources
        .keys()
        .map(|id| {
            json!({
                "name": id.name.as_str(),
                "type": format!("{:?}", id.kind).to_lowercase(),
                "instances": [],
                "faults": [],
            })
        })
        .collect();

    let mut desc = json!({
        "status": status.name(),
        "resources": resources_json,
        "params": params_json,
        "actions": actions_json,
        "install_requirements": install_requirements,
    });

    if let AppStatus::Operating { action_name } = &status {
        desc["current_operation"] = json!({
            "action_name": action_name,
            "barrier": null,
        });
    }

    Ok(desc)
}

// ---------------------------------------------------------------------------
// Phase 2 handlers
// ---------------------------------------------------------------------------

// i[app.register]
// i[app.persist]
fn register_app(state: &OiState, params: Value) -> HandlerResult {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: name"))?;
    let script = params
        .get("script")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: script"))?;

    validate_name(name)?;

    {
        let reg = state.registry.read();
        if reg.is_registered(name) {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!("app already registered: {name}"),
            ));
        }
    }

    // Evaluate script and add to in-memory registry.
    {
        let mut reg = state.registry.write();
        reg.register(name.to_owned(), script.to_owned())
            .map_err(|e| OiError::script_error(e.to_string()))?;
    }

    // Persist to DB.
    {
        let reg = state.registry.read();
        let entry = reg.get(name).expect("just registered");
        let db = state.db.lock();
        AppRegistry::persist_app(&*db, entry)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db persist: {e}")))?;
    }

    tracing::info!(app = %name, "registered app");
    Ok(json!({}))
}

// i[app.deregister]
fn deregister_app(state: &OiState, params: Value) -> HandlerResult {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::NotFound, "missing param: name"))?;

    {
        let reg = state.registry.read();
        if !reg.is_registered(name) {
            return Err(OiError::not_found(format!("app not found: {name}")));
        }
    }

    // Reject if an operation is active or queued for this app.
    if state.scheduler.lock().has_operation_for(name) {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {name}"),
        ));
    }

    // Mark as deregistering and abort the reconciler.
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(name) {
            entry.deregistering = true;
            if let Some(handle) = entry.reconciler_handle.take() {
                handle.abort();
            }
        }
    }

    // Remove from DB.
    {
        let db = state.db.lock();
        AppRegistry::remove_app(&*db, name)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db remove: {e}")))?;
    }

    // Remove from in-memory registry.
    state.registry.write().deregister(name);

    tracing::info!(app = %name, "deregistered app");
    Ok(json!({}))
}

// i[app.update]
fn update_app(state: &OiState, params: Value) -> HandlerResult {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::NotFound, "missing param: name"))?;
    let script = params
        .get("script")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: script"))?;

    {
        let reg = state.registry.read();
        if !reg.is_registered(name) {
            return Err(OiError::not_found(format!("app not found: {name}")));
        }
    }

    let op_in_progress = state
        .registry
        .read()
        .get(name)
        .map_or(false, |e| e.active_progress.read().is_some());

    if op_in_progress {
        // Operation running: just update stored script so next evaluation uses it.
        // The in-memory AppDef is left unchanged until the operation completes.
        if let Some(entry) = state.registry.write().get_mut(name) {
            entry.script = script.to_owned();
        }
    } else {
        // No operation: reload script and apply to in-memory AppDef immediately.
        state
            .registry
            .write()
            .reload(name, script.to_owned(), &BTreeMap::new())
            .map_err(|e| OiError::script_error(e.to_string()))?;
        // Wake reconciler to pick up new desired state.
        if let Some(entry) = state.registry.read().get(name) {
            entry.tick_notify.notify_one();
        }
    }

    // Persist updated script to DB in either case.
    {
        let reg = state.registry.read();
        let entry = reg.get(name).expect("confirmed registered");
        let db = state.db.lock();
        AppRegistry::persist_app(&*db, entry)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db update: {e}")))?;
    }

    tracing::info!(app = %name, "updated app");
    Ok(json!({}))
}
