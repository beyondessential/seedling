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
        scheduler::{RejectReason, ScheduleResult},
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
pub fn dispatch(state: &Arc<OiState>, buf: &[u8]) -> Vec<u8> {
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

fn parse_and_dispatch(state: &Arc<OiState>, buf: &[u8]) -> HandlerResult {
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
        // i[param.set]
        "SetParam" => set_param(state, req.params),
        // i[param.unset]
        "UnsetParam" => unset_param(state, req.params),
        // i[action.invoke]
        "InvokeAction" => invoke_action(state, req.params),
        // i[action.invoke.install]
        "InvokeInstall" => invoke_install(state, req.params),
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

    // Load stored param values from DB. Names come from AppDef; values come
    // from the params table. Params declared by the script but never set by
    // the operator are shown as null, not as the internal <placeholder> string.
    let stored_params = {
        let db = state.db.lock();
        crate::runtime::apps::load_params_for_app(&db, name).unwrap_or_default()
    };

    // i[app.describe] — app-level faults (e.g. script evaluation errors).
    let app_faults_json: Vec<Value> = entry
        .script_error
        .as_ref()
        .map(|(msg, ts)| {
            vec![json!({
                "id": "script_error",
                "app": name,
                "resource_type": null,
                "resource_name": null,
                "instance_id": null,
                "kind": "script_error",
                "timestamp": ts.to_rfc3339(),
                "description": msg,
            })]
        })
        .unwrap_or_default();

    let def = entry.app.def.lock();

    // i[app.describe]
    let params_json: Vec<Value> = def
        .params
        .iter()
        .map(|k| {
            let value = stored_params
                .get(k)
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null);
            json!({ "name": k, "value": value })
        })
        .collect();

    // i[app.describe] — params stored in the DB that the current script does
    // not reference; shown for operator awareness only.
    let unknown_params_json: Vec<Value> = stored_params
        .iter()
        .filter(|(k, _)| !def.params.contains(*k))
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
        "faults": app_faults_json,
        "resources": resources_json,
        "params": params_json,
        "unknown_params": unknown_params_json,
        "actions": actions_json,
        "install_requirements": install_requirements,
    });

    if let AppStatus::Operating { .. } = &status {
        let action_name = state
            .scheduler
            .lock()
            .active()
            .filter(|a| a.app == name)
            .map(|a| a.action.clone())
            .unwrap_or_default();
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
        AppRegistry::persist_app(&db, entry)
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
        AppRegistry::remove_app(&db, name)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db remove: {e}")))?;
    }
    {
        let db = state.db.lock();
        crate::runtime::apps::delete_app_params(&db, name)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db param cleanup: {e}")))?;
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
        .is_some_and(|e| e.active_progress.read().is_some());

    if op_in_progress {
        // Operation running: just update stored script so next evaluation uses it.
        // The in-memory AppDef is left unchanged until the operation completes.
        if let Some(entry) = state.registry.write().get_mut(name) {
            entry.script = script.to_owned();
        }
    } else {
        // No operation: reload script and apply to in-memory AppDef immediately.
        let loaded_params = {
            let db = state.db.lock();
            crate::runtime::apps::load_params_for_app(&db, name)
                .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db params: {e}")))?
        };
        state
            .registry
            .write()
            .reload(name, script.to_owned(), &loaded_params);
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
        AppRegistry::persist_app(&db, entry)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db update: {e}")))?;
    }

    tracing::info!(app = %name, "updated app");
    Ok(json!({}))
}

// ---------------------------------------------------------------------------
// Phase 3 handlers
// ---------------------------------------------------------------------------

