use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json, to_value};

use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};

use crate::{
    defs::resource::Resource,
    oi::{handler::RequestCtx, state::OiState},
    runtime::{self, apps::evaluate_script},
};

use super::apps::{
    self as apps_handler, AppScriptParams, install_requirement_kind_str, serialize_param_schema,
    validate_name,
};

#[derive(Deserialize)]
pub(crate) struct CreateParams {
    pub name: String,
    pub body: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct NameParams {
    pub name: String,
}

#[derive(Deserialize)]
pub(crate) struct PreviewParams {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct InstantiateParams {
    pub template: String,
    pub app: String,
}

// i[template.create]
pub(crate) fn create_template(
    state: &OiState,
    params: CreateParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    // i[impl template.name]
    validate_name(&params.name)?;

    let t = runtime::templates::Template {
        name: params.name.clone(),
        body: params.body,
        description: params.description,
        created_at: jiff::Timestamp::now().to_string(),
    };

    let name_for_check = t.name.clone();
    let already = state
        .db
        .call(move |db| runtime::templates::exists(db, &name_for_check))
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?;
    if already {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("template already exists: {}", t.name),
        ));
    }

    let name_for_event = t.name.clone();
    let to_insert = t.clone();
    state
        .db
        .call(move |db| runtime::templates::create(db, &to_insert))
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?;

    tracing::info!(template = %name_for_event, "created template");
    ctx.events.template_created(&name_for_event);

    Ok(json!({
        "name": t.name,
        "created_at": t.created_at,
    }))
}

// i[template.list]
pub(crate) fn list_templates(state: &OiState) -> HandlerResult {
    let rows = state
        .db
        .call(runtime::templates::list)
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?;

    let out: Vec<Value> = rows
        .into_iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "created_at": t.created_at,
            })
        })
        .collect();
    Ok(json!(out))
}

// i[template.show]
pub(crate) fn show_template(state: &OiState, params: NameParams) -> HandlerResult {
    let name = params.name;
    let name_for_db = name.clone();
    let row = state
        .db
        .call(move |db| runtime::templates::get(db, &name_for_db))
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?
        .ok_or_else(|| OiError::not_found(format!("template not found: {name}")))?;

    Ok(json!({
        "name": row.name,
        "body": row.body,
        "description": row.description,
        "created_at": row.created_at,
    }))
}

// i[template.remove]
pub(crate) fn remove_template(
    state: &OiState,
    params: NameParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name = params.name;
    let name_for_db = name.clone();
    let deleted = state
        .db
        .call(move |db| runtime::templates::delete(db, &name_for_db))
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?;
    if !deleted {
        return Err(OiError::not_found(format!("template not found: {name}")));
    }

    tracing::info!(template = %name, "removed template");
    ctx.events.template_removed(&name);
    Ok(json!({ "removed": true }))
}

// i[template.preview]
pub(crate) fn preview_template(state: &OiState, params: PreviewParams) -> HandlerResult {
    let (display_name, body) = match (params.name, params.body) {
        (Some(_), Some(_)) => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                "supply exactly one of `name` or `body`".to_string(),
            ));
        }
        (None, None) => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                "supply one of `name` or `body`".to_string(),
            ));
        }
        (Some(name), None) => {
            let lookup = name.clone();
            let row = state
                .db
                .call(move |db| runtime::templates::get(db, &lookup))
                .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?
                .ok_or_else(|| OiError::not_found(format!("template not found: {name}")))?;
            (name, row.body)
        }
        (None, Some(body)) => ("(preview)".to_owned(), body),
    };

    let empty_params: BTreeMap<String, String> = BTreeMap::new();
    let (app, err) = evaluate_script(&display_name, &body, &empty_params, &state.script_limits);
    let def = app.def.load();

    let resources_json: Vec<Value> = def
        .resources
        .iter()
        .map(|(id, resource)| {
            let type_str = format!("{:?}", id.kind).to_lowercase();
            let mut obj = json!({
                "name": id.name.as_str(),
                "type": type_str,
                "def": to_value(resource.summary()).unwrap_or(Value::Null),
            });
            if let Resource::Deployment(deployment) = resource {
                let dep_def = deployment.def.lock();
                obj["scale"] = json!({
                    "low": dep_def.scale.start,
                    "high": dep_def.scale.end,
                });
            }
            if let Resource::Volume(vol) = resource {
                let vol_def = vol.def.lock();
                if let Some(export_opts) = &vol_def.exported {
                    let mut export = json!({ "exported": true });
                    if let Some(desc) = &export_opts.description {
                        export["description"] = json!(desc);
                    }
                    obj["export"] = export;
                }
            }
            obj
        })
        .collect();

    let params_json: Vec<Value> = def
        .params
        .iter()
        .map(|(k, schema)| {
            json!({
                "name": k,
                "kind": install_requirement_kind_str(schema.kind),
                "required": schema.required,
                "description": schema.description,
                "default_value": schema.default_value,
                "secret": schema.is_secret(),
            })
        })
        .collect();

    let mut actions_json: Vec<Value> = def
        .actions
        .values()
        .map(|a| {
            let kind = if a.name == "start" {
                "lifecycle"
            } else {
                "action"
            };
            let mut obj = json!({
                "name": a.name,
                "description": a.description,
                "kind": kind,
                "params": serialize_param_schema(&a.params),
            });
            if !a.schedules.is_empty() {
                obj["schedules"] = json!(a.schedules);
            }
            obj
        })
        .collect();

    for s in def.shells.values() {
        actions_json.push(json!({
            "name": s.name,
            "description": s.description,
            "kind": "shell",
            "params": serialize_param_schema(&s.params),
        }));
    }

    if let Some(inst) = &def.install {
        actions_json.push(json!({
            "name": "install",
            "description": null,
            "kind": "install",
            "params": serialize_param_schema(&inst.requirements),
        }));
    }

    Ok(json!({
        "resources": resources_json,
        "params": params_json,
        "actions": actions_json,
        "script_error": err.map(|e| e.to_string()),
    }))
}

// i[template.instantiate]
pub(crate) fn instantiate_template(
    state: &OiState,
    params: InstantiateParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let template_name = params.template;
    let app_name = params.app;

    let lookup = template_name.clone();
    let row = state
        .db
        .call(move |db| runtime::templates::get(db, &lookup))
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?
        .ok_or_else(|| OiError::not_found(format!("template not found: {template_name}")))?;

    let register_params = AppScriptParams {
        app: app_name.clone(),
        script: row.body,
    };
    let register_result = apps_handler::register_app(state, register_params, ctx)?;

    tracing::info!(template = %template_name, app = %app_name, "instantiated template");
    ctx.events.template_instantiated(&template_name, &app_name);

    let generation = register_result
        .get("generation")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Ok(json!({
        "app": app_name,
        "generation": generation,
    }))
}
