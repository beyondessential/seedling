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
    pub generation: Option<u64>,
}

#[derive(Deserialize)]
pub(crate) struct ListGenerationsParams {
    pub app: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub before: Option<u64>,
}

#[derive(Deserialize)]
pub(crate) struct ProposedParam {
    pub name: String,
    /// `Some(s)` to model setting; `None` to model unsetting.
    pub value: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct PlanParams {
    pub app: String,
    #[serde(default)]
    pub proposed_script: Option<String>,
    #[serde(default)]
    pub proposed_params: Option<Vec<ProposedParam>>,
}

#[derive(Deserialize)]
pub(crate) struct ScaleParams {
    pub app: String,
    pub deployment: String,
    pub scale: u16,
}

// r[impl schedule.prune]
fn sync_action_schedules(state: &OiState, app_name: &str) {
    let valid_pairs: Vec<(String, String)> = {
        let reg = state.registry.read();
        let Some(entry) = reg.get(app_name) else {
            return;
        };
        let def = entry.app.def.lock();
        def.actions
            .values()
            .flat_map(|a| {
                a.schedules
                    .iter()
                    .map(|expr| (a.name.clone(), expr.clone()))
            })
            .collect()
    };

    let db = state.db.lock();
    if let Err(e) = crate::runtime::db::prune_schedules(&db, app_name, &valid_pairs) {
        tracing::warn!(app = %app_name, "failed to prune schedules: {e}");
    }
    if let Err(e) = crate::runtime::db::ensure_schedules(&db, app_name, &valid_pairs) {
        tracing::warn!(app = %app_name, "failed to ensure schedules: {e}");
    }
}

fn validate_name(name: &str) -> Result<(), OiError> {
    if name.starts_with('_') {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "name must not start with an underscore".to_string(),
        ));
    }
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
        let mut has_ready = false;
        for inst in &instances {
            let obs = query_observations(db, inst).unwrap_or_default();
            let state = derive_lifecycle_state(inst, &obs);
            if state == LifecycleState::Ready {
                has_ready = true;
            } else if state != LifecycleState::Unscheduled {
                // Any instance in a non-terminal, non-ready state means
                // this resource group is not fully healthy.
                return false;
            }
            // Unscheduled instances (e.g. old singletons after a
            // singleton-to-scaled transition) are inert — they have been
            // intentionally torn down and must not drag the app to Degraded.
        }
        has_ready
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
    let generation = entry.current_generation;

    let base_status = reg.status_of(name).unwrap();
    let status = effective_app_status(base_status, entry, &state.db.lock());

    // Load stored param values from DB. Names come from AppDef; values come
    // from the params table. Params declared by the script but never set by
    // the operator are shown as null.
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

                if id.kind == ResourceKind::Volume
                    && let Some(Resource::Volume(vol)) = def.resources.get(id)
                {
                    let vol_def = vol.def.lock();
                    if let Some(export_opts) = &vol_def.exported {
                        let mut export = json!({ "exported": true });
                        if let Some(desc) = &export_opts.description {
                            export["description"] = json!(desc);
                        }
                        resource_obj["export"] = export;
                    }
                }

                resource_obj
            })
            .collect()
    };

    let mut desc = json!({
        "status": status.name(),
        "generation": generation,
        "faults": app_faults_json,
        "resources": resources_json,
        "params": params_json,
        "unknown_params": unknown_params_json,
        "actions": actions_json,
        "install_requirements": install_requirements,
    });

    if let AppStatus::Operating { .. } = &status {
        let (action_name, source_generation, target_generation) = state
            .scheduler
            .lock()
            .active()
            .filter(|a| a.app == name)
            .map(|a| (a.action.clone(), a.source_generation, a.target_generation))
            .unwrap_or_else(|| (String::new(), 0, 0));
        desc["current_operation"] = json!({
            "action_name": action_name,
            "barrier": null,
            "source_generation": source_generation,
            "target_generation": target_generation,
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
    let (generation, script) = match params.generation {
        Some(gen_n) => {
            let script = crate::runtime::apps::get_script_at_generation(&db, name, gen_n)
                .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?
                .ok_or_else(|| {
                    OiError::not_found(format!("generation {gen_n} not found for app {name}"))
                })?;
            (gen_n, script)
        }
        None => {
            let (gen_n, script) = crate::runtime::apps::get_current_script(&db, name)
                .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?
                .ok_or_else(|| OiError::not_found(format!("no script found for app: {name}")))?;
            (gen_n, script)
        }
    };

    Ok(json!({ "script": script, "generation": generation }))
}

// i[impl generation.history]
pub(crate) fn list_generations(state: &OiState, params: ListGenerationsParams) -> HandlerResult {
    let name = params.app.as_str();
    {
        let reg = state.registry.read();
        if !reg.is_registered(name) {
            return Err(OiError::not_found(format!("app not found: {name}")));
        }
    }

    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let db = state.db.lock();
    let entries = crate::runtime::generations::list(&db, name, params.before, limit)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;

    // Determine for each entry whether the script content changed relative to
    // the immediately preceding generation (informational, per i[generation.history]).
    let mut script_changed_for: std::collections::BTreeMap<u64, bool> =
        std::collections::BTreeMap::new();
    for entry in &entries {
        let prior = if entry.generation > 1 {
            crate::runtime::generations::script_hash_at(&db, name, entry.generation - 1).ok()
        } else {
            None
        };
        let changed = matches!(
            entry.kind,
            crate::runtime::generations::Kind::Register
                | crate::runtime::generations::Kind::ScriptUpdate
        ) || prior.as_deref() != Some(entry.script_hash.as_str());
        script_changed_for.insert(entry.generation, changed);
    }

    let result: Vec<Value> = entries
        .into_iter()
        .map(|e| {
            let mut obj = serde_json::Map::new();
            obj.insert("generation".into(), json!(e.generation));
            obj.insert("timestamp".into(), json!(e.created_at));
            obj.insert("kind".into(), json!(e.kind.as_str()));
            if let Some(name) = &e.param_name {
                obj.insert("param_name".into(), json!(name));
            }
            if matches!(
                e.kind,
                crate::runtime::generations::Kind::ParamSet
                    | crate::runtime::generations::Kind::ParamUnset
            ) {
                obj.insert(
                    "previous_value".into(),
                    e.previous_value.map_or(Value::Null, Value::String),
                );
                obj.insert(
                    "new_value".into(),
                    e.new_value.map_or(Value::Null, Value::String),
                );
            }
            obj.insert(
                "script_changed".into(),
                json!(
                    script_changed_for
                        .get(&e.generation)
                        .copied()
                        .unwrap_or(false)
                ),
            );
            obj.insert(
                "operation_id".into(),
                e.operation_id.map_or(Value::Null, Value::String),
            );
            obj.insert(
                "outcome".into(),
                e.outcome
                    .as_ref()
                    .map_or(Value::Null, |o| json!(o.as_str())),
            );
            if matches!(
                e.outcome,
                Some(crate::runtime::generations::Outcome::Failed)
            ) && let Some(err) = e.outcome_error
            {
                obj.insert("error".into(), json!(err));
            }
            Value::Object(obj)
        })
        .collect();

    Ok(json!(result))
}

// i[impl plan.dry-run]
pub(crate) fn dry_run_plan(state: &OiState, params: PlanParams) -> HandlerResult {
    let name = params.app.as_str();
    {
        let reg = state.registry.read();
        if !reg.is_registered(name) {
            return Err(OiError::not_found(format!("app not found: {name}")));
        }
    }

    // Empty input → empty diff.
    if params.proposed_script.is_none() && params.proposed_params.is_none() {
        return Ok(json!({
            "diff": Vec::<Value>::new(),
            "on_change_would_fire": Vec::<String>::new(),
        }));
    }

    let (current_script, current_params) = {
        let reg = state.registry.read();
        let entry = reg.get(name).expect("confirmed registered");
        let db = state.db.lock();
        let stored = crate::runtime::apps::load_params_for_app(&db, name).unwrap_or_default();
        (entry.script.clone(), stored)
    };

    // Build the proposed param map from current with the proposals overlaid.
    let mut proposed_param_map = current_params.clone();
    if let Some(props) = &params.proposed_params {
        for p in props {
            match &p.value {
                Some(v) => {
                    proposed_param_map.insert(p.name.clone(), v.clone());
                }
                None => {
                    proposed_param_map.remove(&p.name);
                }
            }
        }
    }

    let proposed_script = params.proposed_script.as_deref().unwrap_or(&current_script);

    let proposed_app = match crate::runtime::apps::evaluate_script(
        name,
        proposed_script,
        &proposed_param_map,
        &state.script_limits,
    ) {
        Ok(a) => a,
        Err(e) => {
            return Ok(json!({ "errors": [e.to_string()] }));
        }
    };
    let current_app = {
        let reg = state.registry.read();
        let entry = reg.get(name).expect("confirmed registered");
        // Clone the existing AppDef shape rather than re-evaluating; the
        // current AppDef is already the result of evaluating current_script
        // with current_params.
        entry.app.clone()
    };

    let cur_def = current_app.def.lock();
    let prop_def = proposed_app.def.lock();

    let mut diff: Vec<Value> = Vec::new();
    for id in prop_def.resources.keys() {
        if !cur_def.resources.contains_key(id) {
            diff.push(json!({
                "resource_type": format!("{:?}", id.kind),
                "resource_name": id.name.as_str(),
                "change": "added",
            }));
        }
    }
    for (id, cur_resource) in &cur_def.resources {
        if let Some(prop_resource) = prop_def.resources.get(id) {
            // Both present — compare by `ResourceSummary` and only emit a
            // `modified` entry when fields actually differ.
            let cur_summary = cur_resource.summary();
            let prop_summary = prop_resource.summary();
            let fields = crate::defs::summary::diff_fields(&cur_summary, &prop_summary);
            if !fields.is_empty() {
                diff.push(json!({
                    "resource_type": format!("{:?}", id.kind),
                    "resource_name": id.name.as_str(),
                    "change": "modified",
                    "fields": fields,
                }));
            }
        } else {
            diff.push(json!({
                "resource_type": format!("{:?}", id.kind),
                "resource_name": id.name.as_str(),
                "change": "removed",
            }));
        }
    }

    // Compute on_change_would_fire: for each param with a registered handler
    // in the proposed AppDef, did its effective value change between current
    // and proposed maps?
    let mut on_change_would_fire: Vec<String> = Vec::new();
    for handler_param in prop_def.param_changes.iter() {
        let cur = current_params.get(handler_param);
        let prop = proposed_param_map.get(handler_param);
        if cur != prop {
            on_change_would_fire.push(handler_param.clone());
        }
    }
    on_change_would_fire.sort();

    Ok(json!({
        "diff": diff,
        "on_change_would_fire": on_change_would_fire,
    }))
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

    // Persist app row first so generations can FK against it.
    {
        let reg = state.registry.read();
        let entry = reg.get(name).expect("just registered");
        let db = state.db.lock();
        AppRegistry::persist_app(&db, entry)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db persist: {e}")))?;
    }

    // r[impl generation.bumps] — initial registration creates generation 1.
    let generation = {
        let db = state.db.lock();
        crate::runtime::generations::bump_register(&db, name, script)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db generation: {e}")))?
    };
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(name) {
            entry.current_generation = generation;
        }
    }
    // Persist again now that current_generation is set.
    {
        let reg = state.registry.read();
        let entry = reg.get(name).expect("just registered");
        let db = state.db.lock();
        AppRegistry::persist_app(&db, entry)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db persist: {e}")))?;
    }

    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(name) {
            let db = state.db.lock();
            crate::runtime::apps::sync_script_error_fault(&db, entry);
            crate::runtime::apps::sync_registry_faults(&db, entry);
        }
    }

    // r[impl schedule.prune]
    sync_action_schedules(state, name);

    tracing::info!(app = %name, generation, "registered app");
    crate::oi::events::app_registered(&state.event_tx, name, generation);
    Ok(json!({ "generation": generation }))
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
    // r[impl generation.deregister]
    {
        let db = state.db.lock();
        if let Err(e) = crate::runtime::generations::delete_for_app(&db, name) {
            tracing::warn!(app = %name, "failed to delete generation history during deregister: {e}");
        }
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
    {
        let db = state.db.lock();
        if let Err(e) = crate::runtime::db::delete_schedules_for_app(&db, name) {
            tracing::warn!(app = %name, "failed to clean up schedules during deregister: {e}");
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

    // Reject if an operation is active or queued for this app.
    if state.scheduler.lock().has_operation_for(name) {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {name}"),
        ));
    }

    let previous_generation = {
        let reg = state.registry.read();
        reg.get(name).map(|e| e.current_generation).unwrap_or(0)
    };

    // Reload script and apply to in-memory AppDef immediately.
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

    // r[impl generation.bumps] — script update bumps the generation.
    let generation = {
        let db = state.db.lock();
        crate::runtime::generations::bump_script_update(&db, name, script)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db generation: {e}")))?
    };
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(name) {
            entry.current_generation = generation;
        }
    }
    // Persist with updated current_generation.
    {
        let reg = state.registry.read();
        let entry = reg.get(name).expect("confirmed registered");
        let db = state.db.lock();
        AppRegistry::persist_app(&db, entry)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db update generation: {e}")))?;
    }

    let op_in_progress = false;
    // i[forward.script-update] — tear down any forward whose target service is
    // no longer present in the new AppDef.
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

    // r[impl schedule.prune]
    sync_action_schedules(state, name);

    tracing::info!(app = %name, generation, "updated app");
    crate::oi::events::app_updated(
        &state.event_tx,
        name,
        generation,
        if previous_generation == 0 {
            None
        } else {
            Some(previous_generation)
        },
    );
    Ok(json!({ "generation": generation }))
}

// i[impl scale.set]
pub(crate) fn scale_app(state: &OiState, params: ScaleParams) -> HandlerResult {
    let name = params.app.as_str();
    let deployment_name = params.deployment.as_str();

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

        let new_scale = params.scale.clamp(low, high);

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