// i[param.store]
// i[param.set]
// i[param.unknown]
fn set_param(state: &OiState, params: Value) -> HandlerResult {
    let app = params
        .get("app")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: app"))?;
    let param_name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: name"))?;
    let value = params
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: value"))?;

    {
        let reg = state.registry.read();
        if !reg.is_registered(app) {
            return Err(OiError::not_found(format!("app not found: {app}")));
        }
    }

    if let Some(AppStatus::Deregistering) = state.registry.read().status_of(app) {
        return Err(OiError::new(
            ErrorCode::Deregistering,
            format!("app is deregistering: {app}"),
        ));
    }

    {
        let db = state.db.lock();
        crate::runtime::apps::upsert_param(&db, app, param_name, value)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    }

    let script = {
        let reg = state.registry.read();
        reg.get(app).expect("confirmed registered").script.clone()
    };
    let loaded_params = {
        let db = state.db.lock();
        crate::runtime::apps::load_params_for_app(&db, app)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?
    };
    state.registry.write().reload(app, script, &loaded_params);

    let (has_on_change, is_installed, tick_notify) = {
        let reg = state.registry.read();
        let entry = reg.get(app).expect("confirmed registered");
        let has = entry.app.def.lock().param_changes.contains(param_name);
        let installed = entry.installed;
        let notify = Arc::clone(&entry.tick_notify);
        (has, installed, notify)
    };

    // Only schedule the on_change handler when the app is installed — there is
    // nothing running to respond to the change before that. The stored value
    // takes effect automatically when the app is next evaluated (e.g. at install).
    if has_on_change && is_installed {
        let result = state.scheduler.lock().request(app, param_name, None);
        match result {
            ScheduleResult::Accepted => {
                tracing::info!(app = %app, param = %param_name, schedule = "accepted", "set_param");
                tick_notify.notify_one();
                Ok(json!({ "schedule": "accepted" }))
            }
            ScheduleResult::Queued => {
                tracing::info!(app = %app, param = %param_name, schedule = "queued", "set_param");
                tick_notify.notify_one();
                Ok(json!({ "schedule": "queued" }))
            }
            ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress) => {
                tracing::info!(app = %app, param = %param_name, schedule = "rejected_in_progress", "set_param");
                Err(OiError::new(
                    ErrorCode::OperationInProgress,
                    format!("operation in progress for app: {app}"),
                ))
            }
            ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued) => {
                tracing::info!(app = %app, param = %param_name, schedule = "rejected_queued", "set_param");
                Err(OiError::new(
                    ErrorCode::AlreadyQueued,
                    format!("already queued for app: {app}"),
                ))
            }
        }
    } else {
        Ok(json!({ "schedule": "accepted" }))
    }
}

// i[param.unset]
fn unset_param(state: &OiState, params: Value) -> HandlerResult {
    let app = params
        .get("app")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: app"))?;
    let param_name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: name"))?;

    {
        let reg = state.registry.read();
        if !reg.is_registered(app) {
            return Err(OiError::not_found(format!("app not found: {app}")));
        }
    }

    if let Some(AppStatus::Deregistering) = state.registry.read().status_of(app) {
        return Err(OiError::new(
            ErrorCode::Deregistering,
            format!("app is deregistering: {app}"),
        ));
    }

    {
        let db = state.db.lock();
        crate::runtime::apps::delete_one_param(&db, app, param_name)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    }

    let script = {
        let reg = state.registry.read();
        reg.get(app).expect("confirmed registered").script.clone()
    };
    let loaded_params = {
        let db = state.db.lock();
        crate::runtime::apps::load_params_for_app(&db, app)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?
    };
    state.registry.write().reload(app, script, &loaded_params);

    let (has_on_change, is_installed, tick_notify) = {
        let reg = state.registry.read();
        let entry = reg.get(app).expect("confirmed registered");
        let has = entry.app.def.lock().param_changes.contains(param_name);
        let installed = entry.installed;
        let notify = Arc::clone(&entry.tick_notify);
        (has, installed, notify)
    };

    if has_on_change && is_installed {
        let result = state.scheduler.lock().request(app, param_name, None);
        match result {
            ScheduleResult::Accepted => {
                tracing::info!(app = %app, param = %param_name, schedule = "accepted", "unset_param");
                tick_notify.notify_one();
                Ok(json!({ "schedule": "accepted" }))
            }
            ScheduleResult::Queued => {
                tracing::info!(app = %app, param = %param_name, schedule = "queued", "unset_param");
                tick_notify.notify_one();
                Ok(json!({ "schedule": "queued" }))
            }
            ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress) => {
                Err(OiError::new(
                    ErrorCode::OperationInProgress,
                    format!("operation in progress for app: {app}"),
                ))
            }
            ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued) => Err(OiError::new(
                ErrorCode::AlreadyQueued,
                format!("already queued for app: {app}"),
            )),
        }
    } else {
        tracing::info!(app = %app, param = %param_name, "unset_param");
        Ok(json!({ "schedule": "accepted" }))
    }
}

// ---------------------------------------------------------------------------
// Phase 4 helpers
// ---------------------------------------------------------------------------

