use std::sync::Arc;

use seedling_protocol::error::{ErrorCode, OiError};
use seedling_protocol::names::{ActionName, AppName};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    defs::{
        install::ParamKind,
        resource::{Resource, ResourceKind},
    },
    oi::{handler::RequestCtx, state::OiState},
    runtime::{
        AppPhase,
        apps::{AppEntry, AppRegistry, AppStatus},
        barrier::oracle::{derive_lifecycle_state, derive_state_with_transition_time},
        desired::list_dynamic_resources_for_app,
        faults,
        history::{find_instances_for_group, query_observations},
        identity::{InstanceId, InstanceVariant, ResourceInstance},
        lifecycle::LifecycleState,
        restart_gens, scaling,
        stopped::{self, kind_str, parse_kind},
        transition_phase,
    },
};

use super::{
    HandlerResult,
    appdef_json::{
        action_entry_json, install_entry_json, param_schema_entry_json, resource_static_json,
        scale_bounds_of, shell_entry_json,
    },
};

#[derive(Deserialize)]
pub(crate) struct AppParams {
    pub app: AppName,
}

#[derive(Deserialize)]
pub(crate) struct AppScriptParams {
    pub app: AppName,
    pub script: String,
}

#[derive(Deserialize)]
pub(crate) struct GetScriptParams {
    pub app: AppName,
    pub generation: Option<u64>,
}

#[derive(Deserialize)]
pub(crate) struct ListGenerationsParams {
    pub app: AppName,
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
    pub app: AppName,
    #[serde(default)]
    pub proposed_script: Option<String>,
    #[serde(default)]
    pub proposed_params: Option<Vec<ProposedParam>>,
}

#[derive(Deserialize)]
pub(crate) struct ScaleParams {
    pub app: AppName,
    pub deployment: String,
    pub scale: u16,
}

#[derive(Deserialize)]
pub(crate) struct RestartParams {
    pub app: AppName,
    pub deployment: String,
}

#[derive(Deserialize)]
pub(crate) struct ResourceStopParams {
    pub app: AppName,
    pub kind: String,
    pub name: String,
}

/// Extract the fields needed to persist an app entry without holding `&AppEntry`.
pub(crate) fn extract_persist_fields(
    entry: &crate::runtime::apps::AppEntry,
) -> (AppName, u64, bool, bool, bool) {
    use crate::runtime::apps::AppPhase;
    let phase = entry.phase.lock();
    let installed = matches!(*phase, AppPhase::Installed | AppPhase::Uninstalling);
    let uninstalling = matches!(*phase, AppPhase::Uninstalling);
    let installing = matches!(*phase, AppPhase::Installing);
    (
        entry.name.clone(),
        entry.current_generation,
        installed,
        uninstalling,
        installing,
    )
}

/// Persist app row fields to the DB.
pub(crate) fn persist_app_fields(
    db: &crate::runtime::db::Db,
    name: &AppName,
    generation_n: u64,
    installed: bool,
    uninstalling: bool,
    installing: bool,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR REPLACE INTO registered_apps \
             (name, installed, uninstalling, installing, current_generation) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            name,
            installed as i64,
            uninstalling as i64,
            installing as i64,
            generation_n as i64,
        ],
    )?;
    Ok(())
}

/// Synchronise script-error and disallowed-registry faults for `entry` without
/// holding `&AppEntry` across the DB call boundary.
pub(crate) fn sync_fault_state(
    db: &crate::runtime::db::DbHandle,
    entry: &crate::runtime::apps::AppEntry,
) {
    use crate::defs::{container::image_registry, resource::Resource};
    let app_name = entry.name.clone();
    let script_error = entry.script_error.clone();
    let used_registries: std::collections::BTreeSet<String> = {
        let def = entry.app.def.load();
        let mut regs = std::collections::BTreeSet::new();
        for resource in def.resources.values() {
            let image = match resource {
                Resource::Deployment(d) => {
                    let dd = d.def.lock();
                    let pod = dd.pod.lock();
                    pod.container.lock().image.clone()
                }
                Resource::Job(j) => {
                    let jd = j.def.lock();
                    let pod = jd.pod.lock();
                    pod.container.lock().image.clone()
                }
                _ => None,
            };
            if let Some(ref img) = image
                && let Some(reg) = image_registry(img)
            {
                regs.insert(reg.to_owned());
            }
        }
        regs
    };
    db.call(move |db| {
        // Sync script_error fault.
        {
            use crate::runtime::faults;
            let existing: Vec<_> = faults::list_active_faults(db, Some(&app_name))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| f.kind == "script_error")
                .collect();
            match &script_error {
                Some((msg, _)) => {
                    let dominated = existing.iter().any(|f| f.description == *msg);
                    if !dominated {
                        for f in &existing {
                            if let Err(e) = faults::clear_fault(db, &f.id, &app_name) {
                                tracing::warn!(app = %app_name, fault_id = %f.id, "failed to clear stale script-error fault: {e}");
                            }
                        }
                        if let Err(e) = faults::file_fault(db, &app_name, None, None, None, "script_error", msg) {
                            tracing::warn!(app = %app_name, "failed to file script-error fault: {e}");
                        }
                    }
                }
                None => {
                    for f in &existing {
                        if let Err(e) = faults::clear_fault(db, &f.id, &app_name) {
                            tracing::warn!(app = %app_name, fault_id = %f.id, "failed to clear script-error fault: {e}");
                        }
                    }
                }
            }
        }
        // Sync disallowed_registry fault.
        {
            use crate::runtime::{faults, registries as reg_mod};
            let allowed: std::collections::BTreeSet<String> =
                reg_mod::list_allowed_registries(db)
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
            let disallowed: Vec<&str> = used_registries
                .iter()
                .filter(|r| !allowed.contains(*r))
                .map(String::as_str)
                .collect();
            let existing: Vec<_> = faults::list_active_faults(db, Some(&app_name))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| f.kind == "disallowed_registry")
                .collect();
            if disallowed.is_empty() {
                for f in &existing {
                    if let Err(e) = faults::clear_fault(db, &f.id, &app_name) {
                        tracing::warn!(app = %app_name, fault_id = %f.id, "failed to clear disallowed_registry fault: {e}");
                    }
                }
            } else {
                let description = format!("image references use disallowed registries: {}", disallowed.join(", "));
                if !existing.iter().any(|f| f.description == description) {
                    for f in &existing {
                        if let Err(e) = faults::clear_fault(db, &f.id, &app_name) {
                            tracing::warn!(app = %app_name, fault_id = %f.id, "failed to clear stale disallowed_registry fault: {e}");
                        }
                    }
                    if let Err(e) = faults::file_fault(db, &app_name, None, None, None, "disallowed_registry", &description) {
                        tracing::warn!(app = %app_name, "failed to file disallowed_registry fault: {e}");
                    }
                }
            }
        }
    });
}

