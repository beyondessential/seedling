use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Instant,
};

use parking_lot::RwLock;
use serde_json::{Value, json};

use crate::{
    defs::install::InstallRequirementKind,
    runtime::apps::{AppRegistry, AppStatus},
};

use super::error::{ErrorCode, OiError};

/// Shared state for all OI request handlers.
pub struct OiState {
    pub registry: Arc<RwLock<AppRegistry>>,
    /// Set once by the server after key generation; never changes after that.
    pub spki_fingerprint: OnceLock<String>,
    pub start_time: Instant,
}

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

    match req.method.as_str() {
        // i[status.get]
        "GetStatus" => get_status(state),
        // i[app.list]
        "ListApps" => list_apps(state),
        // i[app.describe]
        "DescribeApp" => describe_app(state, req.params),
        other => Err(OiError::not_found(format!("unknown method: {other}"))),
    }
}

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

fn install_requirement_kind_str(kind: InstallRequirementKind) -> &'static str {
    match kind {
        InstallRequirementKind::Text => "text",
        InstallRequirementKind::Email => "email",
        InstallRequirementKind::Password => "password",
        InstallRequirementKind::WeakPassword => "weak-password",
    }
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