// i[action.invoke.install.validation]
fn is_valid_email(email: &str) -> bool {
    let mut parts = email.splitn(2, '@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

// i[action.invoke.install.validation]
fn is_strong_password(password: &str) -> bool {
    zxcvbn::zxcvbn(password, &[])
        .map(|e| e.score() >= 3)
        .unwrap_or(false)
}

// i[action.invoke.install.validation]
fn validate_requirements(
    install_def: Option<&crate::defs::install::InstallDef>,
    submitted: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OiError> {
    let install_def = match install_def {
        Some(d) => d,
        None => {
            return if submitted.is_empty() {
                Ok(BTreeMap::new())
            } else {
                Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    "app has no install requirements",
                ))
            };
        }
    };

    let mut filled = submitted.clone();
    let mut errors: Vec<String> = Vec::new();

    for (field, req_def) in &install_def.requirements {
        let raw = filled.get(field).map(|s| s.as_str()).unwrap_or("");

        if raw.is_empty() {
            if let Some(default) = &req_def.default_value {
                filled.insert(field.clone(), default.clone());
            } else if req_def.required {
                errors.push(format!("{field}: required field is missing"));
                continue;
            } else {
                continue;
            }
        }

        let value = filled.get(field).map(|s| s.as_str()).unwrap_or("");
        match req_def.kind {
            InstallRequirementKind::Email => {
                if !is_valid_email(value) {
                    errors.push(format!("{field}: invalid email address"));
                }
            }
            InstallRequirementKind::Password => {
                if !is_strong_password(value) {
                    errors.push(format!("{field}: password is too weak"));
                }
            }
            InstallRequirementKind::Text | InstallRequirementKind::WeakPassword => {}
        }
    }

    if !errors.is_empty() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            errors.join("; "),
        ));
    }

    Ok(filled)
}