// r[impl schedule.prune]
fn sync_action_schedules(state: &OiState, app_name: &AppName) {
    let valid_pairs: Vec<(ActionName, String)> = {
        let reg = state.registry.read();
        let Some(entry) = reg.get(app_name.as_str()) else {
            return;
        };
        let def = entry.app.def.load();
        def.actions
            .values()
            .flat_map(|a| {
                a.schedules
                    .iter()
                    .map(|expr| (a.name.clone(), expr.clone()))
            })
            .collect()
    };

    let app_name_owned = app_name.clone();
    let valid_pairs_owned = valid_pairs.clone();
    state.db.call(move |db| {
        if let Err(e) = crate::runtime::db::prune_schedules(db, &app_name_owned, &valid_pairs_owned)
        {
            tracing::warn!(app = %app_name_owned, "failed to prune schedules: {e}");
        }
        if let Err(e) =
            crate::runtime::db::ensure_schedules(db, &app_name_owned, &valid_pairs_owned)
        {
            tracing::warn!(app = %app_name_owned, "failed to ensure schedules: {e}");
        }
    });
}

// i[impl app.describe] — fills in `last_fired_at` and `next_fire_at` on each
// action's `schedules` entries. Schedules whose cronexpr is no longer present
// in the AppDef are ignored (they will be pruned by `sync_action_schedules`).
fn enrich_action_schedules(state: &OiState, app: &AppName, actions_json: &mut [Value]) {
    let app_owned = app.clone();
    let rows = state
        .db
        .call(move |db| crate::runtime::db::list_schedules(db, &app_owned).unwrap_or_default());
    if rows.is_empty() {
        return;
    }

    let now = jiff::Timestamp::now();
    let mut last_fired: std::collections::HashMap<(String, String), Option<String>> =
        std::collections::HashMap::new();
    for row in &rows {
        last_fired.insert(
            (row.action.to_string(), row.cronexpr.clone()),
            row.last_fired_at.clone(),
        );
    }

    for action in actions_json.iter_mut() {
        let Some(action_name) = action
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        let Some(schedules) = action.get_mut("schedules").and_then(|v| v.as_array_mut()) else {
            continue;
        };
        for schedule in schedules.iter_mut() {
            let Some(cronexpr) = schedule
                .get("cronexpr")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            let last = last_fired
                .get(&(action_name.clone(), cronexpr.clone()))
                .cloned()
                .unwrap_or(None);
            let next = compute_next_fire(app, &action_name, &cronexpr, last.as_deref(), now);
            schedule["last_fired_at"] = match &last {
                Some(s) => Value::String(s.clone()),
                None => Value::Null,
            };
            schedule["next_fire_at"] = match next {
                Some(ts) => Value::String(ts.to_string()),
                None => Value::Null,
            };
        }
    }
}

fn compute_next_fire(
    app: &AppName,
    action_name: &str,
    cronexpr: &str,
    last_fired_at: Option<&str>,
    now: jiff::Timestamp,
) -> Option<jiff::Timestamp> {
    let action = ActionName::new(action_name).ok()?;
    let crontab = crate::defs::action::parse_cron_expr(cronexpr, app, &action).ok()?;
    let base_time = last_fired_at
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        .unwrap_or(now);
    crontab.find_next(base_time).ok().map(jiff::Timestamp::from)
}

pub(crate) fn validate_name(name: &str) -> Result<(), OiError> {
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

pub(crate) fn install_requirement_kind_str(kind: ParamKind) -> &'static str {
    kind.as_str()
}

pub(crate) fn serialize_param_schema(
    schema: &std::collections::BTreeMap<
        seedling_protocol::names::ParamName,
        crate::defs::install::ParamDef,
    >,
) -> serde_json::Map<String, Value> {
    schema
        .iter()
        .map(|(k, req)| {
            (
                k.as_str().to_owned(),
                json!({
                    "kind": install_requirement_kind_str(req.kind),
                    "required": req.required,
                    "description": req.description,
                    "default_value": req.default_value,
                    "secret": req.is_secret(),
                }),
            )
        })
        .collect()
}

