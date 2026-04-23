use std::collections::BTreeMap;

use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};
use seedling_protocol::names::{AppName, TemplateName};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    appdef_json::{
        action_entry_json, install_entry_json, param_schema_entry_json, resource_static_json,
        shell_entry_json,
    },
    apps::{self as apps_handler, AppScriptParams},
};
use crate::{
    oi::{handler::RequestCtx, state::OiState},
    runtime::{self, apps::evaluate_script},
};

#[derive(Deserialize)]
pub(crate) struct CreateParams {
    pub name: TemplateName,
    pub body: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct NameParams {
    pub name: TemplateName,
}

#[derive(Deserialize)]
pub(crate) struct PreviewParams {
    #[serde(default)]
    pub name: Option<TemplateName>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct InstantiateParams {
    pub template: TemplateName,
    pub app: AppName,
}

#[derive(Deserialize)]
pub(crate) struct UpdateParams {
    pub name: TemplateName,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default, deserialize_with = "deserialize_description")]
    pub description: DescriptionUpdate,
}

#[derive(Default)]
pub(crate) enum DescriptionUpdate {
    #[default]
    Unchanged,
    Set(Option<String>),
}

fn deserialize_description<'de, D>(de: D) -> Result<DescriptionUpdate, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // serde_json represents `null` as `Option::None`; a missing field is handled by
    // `#[serde(default)]` above. `Some(..)` means the caller provided `null` or a string
    // explicitly, either of which should set the description. An absent field keeps the
    // existing value.
    let opt: Option<String> = Option::deserialize(de)?;
    Ok(DescriptionUpdate::Set(opt))
}

// i[template.create]
pub(crate) fn create_template(
    state: &OiState,
    params: CreateParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    // i[impl template.name] — validation runs inside TemplateName::deserialize.
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

// i[template.update]
pub(crate) fn update_template(
    state: &OiState,
    params: UpdateParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let name = params.name;

    let body_owned = params.body;
    let description_owned = match params.description {
        DescriptionUpdate::Unchanged => None,
        DescriptionUpdate::Set(s) => Some(s),
    };

    let name_for_db = name.clone();
    let updated = state
        .db
        .call(move |db| {
            runtime::templates::update(
                db,
                &name_for_db,
                runtime::templates::UpdateFields {
                    body: body_owned.as_deref(),
                    description: description_owned.as_ref().map(|s| s.as_deref()),
                },
            )
        })
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?;
    if !updated {
        return Err(OiError::not_found(format!("template not found: {name}")));
    }

    tracing::info!(template = %name, "updated template");
    ctx.events.template_updated(&name);

    Ok(json!({ "updated": true }))
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
            let row = state
                .db
                .call({
                    let name = name.clone();
                    move |db| runtime::templates::get(db, &name)
                })
                .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?
                .ok_or_else(|| OiError::not_found(format!("template not found: {name}")))?;
            (name.into_string(), row.body)
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
        .map(|(k, schema)| param_schema_entry_json(k.as_str(), schema))
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
