use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    defs::{
        install::InstallRequirementKind,
        resource::{Resource, ResourceKind},
    },
    oi::{
        error::{ErrorCode, OiError},
        state::OiState,
    },
    runtime::{
        AppPhase,
        apps::{AppEntry, AppRegistry, AppStatus},
        barrier::oracle::{derive_lifecycle_state, derive_state_with_transition_time},
        faults,
        history::{find_instances_for_group, query_observations},
        lifecycle::LifecycleState,
        scaling, transition_phase,
    },
};

use super::HandlerResult;

#[derive(Deserialize)]
pub(crate) struct AppParams {
    pub app: String,
}

#[derive(Deserialize)]
pub(crate) struct AppScriptParams {
    pub app: String,
    pub script: String,
}

#[derive(Deserialize)]
pub(crate) struct GetScriptParams {
    pub app: String,
    pub version: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ScaleParams {
    pub app: String,
    pub deployment: String,
    pub direction: String,
}

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

// i[app.list]
pub(crate) fn list_apps(state: &OiState) -> HandlerResult {
    let reg = state.registry.read();
    let apps = reg.list();
    let db = state.db.lock();
    let result: Vec<Value> = apps
        .into_iter()
        .map(|(name, base_status)| {
            let status = match reg.get(&name) {
                Some(entry) => effective_app_status(base_status, entry, &db),
                None => base_status,
            };
            let mut obj = json!({ "name": name, "status": status.name() });
            if let AppStatus::Operating { action_name } = &status {
                obj["action_name"] = json!(action_name);
            }
            obj
        })
        .collect();
    Ok(json!(result))
}

/// Refines a base `AppStatus::Running` into `Running` or `Degraded` by
/// checking whether all resource instances have reached `Ready`.  All other
/// statuses are returned unchanged.
pub(crate) fn effective_app_status(
    base: AppStatus,
    entry: &AppEntry,
    db: &crate::runtime::db::Db,
) -> AppStatus {
    if !matches!(base, AppStatus::Running) {
        return base;
    }

    let app_name = &entry.name;
    let def = entry.app.def.lock();

    let has_faults = faults::has_active_faults(db, app_name).unwrap_or(false);

    let all_ready = def.resources.keys().all(|id| {
        let instances = find_instances_for_group(db, app_name, id.kind, Some(id.name.as_str()))
            .unwrap_or_default();
        if instances.is_empty() {
            return false;
        }
        instances.iter().all(|inst| {
            let obs = query_observations(db, inst).unwrap_or_default();
            matches!(derive_lifecycle_state(inst, &obs), LifecycleState::Ready)
        })
    });

    if has_faults {
        AppStatus::Faulted
    } else if all_ready {
        AppStatus::Running
    } else {
        AppStatus::Degraded
    }
}

// i[app.describe]
pub(crate) fn describe_app(state: &OiState, params: AppParams) -> HandlerResult {
    let name = params.app.as_str();

    let reg = state.registry.read();
    let entry = reg
        .get(name)
        .ok_or_else(|| OiError::not_found(format!("app not found: {name}")))?;
    let version_id = entry.version_id.clone();

    let base_status = reg.status_of(name).unwrap();
    let status = effective_app_status(base_status, entry, &state.db.lock());

    // Load stored param values from DB. Names come from AppDef; values come
    // from the params table. Params declared by the script but never set by
    // the operator are shown as null, not as the internal <placeholder> string.
    let stored_params = {
        let db = state.db.lock();
        crate::runtime::apps::load_params_for_app(&db, name).unwrap_or_default()
    };

    // Fetch all active faults for this app once, then split by level.
    let all_faults_for_app = {
        let db = state.db.lock();
        faults::list_active_faults(&db, Some(name)).unwrap_or_default()
    };

    // i[app.describe] — app-level faults from the DB.
    let app_faults_json: Vec<Value> = all_faults_for_app
        .iter()
        .filter(|f| f.resource_type.is_none())
        .map(|f| {
            json!({
                "id": f.id,
                "app": f.app,
                "resource_type": f.resource_type,
                "resource_name": f.resource_name,
                "instance_id": f.instance_id,
                "kind": f.kind,
                "timestamp": f.timestamp.to_string(),
                "description": f.description,
            })
        })
        .collect();

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

    // resources — with instance lifecycle state from DB observations.
    // Only query instances for Installed/Uninstalling apps; NotInstalled
    // apps have no live instances and stale DB records are misleading.
    let query_instances = matches!(
        status,
        AppStatus::Running
            | AppStatus::Degraded
            | AppStatus::Faulted
            | AppStatus::Operating { .. }
            | AppStatus::Uninstalling
    );
    let resources_json: Vec<Value> = {
        let db = state.db.lock();
        def.resources
            .keys()
            .map(|id| {
                let instances_json: Vec<Value> = if query_instances {
                    find_instances_for_group(&db, name, id.kind, Some(id.name.as_str()))
                        .unwrap_or_default()
                        .iter()
                        .map(|inst| {
                            let observations = query_observations(&db, inst).unwrap_or_default();
                            let (lifecycle, transition_time) =
                                derive_state_with_transition_time(inst, &observations);
                            json!({
                                "id": inst.id.to_hex(),
                                "display_name": inst.display_name,
                                "lifecycle": format!("{lifecycle:?}"),
                                "transition_time": transition_time.and_then(|t| {
                                    jiff::Timestamp::try_from(t).ok().map(|ts| ts.to_string())
                                }),
                            })
                        })
                        .collect()
                } else {
                    vec![]
                };

                let resource_type_str = format!("{:?}", id.kind).to_lowercase();
                let resource_faults: Vec<Value> = all_faults_for_app
                    .iter()
                    .filter(|f| {
                        f.resource_type.as_deref() == Some(&resource_type_str)
                            && f.resource_name.as_deref() == Some(id.name.as_str())
                    })
                    .map(|f| {
                        json!({
                            "id": f.id,
                            "app": f.app,
                            "resource_type": f.resource_type,
                            "resource_name": f.resource_name,
                            "instance_id": f.instance_id,
                            "kind": f.kind,
                            "timestamp": f.timestamp.to_string(),
                            "description": f.description,
                        })
                    })
                    .collect();

                let mut resource_obj = json!({
                    "name": id.name.as_str(),
                    "type": resource_type_str,
                    "instances": instances_json,
                    "faults": resource_faults,
                });

                // i[impl scale.describe]
                if id.kind == ResourceKind::Deployment
                    && let Some(Resource::Deployment(deployment)) = def.resources.get(id)
                {
                    let dep_def = deployment.def.lock();
                    let low = dep_def.scale.start;
                    let high = dep_def.scale.end;
                    let current = scaling::effective_scale(&db, name, id.name.as_str(), low, high)
                        .unwrap_or(low);
                    resource_obj["scale"] = json!({
                        "low": low,
                        "high": high,
                        "current": current,
                    });
                }

                resource_obj
            })
            .collect()
    };

    let mut desc = json!({
        "status": status.name(),
        "version_id": version_id,
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

// i[app.script]
pub(crate) fn get_app_script(state: &OiState, params: GetScriptParams) -> HandlerResult {
    let name = params.app.as_str();

    {
        let reg = state.registry.read();
        if !reg.is_registered(name) {
            return Err(OiError::not_found(format!("app not found: {name}")));
        }
    }

    let db = state.db.lock();
    let (version_id, script) = match &params.version {
        Some(vid) => {
            let (app, script) = crate::runtime::apps::get_version_script(&db, vid)
                .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?
                .ok_or_else(|| OiError::not_found(format!("version not found: {vid}")))?;
            if app != name {
                return Err(OiError::not_found(format!(
                    "version {vid} does not belong to app {name}"
                )));
            }
            (vid.clone(), script)
        }
        None => {
            let (vid, script) = crate::runtime::apps::get_current_script(&db, name)
                .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?
                .ok_or_else(|| OiError::not_found(format!("no script found for app: {name}")))?;
            (vid, script)
        }
    };

    Ok(json!({ "script": script, "version_id": version_id }))
}

// i[app.register]
// i[app.persist]
pub(crate) fn register_app(state: &OiState, params: AppScriptParams) -> HandlerResult {
    let name = params.app.as_str();
    let script = params.script.as_str();

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
        reg.register(
            name.to_owned(),
            script.to_owned(),
            Arc::clone(&state.tick_notify),
            &state.script_limits,
        )
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

    // Create initial version.
    let version_id = {
        let db = state.db.lock();
        crate::runtime::apps::insert_app_version(&db, name, script)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db version: {e}")))?
    };
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(name) {
            entry.version_id = version_id.clone();
        }
    }

    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(name) {
            let db = state.db.lock();
            crate::runtime::apps::sync_script_error_fault(&db, entry);
            crate::runtime::apps::sync_registry_faults(&db, entry);
        }
    }

    tracing::info!(app = %name, "registered app");
    crate::oi::events::app_registered(&state.event_tx, name, &version_id);
    Ok(json!({}))
}

// i[app.deregister]
pub(crate) fn deregister_app(state: &OiState, params: AppParams) -> HandlerResult {
    let name = params.app.as_str();

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

    // Reject if the app is not in the NotInstalled phase.
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(name) {
            let phase = entry.phase.lock();
            if !matches!(*phase, AppPhase::NotInstalled) {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    if matches!(*phase, AppPhase::Uninstalling) {
                        format!("app is still uninstalling: {name}")
                    } else {
                        format!("app is installed; call uninstall first: {name}")
                    },
                ));
            }
            drop(phase);
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
    {
        let db = state.db.lock();
        if let Err(e) = crate::runtime::faults::clear_all_faults_for_app(&db, name) {
            tracing::warn!(app = %name, "failed to clear faults during deregister: {e}");
        }
    }
    {
        let db = state.db.lock();
        if let Err(e) = scaling::delete_scaling_decisions_for_app(&db, name) {
            tracing::warn!(app = %name, "failed to clean up scaling decisions during deregister: {e}");
        }
    }

    // Remove from in-memory registry.
    state.registry.write().deregister(name);

    tracing::info!(app = %name, "deregistered app");
    crate::oi::events::app_deregistered(&state.event_tx, name);
    Ok(json!({}))
}

pub(crate) fn uninstall_app(state: &OiState, params: AppParams) -> HandlerResult {
    let name = params.app.as_str();

    let phase_arc = {
        let reg = state.registry.read();
        let entry = reg
            .get(name)
            .ok_or_else(|| OiError::not_found(format!("app not found: {name}")))?;
        if !matches!(*entry.phase.lock(), AppPhase::Installed) {
            return Err(OiError::new(
                ErrorCode::NotInstalled,
                format!("app is not installed: {name}"),
            ));
        }
        Arc::clone(&entry.phase)
    };

    // Persist the transition before waking the reconciler.
    {
        let db = state.db.lock();
        transition_phase(&phase_arc, AppPhase::Uninstalling, &db, name, "");
    }

    // Wake the reconciler so it starts cleanup immediately.
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(name) {
            entry.tick_notify.notify_one();
        }
    }

