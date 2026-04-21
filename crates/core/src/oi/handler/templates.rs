use std::collections::BTreeMap;

use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};
use seedling_protocol::names::AppName;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    appdef_json::{
        action_entry_json, install_entry_json, param_schema_entry_json, resource_static_json,
        shell_entry_json,
    },
    apps::{self as apps_handler, AppScriptParams, validate_name},
};
use crate::{
    oi::{handler::RequestCtx, state::OiState},
    runtime::{self, apps::evaluate_script},
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
    pub app: AppName,
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
    // Previews are not installed — validating the display_name against AppName
    // rules is overkill. Use new_unchecked so the "(preview)" placeholder is
    // allowed to stand in for an unnamed preview.
    let preview_app = AppName::new_unchecked(display_name.clone());
    let (app, err) = evaluate_script(&preview_app, &body, &empty_params, &state.script_limits);
    let def = app.def.load();

    let resources_json: Vec<Value> = def
        .resources
        .iter()
        .map(|(id, resource)| resource_static_json(id.kind, id.name.as_str(), resource))
        .collect();

    let params_json: Vec<Value> = def
        .params
        .iter()
        .map(|(k, schema)| param_schema_entry_json(k, schema))
        .collect();

    let mut actions_json: Vec<Value> = def.actions.values().map(action_entry_json).collect();
    for s in def.shells.values() {
        actions_json.push(shell_entry_json(s));
    }
    if let Some(inst) = &def.install {
        actions_json.push(install_entry_json(inst));
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