// i[app.list]
// i[impl resource.stop.status]
pub(crate) fn list_apps(state: &OiState) -> HandlerResult {
    let reg = state.registry.read();
    let apps = reg.list();
    let result: Vec<Value> = apps
        .into_iter()
        .map(|(name, base_status)| {
            let status = match reg.get(name.as_str()) {
                Some(entry) => effective_app_status(base_status, entry, &state.db),
                None => base_status,
            };
            let has_stopped = {
                let name_clone = name.clone();
                state.db.call(move |db| {
                    stopped::load_stopped(db, &name_clone)
                        .map(|s| !s.is_empty())
                        .unwrap_or(false)
                })
            };
            let mut obj = json!({
                "name": name,
                "status": status.name(),
                "has_stopped_resources": has_stopped,
            });
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
// i[impl app.status.priority]
pub(crate) fn effective_app_status(
    base: AppStatus,
    entry: &AppEntry,
    db: &crate::runtime::db::DbHandle,
) -> AppStatus {
    if !matches!(base, AppStatus::Running) {
        return base;
    }

    let app_name = entry.name.clone();

    // Collect resource IDs with a brief def lock, then release before touching db.
    let resource_ids: Vec<(ResourceKind, Arc<String>)> = {
        let def = entry.app.def.load();
        def.resources
            .keys()
            .map(|id| (id.kind, Arc::clone(&id.name)))
            .collect()
    };

    db.call(move |db| {
        let has_faults = faults::has_active_faults(db, &app_name).unwrap_or(false);

        let all_ready = resource_ids.iter().all(|(kind, name)| {
            let instances = find_instances_for_group(db, &app_name, *kind, Some(name.as_str()))
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
                } else if matches!(
                    state,
                    LifecycleState::Unscheduled
                        | LifecycleState::Terminating
                        | LifecycleState::Terminated
                ) {
                    // Instances being torn down must not drag the app to Degraded.
                } else {
                    return false;
                }
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
    })
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
    let status = effective_app_status(base_status, entry, &state.db);

    // Load stored param values from DB (both plaintext and secret tables).
    // Names come from AppDef; values come from the params/secret_params tables.
    // Params declared by the script but never set by the operator are shown as null.
    let name_owned = params.app.clone();
    let cipher = std::sync::Arc::clone(&state.cipher);
    let stored_params = state
        .db
        .call(move |db| crate::runtime::apps::load_all_params_for_app(db, &cipher, &name_owned));

    // Fetch all active faults for this app once, then split by level.
    let name_owned = params.app.clone();
    let all_faults_for_app = state
        .db
        .call(move |db| faults::list_active_faults(db, Some(&name_owned)).unwrap_or_default());

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

    // i[impl resource.stop.status]
    let stopped_set = {
        let name_owned = params.app.clone();
        state
            .db
            .call(move |db| stopped::load_stopped(db, &name_owned).unwrap_or_default())
    };

    let def = entry.app.def.load();

    // i[app.describe]
    // i[impl app.describe.param-secret]
    let params_json: Vec<Value> = def
        .params
        .iter()
        .map(|(k, schema)| {
            let mut obj = param_schema_entry_json(k.as_str(), schema);
            let is_set = stored_params.contains_key(k.as_str());
            let value = if schema.is_secret() {
                Value::Null
            } else {
                stored_params
                    .get(k.as_str())
                    .map(|v| Value::String(v.clone()))
                    .unwrap_or(Value::Null)
            };
            obj["value"] = value;
            obj["is_set"] = json!(is_set);
            obj
        })
        .collect();

    // i[app.describe] — params stored in the DB that the current script does
    // not reference; shown for operator awareness only.
    let unknown_params_json: Vec<Value> = stored_params
        .iter()
        .filter(|(k, _)| !def.params.contains_key(k.as_str()))
        .map(|(k, v)| json!({ "name": k, "value": v }))
        .collect();

    // i[app.describe]
    let mut actions_json: Vec<Value> = def.actions.values().map(action_entry_json).collect();
    for s in def.shells.values() {
        actions_json.push(shell_entry_json(s));
    }
    if let Some(inst) = &def.install {
        actions_json.push(install_entry_json(inst));
    }

    // i[impl app.describe] — overlay live schedule timing onto each action's
    // `schedules` array. The bare entries from `action_entry_json` carry
    // `last_fired_at` and `next_fire_at` set to null; here we replace those
    // with the values stored in the schedule table and a freshly computed
    // next-fire timestamp.
    enrich_action_schedules(state, &params.app, &mut actions_json);

    // resources — with instance lifecycle state from DB observations.
    // Only query instances for Installed/Uninstalling apps; NotInstalled
    // apps have no live instances and stale DB records are misleading.
    let query_instances = matches!(
        status,
        AppStatus::Running
            | AppStatus::Degraded
            | AppStatus::Faulted
            | AppStatus::Operating { .. }
            | AppStatus::Installing
            | AppStatus::Uninstalling
    );
    // i[app.describe]
    // Pre-render each resource's static declaration while we still hold the
    // def. The DB closure below only needs plain data (no locks) to augment
    // with instance state, faults, stopped flag, and current scale.
    struct ResourceInfo {
        kind: ResourceKind,
        name_str: String,
        base_json: Value,
        scale_bounds: Option<(u16, u16)>,
    }
    let resource_infos: Vec<ResourceInfo> = def
        .resources
        .iter()
        .map(|(id, resource)| ResourceInfo {
            kind: id.kind,
            name_str: id.name.as_str().to_owned(),
            base_json: resource_static_json(id.kind, id.name.as_str(), resource),
            scale_bounds: scale_bounds_of(resource),
        })
        .collect();

    let name_owned = params.app.clone();
    let all_faults_clone = all_faults_for_app.clone();
    let stopped_set_clone = stopped_set.clone();
    let resources_json: Vec<Value> = state.db.call(move |db| {
        resource_infos
            .into_iter()
            .map(|info| {
                let instances_json: Vec<Value> = if query_instances {
                    find_instances_for_group(db, &name_owned, info.kind, Some(&info.name_str))
                        .unwrap_or_default()
                        .iter()
                        .map(|inst| {
                            let observations = query_observations(db, inst).unwrap_or_default();
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

                let resource_type_str = format!("{:?}", info.kind).to_lowercase();
                let resource_faults: Vec<Value> = all_faults_clone
                    .iter()
                    .filter(|f| {
                        f.resource_type.as_deref() == Some(&resource_type_str)
                            && f.resource_name.as_deref() == Some(&info.name_str)
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

                let is_stopped = stopped_set_clone.contains(&(info.kind, info.name_str.clone()));

                let mut resource_obj = info.base_json;
                resource_obj["instances"] = json!(instances_json);
                resource_obj["faults"] = json!(resource_faults);
                resource_obj["stopped"] = json!(is_stopped);

                // i[impl scale.describe]
                if let Some((low, high)) = info.scale_bounds {
                    let current =
                        scaling::effective_scale(db, &name_owned, &info.name_str, low, high)
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
    });

    // Dynamic resources (created inside action closures, not in AppDef).
    // Returned as a separate top-level field so consumers can choose whether
    // to merge them with `resources` or render them independently — anonymous
    // jobs from an in-flight action have a different lifecycle from declared
    // app resources and a UI may want to surface that distinction.
    let dynamic_resources_json: Vec<Value> = if query_instances {
        let name_owned = params.app.clone();
        let all_faults_clone2 = all_faults_for_app.clone();
        state.db.call(move |db| {
            let dyn_records = list_dynamic_resources_for_app(db, &name_owned).unwrap_or_default();
            let mut out = Vec::new();
            for rec in dyn_records {
                let Some(id) = InstanceId::from_hex(&rec.instance_id) else {
                    continue;
                };
                let Some(kind) = resource_kind_from_debug_str(&rec.kind) else {
                    continue;
                };
                let inst = ResourceInstance {
                    id,
                    app: rec.app.clone(),
                    kind,
                    name: rec.resource_name.clone(),
                    variant: InstanceVariant::Singleton,
                    display_name: rec.display_name.clone(),
                };
                let observations = query_observations(db, &inst).unwrap_or_default();
                let (lifecycle, transition_time) =
                    derive_state_with_transition_time(&inst, &observations);
                let kind_str = format!("{:?}", kind).to_lowercase();
                // Anonymous resources have no `resource_name`; fall back to
                // the display_name so they have a stable identifier in the
                // UI (e.g. `tamanu-central--abc12345`).
                let display_name = rec
                    .resource_name
                    .as_deref()
                    .unwrap_or(&rec.display_name)
                    .to_owned();
                let instance_faults: Vec<Value> = all_faults_clone2
                    .iter()
                    .filter(|f| f.instance_id.as_deref() == Some(&rec.instance_id))
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
                out.push(json!({
                    "name": display_name,
                    "type": kind_str,
                    "anonymous": rec.resource_name.is_none(),
                    "operation_id": rec.operation_id,
                    // l[impl bsl.resource.description]
                    "description": rec.description,
                    "instances": [{
                        "id": rec.instance_id,
                        "display_name": rec.display_name,
                        "lifecycle": format!("{lifecycle:?}"),
                        "transition_time": transition_time.and_then(|t| {
                            jiff::Timestamp::try_from(t).ok().map(|ts| ts.to_string())
                        }),
                    }],
                    "faults": instance_faults,
                }));
            }
            out
        })
    } else {
        Vec::new()
    };

    // i[impl resource.stop.status]
    let stopped_resources_json: Vec<Value> = stopped_set
        .iter()
        .map(|(k, n)| json!({ "kind": kind_str(*k), "name": n }))
        .collect();

    let mut desc = json!({
        "status": status.name(),
        "generation": generation,
        "description": def.description,
        "faults": app_faults_json,
        "resources": resources_json,
        "dynamic_resources": dynamic_resources_json,
        "stopped_resources": stopped_resources_json,
        "params": params_json,
        "unknown_params": unknown_params_json,
        "actions": actions_json,
    });

    // i[impl action.describe.barrier]
    // Surface the in-flight operation (action name, generation transition,
    // currently-blocking barrier) for any status that has a scheduled action
    // attached — install and uninstall are tracked as their own statuses but
    // both run an action closure with the same observable surface.
    if matches!(
        &status,
        AppStatus::Operating { .. } | AppStatus::Installing | AppStatus::Uninstalling
    ) {
        let (action_name, source_generation, target_generation, op_id) = state
            .scheduler
            .lock()
            .active()
            .filter(|a| a.app == name)
            .map(|a| {
                (
                    a.action.clone(),
                    a.source_generation,
                    a.target_generation,
                    Some(a.operation_id.clone()),
                )
            })
            .unwrap_or_else(|| (ActionName::default(), 0, 0, None));

        // i[impl action.describe.barrier]
        // Surface the currently-blocking barrier (if any) so operators can
        // see elapsed time / deadline without tailing logs. Between replay
        // passes the unsatisfied barrier record sits in the action log
        // alongside its started_at_secs timestamp.
        let barrier_json = if let Some(op_id) = op_id {
            let op_id_for_load = op_id.clone();
            let entries = state.db.call(move |db| {
                crate::runtime::history::load_action_log(db, &op_id_for_load).unwrap_or_default()
            });
            entries
                .into_iter()
                .filter_map(|e| {
                    let b = e.barrier?;
                    if b.satisfied {
                        return None;
                    }
                    let started_at = b.started_at_secs?;
                    Some((e.resources, b.required_state, b.deadline_secs, started_at))
                })
                .next()
                .map(|(resources, required_state, deadline_secs, started_at)| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let elapsed_secs = now.saturating_sub(started_at);
                    let resources_json: Vec<Value> = resources
                        .iter()
                        .map(|r| Value::String(r.display_name.clone()))
                        .collect();
                    json!({
                        "resources": resources_json,
                        "required_state": format!("{required_state:?}"),
                        "deadline_secs": deadline_secs,
                        "elapsed_secs": elapsed_secs,
                    })
                })
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };

        desc["current_operation"] = json!({
            "action_name": action_name,
            "barrier": barrier_json,
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

    let name_owned = params.app.clone();
    let (generation, script) = match params.generation {
        Some(gen_n) => {
            let script = state
                .db
                .call(move |db| {
                    crate::runtime::apps::get_script_at_generation(db, &name_owned, gen_n)
                })
                .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?
                .ok_or_else(|| {
                    OiError::not_found(format!("generation {gen_n} not found for app {name}"))
                })?;
            (gen_n, script)
        }
        None => {
            let name_owned2 = params.app.clone();
            let (gen_n, script) = state
                .db
                .call(move |db| crate::runtime::apps::get_current_script(db, &name_owned2))
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

    // Snapshot the live param schema once for redaction decisions.
    let live_params_schema = {
        let reg = state.registry.read();
        reg.get(name)
            .map(|e| e.app.def.load().params.clone())
            .unwrap_or_default()
    };

    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let name_owned = params.app.clone();
    let before = params.before;
    let (entries, script_changed_for) = state
        .db
        .call(move |db| -> rusqlite::Result<_> {
            let entries = crate::runtime::generations::list(db, &name_owned, before, limit)?;

            // Determine for each entry whether the script content changed relative to
            // the immediately preceding generation (informational, per i[generation.history]).
            let mut script_changed_for: std::collections::BTreeMap<u64, bool> =
                std::collections::BTreeMap::new();
            for entry in &entries {
                let prior = if entry.generation > 1 {
                    crate::runtime::generations::script_hash_at(
                        db,
                        &name_owned,
                        entry.generation - 1,
                    )
                    .ok()
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
            Ok((entries, script_changed_for))
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;

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
                // i[impl generation.history]
                // r[impl secret.redaction]
                let is_secret = e
                    .param_name
                    .as_deref()
                    .and_then(|n| live_params_schema.get(n))
                    .map(|d| d.is_secret())
                    .unwrap_or(false);
                let redacted = is_secret || e.previous_value_redacted || e.new_value_redacted;
                if redacted {
                    obj.insert("redacted".into(), json!(true));
                } else {
                    obj.insert(
                        "previous_value".into(),
                        e.previous_value.map_or(Value::Null, Value::String),
                    );
                    obj.insert(
                        "new_value".into(),
                        e.new_value.map_or(Value::Null, Value::String),
                    );
                }
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

    let current_script = {
        let reg = state.registry.read();
        let entry = reg.get(name).expect("confirmed registered");
        entry.script.clone()
    };
    let name_owned = params.app.clone();
    let cipher = std::sync::Arc::clone(&state.cipher);
    let current_params = state
        .db
        .call(move |db| crate::runtime::apps::load_all_params_for_app(db, &cipher, &name_owned));

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

    let (proposed_app, proposed_err) = crate::runtime::apps::evaluate_script(
        &params.app,
        proposed_script,
        &proposed_param_map,
        &state.script_limits,
    );
    if let Some(e) = proposed_err {
        return Ok(json!({ "errors": [e.to_string()] }));
    }
    let current_app = {
        let reg = state.registry.read();
        let entry = reg.get(name).expect("confirmed registered");
        // Clone the existing AppDef shape rather than re-evaluating; the
        // current AppDef is already the result of evaluating current_script
        // with current_params.
        entry.app.clone()
    };

    let cur_def = current_app.def.load();
    let prop_def = proposed_app.def.load();

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
        let cur = current_params.get(handler_param.as_str());
        let prop = proposed_param_map.get(handler_param.as_str());
        if cur != prop {
            on_change_would_fire.push(handler_param.as_str().to_owned());
        }
    }
    on_change_would_fire.sort();

    // i[impl backup.app.validation]
    // If this app is a registered backup app, report any missing required
    // actions as errors in the plan output.
    let mut errors: Vec<String> = Vec::new();
    {
        let name_owned = params.app.clone();
        let is_backup_app = state
            .db
            .call(move |db| crate::runtime::backup_apps::is_registered(db, &name_owned))
            .map_err(|e| OiError::new(ErrorCode::Internal, format!("db backup apps: {e}")))?;
        if is_backup_app {
            let missing: Vec<&str> = seedling_protocol::backup_actions::REQUIRED_ACTIONS
                .iter()
                .copied()
                .filter(|a| !prop_def.actions.contains_key(*a))
                .collect();
            if !missing.is_empty() {
                errors.push(format!(
                    "backup app must declare actions: {}",
                    missing.join(", ")
                ));
            }
        }
    }

    Ok(json!({
        "diff": diff,
        "on_change_would_fire": on_change_would_fire,
        "errors": errors,
    }))
}

// i[app.register]
// i[app.persist]
pub(crate) fn register_app(
    state: &OiState,
    params: AppScriptParams,
    ctx: &RequestCtx,
) -> HandlerResult {
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
            params.app.clone(),
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
        let (app_name, generation_n, installed, uninstalling, installing) =
            extract_persist_fields(entry);
        state
            .db
            .call(move |db| {
                persist_app_fields(
                    db,
                    &app_name,
                    generation_n,
                    installed,
                    uninstalling,
                    installing,
                )
            })
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db persist: {e}")))?;
    }

    // r[impl generation.bumps] — initial registration creates generation 1.
    let name_owned = params.app.clone();
    let script_owned = script.to_owned();
    let generation = state
        .db
        .call(move |db| crate::runtime::generations::bump_register(db, &name_owned, &script_owned))
        .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db generation: {e}")))?;
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
        let (app_name, generation_n, installed, uninstalling, installing) =
            extract_persist_fields(entry);
        state
            .db
            .call(move |db| {
                persist_app_fields(
                    db,
                    &app_name,
                    generation_n,
                    installed,
                    uninstalling,
                    installing,
                )
            })
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db persist: {e}")))?;
    }

    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(name) {
            sync_fault_state(&state.db, entry);
        }
    }

    // r[impl schedule.prune]
    sync_action_schedules(state, &params.app);

    tracing::info!(app = %name, generation, "registered app");
    ctx.events.app_registered(&params.app, generation);
    Ok(json!({ "generation": generation }))
}

// i[app.deregister]
pub(crate) fn deregister_app(
    state: &OiState,
    params: AppParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name = params.app.as_str();

    {
        let reg = state.registry.read();
        if !reg.is_registered(name) {
            return Err(OiError::not_found(format!("app not found: {name}")));
        }
    }

    // Reject if an operation is active or queued for this app.
    if state.scheduler.lock().has_operation_for(&params.app) {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {name}"),
        ));
    }

    // Reject if the app is not in the NotInstalled phase. The message
    // distinguishes the three non-removable phases so operators know whether
    // to wait (Installing, Uninstalling) or run a separate uninstall first
    // (Installed).
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(name) {
            let phase = entry.phase.lock();
            match *phase {
                AppPhase::NotInstalled => {}
                AppPhase::Installing => {
                    return Err(OiError::new(
                        ErrorCode::OperationInProgress,
                        format!("an install is in progress; wait for it to finish: {name}"),
                    ));
                }
                AppPhase::Uninstalling => {
                    return Err(OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("app is still uninstalling: {name}"),
                    ));
                }
                AppPhase::Installed => {
                    return Err(OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("app is installed; call uninstall first: {name}"),
                    ));
                }
            }
            drop(phase);
        }
    }

    // Remove from DB.
    let name_owned = params.app.clone();
    state
        .db
        .call(move |db| {
            AppRegistry::remove_app(db, &name_owned)
                .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db remove: {e}")))?;
            crate::runtime::apps::delete_app_params(db, &name_owned)
                .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db param cleanup: {e}")))?;
            // r[impl generation.deregister]
            if let Err(e) = crate::runtime::generations::delete_for_app(db, &name_owned) {
                tracing::warn!(app = %name_owned, "failed to delete generation history during deregister: {e}");
            }
            if let Err(e) = crate::runtime::faults::clear_all_faults_for_app(db, &name_owned) {
                tracing::warn!(app = %name_owned, "failed to clear faults during deregister: {e}");
            }
            if let Err(e) = scaling::delete_scaling_decisions_for_app(db, &name_owned) {
                tracing::warn!(app = %name_owned, "failed to clean up scaling decisions during deregister: {e}");
            }
            if let Err(e) = restart_gens::delete_restart_gens_for_app(db, &name_owned) {
                tracing::warn!(app = %name_owned, "failed to clean up restart generations during deregister: {e}");
            }
            if let Err(e) = stopped::delete_stopped_for_app(db, &name_owned) {
                tracing::warn!(app = %name_owned, "failed to clean up stopped resources during deregister: {e}");
            }
            if let Err(e) = crate::runtime::db::delete_schedules_for_app(db, &name_owned) {
                tracing::warn!(app = %name_owned, "failed to clean up schedules during deregister: {e}");
            }
            Ok::<_, OiError>(())
        })?;

    // Remove from in-memory registry.
    state.registry.write().deregister(name);

    tracing::info!(app = %name, "deregistered app");
    ctx.events.app_deregistered(&params.app);
    Ok(json!({}))
}

pub(crate) fn uninstall_app(state: &OiState, params: AppParams) -> HandlerResult {
    let name = params.app.as_str();

    let phase_arc = {
        let reg = state.registry.read();
        let entry = reg
            .get(name)
            .ok_or_else(|| OiError::not_found(format!("app not found: {name}")))?;
        match *entry.phase.lock() {
            AppPhase::Installed => {}
            AppPhase::Installing => {
                return Err(OiError::new(
                    ErrorCode::OperationInProgress,
                    format!("install is in progress; cannot uninstall yet: {name}"),
                ));
            }
            AppPhase::NotInstalled | AppPhase::Uninstalling => {
                return Err(OiError::new(
                    ErrorCode::NotInstalled,
                    format!("app is not installed: {name}"),
                ));
            }
        }
        Arc::clone(&entry.phase)
    };

    // Persist the transition before waking the reconciler.
    let name_owned = params.app.clone();
    state.db.call(move |db| {
        transition_phase(&phase_arc, AppPhase::Uninstalling, db, &name_owned, "");
        // i[impl scale.reset-on-uninstall]
        if let Err(e) = scaling::delete_scaling_decisions_for_app(db, &name_owned) {
            tracing::warn!(app = %name_owned, "failed to clear scaling decisions on uninstall: {e}");
        }
        if let Err(e) = restart_gens::delete_restart_gens_for_app(db, &name_owned) {
            tracing::warn!(app = %name_owned, "failed to clear restart generations on uninstall: {e}");
        }
        if let Err(e) = stopped::delete_stopped_for_app(db, &name_owned) {
            tracing::warn!(app = %name_owned, "failed to clear stopped resources on uninstall: {e}");
        }
    });

    // i[impl event.types]
    // Notify clients the app entered Uninstalling. The reconciler will emit a
    // second AppPhaseChanged(not_installed) once teardown completes.
    state
        .event_tx
        .app_phase_changed(&params.app, "uninstalling", None);

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
pub(crate) fn update_app(
    state: &OiState,
    params: AppScriptParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name = params.app.as_str();
    let script = params.script.as_str();

    {
        let reg = state.registry.read();
        if !reg.is_registered(name) {
            return Err(OiError::not_found(format!("app not found: {name}")));
        }
    }

    // Reject if an operation is active or queued for this app.
    if state.scheduler.lock().has_operation_for(&params.app) {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {name}"),
        ));
    }

    let previous_generation = {
        let reg = state.registry.read();
        reg.get(name).map(|e| e.current_generation).unwrap_or(0)
    };

    // i[impl backup.app.validation]
    // If this app is a registered backup app, validate the new script still
    // declares all required backup actions before applying the update.
    // i[impl backup.app.validation]
    {
        let name_owned = params.app.clone();
        let is_backup_app = state
            .db
            .call(move |db| crate::runtime::backup_apps::is_registered(db, &name_owned))
            .map_err(|e| OiError::new(ErrorCode::Internal, format!("db backup apps: {e}")))?;
        if is_backup_app {
            let (proposed, proposed_err) = crate::runtime::apps::evaluate_script(
                &params.app,
                script,
                &std::collections::BTreeMap::new(),
                &state.script_limits,
            );
            if let Some(e) = proposed_err {
                return Err(OiError::new(ErrorCode::ScriptError, e.to_string()));
            }
            let def = proposed.def.load();
            let missing: Vec<&str> = seedling_protocol::backup_actions::REQUIRED_ACTIONS
                .iter()
                .copied()
                .filter(|a| !def.actions.contains_key(*a))
                .collect();
            if !missing.is_empty() {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!("backup app must declare actions: {}", missing.join(", ")),
                ));
            }
        }
    }

    // r[impl actuate.volume.hold]
    // Capture the set of named non-tmpfs volumes in the current AppDef before
    // the reload swaps it out. After reload we diff against the new set to
    // find volumes that were removed from the script and hold their on-disk
    // data for operator review — without this, the reconciler would simply
    // stop iterating over the removed resource on its next tick and the
    // data would sit unreferenced in /opt/seedling/volumes forever.
    let previous_named_volumes: Vec<String> = {
        let reg = state.registry.read();
        reg.get(name)
            .map(|entry| {
                let def = entry.app.def.load();
                def.resources
                    .iter()
                    .filter_map(|(id, resource)| match resource {
                        Resource::Volume(v) if !v.def.lock().tmpfs => {
                            Some(id.name.as_str().to_owned())
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    // Reload script and apply to in-memory AppDef immediately.
    let name_owned = params.app.clone();
    let cipher = std::sync::Arc::clone(&state.cipher);
    let loaded_params = state
        .db
        .call(move |db| crate::runtime::apps::load_all_params_for_app(db, &cipher, &name_owned));
    state.registry.write().reload(
        &params.app,
        script.to_owned(),
        &loaded_params,
        &state.script_limits,
    );

    // r[impl actuate.volume.hold]
    // Diff previous vs current volume resources and hold anything the new
    // script dropped. Runs synchronously with the update so there's no
    // window where the operator sees the old volume as gone but the on-disk
    // data hasn't been relocated yet.
    {
        let current_named_volumes: std::collections::HashSet<String> = {
            let reg = state.registry.read();
            reg.get(name)
                .map(|entry| {
                    let def = entry.app.def.load();
                    def.resources
                        .iter()
                        .filter_map(|(id, resource)| match resource {
                            Resource::Volume(v) if !v.def.lock().tmpfs => {
                                Some(id.name.as_str().to_owned())
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let removed_volumes: Vec<String> = previous_named_volumes
            .into_iter()
            .filter(|n| !current_named_volumes.contains(n))
            .collect();
        if !removed_volumes.is_empty() {
            let vol_store = &state.driver.volume_store;
            for vol_name in &removed_volumes {
                // Each named volume may have one or more resource_instances
                // rows. Use the registry's display_name (the authoritative
                // on-disk name, which for volumes is "<app>-volume-<name>"
                // via kind_slug, not the "<app>-<name>" form Deployments
                // use) — hardcoding a format here would miss future
                // identity-scheme changes and trip on legacy rows with a
                // different slug.
                let name_owned = params.app.clone();
                let vol_name_owned = vol_name.clone();
                let instances = state.db.call(move |db| {
                    crate::runtime::history::find_instances_for_group(
                        db,
                        &name_owned,
                        crate::defs::resource::ResourceKind::Volume,
                        Some(&vol_name_owned),
                    )
                    .unwrap_or_default()
                });
                for inst in &instances {
                    let vol_name = crate::runtime::identity::VolumeName::of_instance(inst);
                    match tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(vol_store.hold(
                            &vol_name,
                            name,
                            "removed from app definition",
                        ))
                    }) {
                        Ok(meta) => {
                            tracing::info!(
                                app = %name,
                                volume = %vol_name,
                                display = %inst.display_name,
                                held_id = %meta.id,
                                "held volume removed from app definition"
                            );
                            // r[impl actuate.volume.hold.events]
                            ctx.events.held_volume_created(
                                meta.id,
                                &params.app,
                                &meta.volume_name,
                                &meta.reason,
                            );
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            // Volume declared but never created on disk
                            // (e.g. the app was never installed, or the
                            // user added + removed the volume quickly).
                            // The instance row still needs cleanup below.
                        }
                        Err(e) => {
                            tracing::error!(
                                app = %name,
                                volume = %vol_name,
                                display = %inst.display_name,
                                "failed to hold removed volume: {e}"
                            );
                        }
                    }
                }
                // Drop the instance rows so the registry matches the new
                // AppDef. Done after the hold so on failure the rows
                // remain and the next /apps/update retry can try again.
                let name_owned = params.app.clone();
                let inst_ids: Vec<_> = instances.iter().map(|i| i.id).collect();
                let inst_displays: Vec<_> =
                    instances.iter().map(|i| i.display_name.clone()).collect();
                state.db.call(move |db| {
                    for (id, disp) in inst_ids.iter().zip(inst_displays.iter()) {
                        if let Err(e) = crate::runtime::history::delete_instance(db, *id) {
                            tracing::warn!(
                                app = %name_owned,
                                instance = %disp,
                                "failed to delete instance row for held volume: {e}"
                            );
                        }
                    }
                });
            }
        }
    }
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(name) {
            sync_fault_state(&state.db, entry);
        }
    }
    // r[impl scaling.clamp]
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(name) {
            let def = entry.app.def.load();
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
            let name_owned = params.app.clone();
            state.db.call(move |db| {
                if let Err(e) = scaling::clamp_scaling_decisions(db, &name_owned, &deployment_bounds) {
                    tracing::error!(app = %name_owned, error = %e, "failed to clamp scaling decisions");
                }
            });
        }
    }
    // Wake reconciler to pick up new desired state.
    if let Some(entry) = state.registry.read().get(name) {
        entry.tick_notify.notify_one();
    }

    // r[impl generation.bumps] — script update bumps the generation.
    let name_owned = params.app.clone();
    let script_owned = script.to_owned();
    let generation = state
        .db
        .call(move |db| {
            crate::runtime::generations::bump_script_update(db, &name_owned, &script_owned)
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db generation: {e}")))?;
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
        let (app_name, generation_n, installed, uninstalling, installing) =
            extract_persist_fields(entry);
        state
            .db
            .call(move |db| {
                persist_app_fields(
                    db,
                    &app_name,
                    generation_n,
                    installed,
                    uninstalling,
                    installing,
                )
            })
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db update generation: {e}")))?;
    }

    let op_in_progress = false;
    // i[forward.script-update] — tear down any forward whose target service is
    // no longer present in the new AppDef.
    if !op_in_progress {
        let valid_services: std::collections::HashSet<String> = {
            let reg = state.registry.read();
            if let Some(entry) = reg.get(name) {
                let def = entry.app.def.load();
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
    sync_action_schedules(state, &params.app);

    // r[impl image.pin.update-reconcile]
    super::images::reconcile_pins_post_update(state, &params.app);

    tracing::info!(app = %name, generation, "updated app");
    ctx.events.app_updated(
        &params.app,
        generation,
        if previous_generation == 0 {
            None
        } else {
            Some(previous_generation)
        },
    );
    Ok(json!({ "generation": generation }))
}

// i[impl deployment.restart]
pub(crate) fn restart_deployment(
    state: &OiState,
    params: RestartParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name = params.app.as_str();
    let deployment_name = params.deployment.as_str();

    let reg = state.registry.read();
    let entry = reg
        .get(name)
        .ok_or_else(|| OiError::not_found(format!("app not found: {name}")))?;

    {
        let def = entry.app.def.load();
        let found = def.resources.iter().any(|(id, resource)| {
            matches!(resource, Resource::Deployment(_)) && id.name.as_str() == deployment_name
        });
        if !found {
            return Err(OiError::not_found(format!(
                "deployment not found: {deployment_name}"
            )));
        }
    }

    let operation_id = uuid::Uuid::new_v4().to_string();

    let name_owned = params.app.clone();
    let deployment_name_owned = deployment_name.to_owned();
    state
        .db
        .call(move |db| restart_gens::bump_restart_gen(db, &name_owned, &deployment_name_owned))
        .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db error: {e}")))?;

    entry.tick_notify.notify_one();

    ctx.events
        .deployment_restarted(&params.app, deployment_name, &operation_id);

    Ok(json!({ "operation_id": operation_id }))
}

// i[impl scale.set]
pub(crate) fn scale_app(state: &OiState, params: ScaleParams, ctx: &RequestCtx) -> HandlerResult {
    let name = params.app.as_str();
    let deployment_name = params.deployment.as_str();

    let reg = state.registry.read();
    let entry = reg
        .get(name)
        .ok_or_else(|| OiError::not_found(format!("app not found: {name}")))?;

    let def = entry.app.def.load();
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

    let name_owned = params.app.clone();
    let deployment_name_owned = deployment_name.to_owned();
    let new_scale_clamped = params.scale.clamp(low, high);
    let (previous_scale, new_scale) = state.db.call(move |db| -> Result<_, OiError> {
        let current = scaling::effective_scale(db, &name_owned, &deployment_name_owned, low, high)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db error: {e}")))?;
        // i[impl scale.decision-persistence]
        scaling::save_scaling_decision(db, &name_owned, &deployment_name_owned, new_scale_clamped)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db error: {e}")))?;
        Ok((current, new_scale_clamped))
    })?;

    entry.tick_notify.notify_one();

    ctx.events
        .scale(params.app.clone(), deployment_name, low, high)
        .changed(new_scale, previous_scale);

    Ok(json!({
        "scale": new_scale,
        "bounds": { "low": low, "high": high },
    }))
}

fn resource_kind_from_debug_str(s: &str) -> Option<ResourceKind> {
    match s {
        "Deployment" => Some(ResourceKind::Deployment),
        "Job" => Some(ResourceKind::Job),
        "Service" => Some(ResourceKind::Service),
        "HttpService" => Some(ResourceKind::HttpService),
        "Volume" => Some(ResourceKind::Volume),
        "ExternalVolume" => Some(ResourceKind::ExternalVolume),
        _ => None,
    }
}

// i[impl resource.stop]
pub(crate) fn stop_resource(
    state: &OiState,
    params: ResourceStopParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let app = params.app.as_str();
    let resource_name = params.name.as_str();

    let kind = parse_kind(&params.kind).filter(|k| {
        matches!(
            k,
            ResourceKind::Deployment | ResourceKind::Job | ResourceKind::Ingress
        )
    });
    let kind = kind.ok_or_else(|| {
        OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "kind {:?} cannot be stopped; only deployment, job, or ingress are stoppable",
                params.kind
            ),
        )
    })?;

    // i[impl resource.stop.no-active-op]
    // Desired-state mutations must not race with a running action, and an
    // action whose resource is about to vanish has no sensible continuation.
    // Operators cancel the active op first (via /apps/action/cancel), then
    // stop resources.
    if state.scheduler.lock().has_operation_for(&params.app) {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!(
                "operation in progress for app: {app}; cancel it first via /apps/action/cancel"
            ),
        ));
    }

    let reg = state.registry.read();
    let entry = reg
        .get(app)
        .ok_or_else(|| OiError::not_found(format!("app not found: {app}")))?;

    {
        let def = entry.app.def.load();
        let found = def
            .resources
            .iter()
            .any(|(id, _)| id.kind == kind && id.name.as_str() == resource_name);
        if !found {
            return Err(OiError::not_found(format!(
                "resource {}/{resource_name} not found in app {app}",
                kind_str(kind)
            )));
        }
    }

    let app_owned = params.app.clone();
    let resource_name_owned = resource_name.to_owned();
    state
        .db
        .call(move |db| stopped::stop_resource(db, &app_owned, kind, &resource_name_owned))
        .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db error: {e}")))?;

    entry.tick_notify.notify_one();

    let ks = kind_str(kind);
    tracing::info!(app = %app, kind = %ks, name = %resource_name, "resource stopped");
    ctx.events.resource_stopped(&params.app, ks, resource_name);

    Ok(json!({}))
}

// i[impl resource.unstop]
pub(crate) fn unstop_resource(
    state: &OiState,
    params: ResourceStopParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let app = params.app.as_str();
    let resource_name = params.name.as_str();

    let kind = parse_kind(&params.kind).ok_or_else(|| {
        OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("unknown resource kind: {:?}", params.kind),
        )
    })?;

    {
        let reg = state.registry.read();
        if !reg.is_registered(app) {
            return Err(OiError::not_found(format!("app not found: {app}")));
        }
    }

    // i[impl resource.stop.no-active-op]
    if state.scheduler.lock().has_operation_for(&params.app) {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!(
                "operation in progress for app: {app}; cancel it first via /apps/action/cancel"
            ),
        ));
    }

    let app_owned = params.app.clone();
    let resource_name_owned = resource_name.to_owned();
    state
        .db
        .call(move |db| stopped::unstop_resource(db, &app_owned, kind, &resource_name_owned))
        .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db error: {e}")))?;

    let ks = kind_str(kind);
    tracing::info!(app = %app, kind = %ks, name = %resource_name, "resource unstopped");
    ctx.events
        .resource_unstopped(&params.app, ks, resource_name);

    let reg = state.registry.read();
    if let Some(entry) = reg.get(app) {
        entry.tick_notify.notify_one();
    }

    Ok(json!({}))
}

// i[impl resource.unstop-all]
pub(crate) fn unstop_all_resources(
    state: &OiState,
    params: AppParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let app = params.app.as_str();

    {
        let reg = state.registry.read();
        if !reg.is_registered(app) {
            return Err(OiError::not_found(format!("app not found: {app}")));
        }
    }

    // i[impl resource.stop.no-active-op]
    if state.scheduler.lock().has_operation_for(&params.app) {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!(
                "operation in progress for app: {app}; cancel it first via /apps/action/cancel"
            ),
        ));
    }

    let app_owned = params.app.clone();
    let stopped_list = state.db.call(move |db| -> Result<_, OiError> {
        let set = stopped::load_stopped(db, &app_owned)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db error: {e}")))?;
        stopped::unstop_all(db, &app_owned)
            .map_err(|e| OiError::new(ErrorCode::ScriptError, format!("db error: {e}")))?;
        Ok(set)
    })?;

    for (kind, name) in &stopped_list {
        let ks = kind_str(*kind);
        tracing::info!(app = %app, kind = %ks, name = %name, "resource unstopped (unstop-all)");
        ctx.events.resource_unstopped(&params.app, ks, name);
    }

    let reg = state.registry.read();
    if let Some(entry) = reg.get(app) {
        entry.tick_notify.notify_one();
    }

    Ok(json!({}))
}