    tracing::info!(app = %name, "uninstall initiated");
    Ok(json!({}))
}

// i[app.update]
pub(crate) fn update_app(state: &OiState, params: AppScriptParams) -> HandlerResult {
    let name = params.app.as_str();
    let script = params.script.as_str();

    {
        let reg = state.registry.read();
        if !reg.is_registered(name) {
            return Err(OiError::not_found(format!("app not found: {name}")));
        }
    }

    let previous_version_id = {
        let reg = state.registry.read();
        reg.get(name).map(|e| e.version_id.clone())
    };

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
        state.registry.write().reload(
            name,
            script.to_owned(),
            &loaded_params,
            &state.script_limits,
        );
        {
            let reg = state.registry.read();
            if let Some(entry) = reg.get(name) {
                let db = state.db.lock();
                crate::runtime::apps::sync_script_error_fault(&db, entry);
                crate::runtime::apps::sync_registry_faults(&db, entry);
            }
        }
        // r[impl scaling.clamp]
        {
            let reg = state.registry.read();
            if let Some(entry) = reg.get(name) {
                let def = entry.app.def.lock();
                let deployment_bounds: std::collections::BTreeMap<String, (u16, u16)> = def
                    .resources
                    .iter()
                    .filter_map(|(id, resource)| {
                        if let Resource::Deployment(deployment) = resource {
                            let dep_def = deployment.def.lock();
                            Some((
                                id.name.as_str().to_owned(),
                                (dep_def.scale.start, dep_def.scale.end),
                            ))
                        } else {
                            None
                        }
                    })
                    .collect();
                drop(def);
                let db = state.db.lock();
                if let Err(e) = scaling::clamp_scaling_decisions(&db, name, &deployment_bounds) {
                    tracing::error!(app = %name, error = %e, "failed to clamp scaling decisions");
                }
            }
        }
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

    // Create new version.
    let version_id = {
        let db = state.db.lock();
        crate::runtime::apps::insert_app_version(&db, name, script)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db version: {e}")))?
    };
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(name) {
            entry.previous_version_id = previous_version_id.clone();
            entry.version_id = version_id.clone();
        }
    }
    // Persist again with updated version_id.
    {
        let reg = state.registry.read();
        let entry = reg.get(name).expect("confirmed registered");
        let db = state.db.lock();
        AppRegistry::persist_app(&db, entry)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db update version: {e}")))?;
    }

    // i[forward.script-update] — tear down any forward whose target service is
    // no longer present in the new AppDef (only when the AppDef was reloaded
    // immediately; deferred reload at evaluation boundary is handled separately).
    if !op_in_progress {
        let valid_services: std::collections::HashSet<String> = {
            let reg = state.registry.read();
            if let Some(entry) = reg.get(name) {
                let def = entry.app.def.lock();
                def.resources
                    .keys()
                    .filter(|rid| rid.kind == ResourceKind::Service)
                    .map(|rid| rid.name.as_str().to_owned())
                    .collect()
            } else {
                Default::default()
            }
        };
        let stale = state
            .forwards
            .lock()
            .remove_stale_for_app(name, |fwd| !valid_services.contains(&fwd.service));
        for entry in stale {
            let _ = entry.stop_tx.send(true);
        }
    }

    tracing::info!(app = %name, "updated app");
    crate::oi::events::app_updated(
        &state.event_tx,
        name,
        &version_id,
        previous_version_id.as_deref(),
    );
    Ok(json!({}))
}