// i[action.invoke.install.validation]
fn validate_install_requirements(
    state: &OiState,
    app_name: &str,
    submitted: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OiError> {
    let reg = state.registry.read();
    let entry = reg.get(app_name).expect("caller confirmed exists");
    let def = entry.app.def.lock();
    validate_requirements(def.install.as_ref(), submitted)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::defs::install::{InstallDef, InstallRequirementDef, InstallRequirementKind};

    use super::{is_strong_password, is_valid_email, validate_requirements};

    // i[verify action.invoke.install.validation]
    #[test]
    fn valid_email_basic() {
        assert!(is_valid_email("user@example.com"));
        assert!(is_valid_email("a@b.co"));
        assert!(is_valid_email("user+tag@sub.example.org"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_no_at() {
        assert!(!is_valid_email("notanemail"));
        assert!(!is_valid_email(""));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_no_dot_in_domain() {
        assert!(!is_valid_email("user@nodot"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_empty_local() {
        assert!(!is_valid_email("@example.com"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_domain_starts_with_dot() {
        assert!(!is_valid_email("user@.example.com"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_domain_ends_with_dot() {
        assert!(!is_valid_email("user@example.com."));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn strong_password_accepted() {
        assert!(is_strong_password("correct-horse-battery-staple-42!"));
        assert!(is_strong_password("Tr0ub4dor&3xtraL0ng"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn weak_password_rejected() {
        assert!(!is_strong_password("password"));
        assert!(!is_strong_password("123456"));
        assert!(!is_strong_password("abc"));
    }

    fn req(
        kind: InstallRequirementKind,
        required: bool,
        default: Option<&str>,
    ) -> InstallRequirementDef {
        InstallRequirementDef {
            kind,
            required,
            default_value: default.map(|s| s.to_owned()),
            description: None,
        }
    }

    fn install_def(fields: &[(&str, InstallRequirementDef)]) -> InstallDef {
        InstallDef {
            requirements: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn no_install_def_empty_requirements_ok() {
        let result = validate_requirements(None, &BTreeMap::new());
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn no_install_def_nonempty_requirements_rejected() {
        let mut submitted = BTreeMap::new();
        submitted.insert("key".to_owned(), "value".to_owned());
        let result = validate_requirements(None, &submitted);
        assert!(result.is_err());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn required_field_missing_returns_error() {
        let def = install_def(&[("email", req(InstallRequirementKind::Text, true, None))]);
        let result = validate_requirements(Some(&def), &BTreeMap::new());
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("email"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn required_field_with_default_filled_in() {
        let def = install_def(&[(
            "site",
            req(InstallRequirementKind::Text, true, Some("default-site")),
        )]);
        let result = validate_requirements(Some(&def), &BTreeMap::new());
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap().get("site").map(String::as_str),
            Some("default-site")
        );
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn optional_field_absent_is_ok() {
        let def = install_def(&[("note", req(InstallRequirementKind::Text, false, None))]);
        let result = validate_requirements(Some(&def), &BTreeMap::new());
        assert!(result.is_ok());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_field_returns_error() {
        let def = install_def(&[("email", req(InstallRequirementKind::Email, true, None))]);
        let mut submitted = BTreeMap::new();
        submitted.insert("email".to_owned(), "notanemail".to_owned());
        let result = validate_requirements(Some(&def), &submitted);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("email"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn valid_email_field_passes() {
        let def = install_def(&[("email", req(InstallRequirementKind::Email, true, None))]);
        let mut submitted = BTreeMap::new();
        submitted.insert("email".to_owned(), "user@example.com".to_owned());
        let result = validate_requirements(Some(&def), &submitted);
        assert!(result.is_ok());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn weak_password_field_returns_error() {
        let def = install_def(&[("pw", req(InstallRequirementKind::Password, true, None))]);
        let mut submitted = BTreeMap::new();
        submitted.insert("pw".to_owned(), "password".to_owned());
        let result = validate_requirements(Some(&def), &submitted);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("pw"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn strong_password_field_passes() {
        let def = install_def(&[("pw", req(InstallRequirementKind::Password, true, None))]);
        let mut submitted = BTreeMap::new();
        submitted.insert(
            "pw".to_owned(),
            "correct-horse-battery-staple-42!".to_owned(),
        );
        let result = validate_requirements(Some(&def), &submitted);
        assert!(result.is_ok());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn weak_password_kind_always_passes() {
        let def = install_def(&[("pw", req(InstallRequirementKind::WeakPassword, true, None))]);
        let mut submitted = BTreeMap::new();
        submitted.insert("pw".to_owned(), "password".to_owned());
        let result = validate_requirements(Some(&def), &submitted);
        assert!(result.is_ok());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn multiple_errors_collected() {
        let def = install_def(&[
            ("email", req(InstallRequirementKind::Email, true, None)),
            ("name", req(InstallRequirementKind::Text, true, None)),
        ]);
        let result = validate_requirements(Some(&def), &BTreeMap::new());
        assert!(result.is_err());
        let msg = result.unwrap_err().message;
        assert!(msg.contains("email") || msg.contains("name"));
    }
}

// ---------------------------------------------------------------------------
// Phase 4 handlers
// ---------------------------------------------------------------------------

// i[action.not-installed-gate]
// i[action.invoke]
fn invoke_action(state: &Arc<OiState>, params: Value) -> HandlerResult {
    let app_name = params
        .get("app")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: app"))?;
    let action_name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: name"))?;

    {
        let reg = state.registry.read();
        let entry = reg
            .get(app_name)
            .ok_or_else(|| OiError::not_found(format!("app not found: {app_name}")))?;

        // i[action.not-installed-gate]
        if !entry.installed {
            return Err(OiError::new(
                ErrorCode::NotInstalled,
                format!("app is not installed: {app_name}"),
            ));
        }

        let def = entry.app.def.lock();
        if def.shells.contains_key(action_name) {
            return Err(OiError::not_found(format!(
                "'{action_name}' is a shell action; use OpenShell"
            )));
        }
        if !def.actions.contains_key(action_name) {
            return Err(OiError::not_found(format!(
                "action not found: {action_name}"
            )));
        }
    }

    let (result, op_id_opt) = {
        let mut sched = state.scheduler.lock();
        let result = sched.request(app_name, action_name, None);
        let op_id = if matches!(result, ScheduleResult::Accepted) {
            sched.active().map(|a| a.operation_id.clone())
        } else {
            None
        };
        (result, op_id)
    };

    match result {
        ScheduleResult::Accepted => {
            if let Some(op_id) = op_id_opt {
                spawn_accepted_operation(
                    Arc::clone(state),
                    app_name.to_owned(),
                    action_name.to_owned(),
                    op_id,
                    None,
                );
            }
            tracing::info!(app = %app_name, action = %action_name, schedule = "accepted", "invoke_action");
            Ok(json!({ "schedule": "accepted" }))
        }
        ScheduleResult::Queued => {
            tracing::info!(app = %app_name, action = %action_name, schedule = "queued", "invoke_action");
            Ok(json!({ "schedule": "queued" }))
        }
        ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress) => Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {app_name}"),
        )),
        ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued) => Err(OiError::new(
            ErrorCode::AlreadyQueued,
            format!("already queued for app: {app_name}"),
        )),
    }
}

// i[action.not-installed-gate]
// i[action.invoke.install]
// i[action.invoke.install.validation]
fn invoke_install(state: &Arc<OiState>, params: Value) -> HandlerResult {
    let app_name = params
        .get("app")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: app"))?;

    let submitted: BTreeMap<String, String> = match params.get("requirements") {
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned()))
            .collect(),
        None | Some(Value::Null) => BTreeMap::new(),
        _ => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                "requirements must be an object",
            ));
        }
    };

    let has_install_action = {
        let reg = state.registry.read();
        let entry = reg
            .get(app_name)
            .ok_or_else(|| OiError::not_found(format!("app not found: {app_name}")))?;

        // i[action.invoke.install] - reject if already installed
        if entry.installed {
            return Err(OiError::new(
                ErrorCode::AlreadyInstalled,
                format!("app is already installed: {app_name}"),
            ));
        }
        if entry.deregistering {
            return Err(OiError::new(
                ErrorCode::Deregistering,
                format!("app is deregistering: {app_name}"),
            ));
        }

        entry.app.def.lock().install.is_some()
    };

    let filled = validate_install_requirements(state, app_name, &submitted)?;

    // If there is no on_install closure: mark installed immediately and start the reconciler.
    // The reconciler will run the start action on its first tick.
    if !has_install_action {
        {
            let mut reg = state.registry.write();
            if let Some(entry) = reg.get_mut(app_name) {
                entry.installed = true;
                let db = state.db.lock();
                AppRegistry::persist_app(&db, entry)
                    .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db persist: {e}")))?;
            }
        }
        let (app_val, ap, tn) = {
            let reg = state.registry.read();
            let entry = reg.get(app_name).expect("confirmed registered");
            (
                entry.app.clone(),
                Arc::clone(&entry.active_progress),
                Arc::clone(&entry.tick_notify),
            )
        };
        let handle = state
            .reconciler_factory
            .spawn_for(app_name.to_owned(), app_val, ap, tn);
        if let Some(entry) = state.registry.write().get_mut(app_name) {
            entry.reconciler_handle = Some(handle);
        }
        tracing::info!(app = %app_name, schedule = "accepted", "invoke_install (immediate)");
        return Ok(json!({ "schedule": "accepted" }));
    }

    let install_reqs = if filled.is_empty() {
        None
    } else {
        Some(filled)
    };

    let (result, op_id_opt) = {
        let mut sched = state.scheduler.lock();
        let result = sched.request(app_name, "install", install_reqs.clone());
        let op_id = if matches!(result, ScheduleResult::Accepted) {
            sched.active().map(|a| a.operation_id.clone())
        } else {
            None
        };
        (result, op_id)
    };

    match result {
        ScheduleResult::Accepted => {
            if let Some(op_id) = op_id_opt {
                spawn_accepted_operation(
                    Arc::clone(state),
                    app_name.to_owned(),
                    "install".to_owned(),
                    op_id,
                    install_reqs,
                );
            }
            tracing::info!(app = %app_name, schedule = "accepted", "invoke_install");
            Ok(json!({ "schedule": "accepted" }))
        }
        ScheduleResult::Queued => {
            tracing::info!(app = %app_name, schedule = "queued", "invoke_install");
            Ok(json!({ "schedule": "queued" }))
        }
        ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress) => Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {app_name}"),
        )),
        ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued) => Err(OiError::new(
            ErrorCode::AlreadyQueued,
            format!("already queued for app: {app_name}"),
        )),
    }
}

/// Spawn an async task that runs a lifecycle operation to completion, then
/// handles queued follow-on operations and install completion bookkeeping.
fn spawn_accepted_operation(
    state: Arc<OiState>,
    app_name: String,
    action_name: String,
    operation_id: crate::runtime::barrier::OperationId,
    install_requirements: Option<BTreeMap<String, String>>,
) {
    use crate::runtime::{
        AppRegistry, InstanceRegistry,
        barrier::oracle::DbWorldOracle,
        barrier::replay::{DbActionLog, OperationContext, OperationResult, run_operation},
        registry::DbInstanceRegistry,
    };

    let (app, active_progress, tick_notify, script) = {
        let reg = state.registry.read();
        match reg.get(&app_name) {
            Some(e) => (
                e.app.clone(),
                Arc::clone(&e.active_progress),
                Arc::clone(&e.tick_notify),
                e.script.clone(),
            ),
            None => {
                tracing::error!(app = %app_name, "spawn_accepted_operation: app not found");
                return;
            }
        }
    };
    let db_path = state.reconciler_factory.db_path.clone();
    let is_install = action_name == "install";

    tokio::spawn(async move {
        let app_name_bl = app_name.clone();
        let action_name_bl = action_name.clone();
        let active_progress_bl = Arc::clone(&active_progress);
        let tick_notify_bl = Arc::clone(&tick_notify);

        let success = tokio::task::spawn_blocking(move || {
            let (engine, mut scope, _) = crate::setup_language();
            let ast = match engine.compile(&script) {
                Ok(a) => a,
                Err(e) => {
                    tracing::error!(
                        app = %app_name_bl, action = %action_name_bl,
                        "script compile error: {e}"
                    );
                    return false;
                }
            };

            let action_log_db = match crate::runtime::db::Db::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!(app = %app_name_bl, "open action-log db: {e}");
                    return false;
                }
            };
            let world_db = match crate::runtime::db::Db::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!(app = %app_name_bl, "open world-oracle db: {e}");
                    return false;
                }
            };
            let instance_db = match crate::runtime::db::Db::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!(app = %app_name_bl, "open instance-registry db: {e}");
                    return false;
                }
            };

            let log = DbActionLog::new(
                action_log_db,
                operation_id.clone(),
                app_name_bl.clone(),
                action_name_bl.clone(),
            );
            let world = Arc::new(DbWorldOracle::new(world_db));
            let registry: Arc<dyn InstanceRegistry> =
                Arc::new(DbInstanceRegistry::new(instance_db));

            loop {
                let result = run_operation(
                    OperationContext {
                        engine: &engine,
                        script_ast: &ast,
                        operation_id: operation_id.clone(),
                        app: &app,
                        action_name: &action_name_bl,
                        log: &log,
                        world: Arc::clone(&world),
                        registry: Arc::clone(&registry),
                        active_progress: Some(Arc::clone(&active_progress_bl)),
                        tick_notify: Some(Arc::clone(&tick_notify_bl)),
                        install_requirements: install_requirements.clone(),
                    },
                    &mut scope,
                );
                match result {
                    OperationResult::Completed => return true,
                    OperationResult::Failed(e) => {
                        tracing::error!(
                            app = %app_name_bl, action = %action_name_bl,
                            "operation failed: {e}"
                        );
                        return false;
                    }
                    OperationResult::Suspended(_) => {
                        tick_notify_bl.notify_one();
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        })
        .await
        .unwrap_or(false);

        // Clear active progress and wake the reconciler.
        *active_progress.write() = None;
        tick_notify.notify_one();

        // i[action.invoke.install.completion]
        if is_install && success {
            let reconciler_info = {
                let mut reg = state.registry.write();
                reg.get_mut(&app_name).map(|entry| {
                    entry.installed = true;
                    let db = state.db.lock();
                    if let Err(e) = AppRegistry::persist_app(&db, entry) {
                        tracing::error!(app = %app_name, "persist installed flag: {e}");
                    }
                    (
                        entry.app.clone(),
                        Arc::clone(&entry.active_progress),
                        Arc::clone(&entry.tick_notify),
                    )
                })
            };
            if let Some((app_val, ap, tn)) = reconciler_info {
                let handle = state
                    .reconciler_factory
                    .spawn_for(app_name.clone(), app_val, ap, tn);
                if let Some(entry) = state.registry.write().get_mut(&app_name) {
                    entry.reconciler_handle = Some(handle);
                }
            }
            tracing::info!(app = %app_name, "install completed; app is now installed");
        }

        // Start the next queued operation, if any.
        let next = state.scheduler.lock().complete_current();
        if let Some(queued) = next {
            spawn_accepted_operation(
                Arc::clone(&state),
                queued.app,
                queued.action,
                queued.operation_id,
                queued.install_requirements,
            );
        }
    });
}