// i[impl scale.set]
pub(crate) fn scale_app(state: &OiState, params: ScaleParams) -> HandlerResult {
    let name = params.app.as_str();
    let deployment_name = params.deployment.as_str();
    let direction = params.direction.as_str();

    let reg = state.registry.read();
    let entry = reg
        .get(name)
        .ok_or_else(|| OiError::not_found(format!("app not found: {name}")))?;

    let def = entry.app.def.lock();
    let (low, high) = {
        let mut found = None;
        for (id, resource) in &def.resources {
            if let Resource::Deployment(deployment) = resource
                && id.name.as_str() == deployment_name
            {
                let dep_def = deployment.def.lock();
                found = Some((dep_def.scale.start, dep_def.scale.end));
                break;
            }
        }
        found
            .ok_or_else(|| OiError::not_found(format!("deployment not found: {deployment_name}")))?
    };
    drop(def);

    let (previous_scale, new_scale) = {
        let db = state.db.lock();
        let current = scaling::effective_scale(&db, name, deployment_name, low, high)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db error: {e}")))?;

        let new_scale = match direction {
            "up" => current.saturating_add(1).min(high),
            "down" => current.saturating_sub(1).max(low),
            "to-min" => low,
            _ => {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!("invalid scale direction: {direction}"),
                ));
            }
        };

        // i[impl scale.decision-persistence]
        scaling::save_scaling_decision(&db, name, deployment_name, new_scale)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db error: {e}")))?;

        (current, new_scale)
    };

    entry.tick_notify.notify_one();

    crate::oi::events::scale_changed(
        &state.event_tx,
        name,
        deployment_name,
        new_scale,
        previous_scale,
        low,
        high,
    );

    Ok(json!({
        "scale": new_scale,
        "bounds": { "low": low, "high": high },
    }))
}
