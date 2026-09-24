use std::{collections::BTreeMap, sync::Arc};

use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};
use seedling_protocol::names::{AppName, TemplateName};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    appdef_json::{
        action_entry_json, install_entry_json, param_schema_entry_json, resource_static_json,
        shell_entry_json,
    },
    apps as apps_handler,
    definition::{self, DefinitionInput, Resolved},
};
use crate::{
    oi::{handler::RequestCtx, state::OiState},
    runtime::{
        self,
        apps::evaluate,
        definition::{Bundle, Origin, version},
    },
};

/// A template request's definition: as for an app, with the script text
/// given as `body`.
// i[impl template.definition]
#[derive(Deserialize, Default)]
pub(crate) struct TemplateDefinition {
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub bundle: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub origin: Option<Origin>,
}

impl From<TemplateDefinition> for DefinitionInput {
    fn from(t: TemplateDefinition) -> Self {
        Self {
            script: t.body,
            bundle: t.bundle,
            reference: t.reference,
            origin: t.origin,
        }
    }
}

fn internal(e: impl std::fmt::Display) -> OiError {
    OiError::new(ErrorCode::Internal, format!("db error: {e}"))
}

/// Resolve and check a definition for storing in a template: fetched and
/// held to the bundle rules as for an app, but stored even when the running
/// Seedling does not support it, in which case the rest of its metadata is
/// left for a Seedling that does.
// i[impl template.create]
fn resolve_for_template(
    state: &OiState,
    input: DefinitionInput,
    ctx: &RequestCtx,
) -> Result<Resolved, OiError> {
    let resolved = definition::resolve(state, input, ctx)?;
    if resolved.bundle.supports(&version::running()) {
        resolved.bundle.script()?;
    }
    Ok(resolved)
}

fn supported(bundle: &Bundle) -> bool {
    bundle.supports(&version::running())
}

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
pub(crate) struct CreateParams {
    pub name: TemplateName,
    #[serde(flatten)]
    pub definition: TemplateDefinition,
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
    #[serde(flatten)]
    pub definition: TemplateDefinition,
}

#[derive(Deserialize)]
pub(crate) struct InstantiateParams {
    pub template: TemplateName,
    pub app: AppName,
}

#[derive(Deserialize)]
pub(crate) struct UpdateParams {
    pub name: TemplateName,
    #[serde(flatten)]
    pub definition: TemplateDefinition,
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
    let name_for_check = params.name.clone();
    let already = state
        .db
        .call(move |db| runtime::templates::exists(db, &name_for_check))
        .map_err(internal)?;
    if already {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("template already exists: {}", params.name),
        ));
    }

    let Resolved { bundle, source } = resolve_for_template(state, params.definition.into(), ctx)?;
    let t = runtime::templates::Template {
        name: params.name.clone(),
        bundle,
        source,
        description: params.description,
        created_at: jiff::Timestamp::now().to_string(),
    };

    let name_for_event = t.name.clone();
    let to_insert = t.clone();
    state
        .db
        .call(move |db| runtime::templates::create(db, &to_insert))
        .map_err(internal)?;

    tracing::info!(template = %name_for_event, "created template");
    ctx.events.template_created(&name_for_event);

    Ok(json!({
        "name": t.name,
        "created_at": t.created_at,
    }))
}

// i[template.list]
pub(crate) fn list_templates(state: &OiState) -> HandlerResult {
    let rows = state.db.call(runtime::templates::list).map_err(internal)?;

    let out: Vec<Value> = rows
        .into_iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "created_at": t.created_at,
                "supported": supported(&t.bundle),
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
        .map_err(internal)?
        .ok_or_else(|| OiError::not_found(format!("template not found: {name}")))?;

    // A template for a newer Seedling may name script files this one cannot
    // read; its body is then empty rather than the request failing.
    let body = row
        .bundle
        .script()
        .map(|s| s.text().to_owned())
        .unwrap_or_default();
    Ok(json!({
        "name": row.name,
        "body": body,
        "provenance": row.source.to_json(&row.bundle),
        "supported": supported(&row.bundle),
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

    let input: DefinitionInput = params.definition.into();
    let definition = if input.is_empty() {
        None
    } else {
        Some(resolve_for_template(state, input, ctx)?)
    };
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
                    definition: definition.as_ref().map(|d| (d.bundle.as_ref(), &d.source)),
                    description: description_owned.as_ref().map(|s| s.as_deref()),
                },
            )
        })
        .map_err(internal)?;
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
        .map_err(internal)?;
    if !deleted {
        return Err(OiError::not_found(format!("template not found: {name}")));
    }

    tracing::info!(template = %name, "removed template");
    ctx.events.template_removed(&name);
    Ok(json!({ "removed": true }))
}

// i[template.preview]
pub(crate) fn preview_template(
    state: &OiState,
    params: PreviewParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let input: DefinitionInput = params.definition.into();
    let (display_name, bundle) = match (params.name, input.is_empty()) {
        (Some(_), false) => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                "supply exactly one of `name` or a definition".to_string(),
            ));
        }
        (None, true) => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                "supply one of `name` or a definition".to_string(),
            ));
        }
        (Some(name), true) => {
            let row = state
                .db
                .call({
                    let name = name.clone();
                    move |db| runtime::templates::get(db, &name)
                })
                .map_err(internal)?
                .ok_or_else(|| OiError::not_found(format!("template not found: {name}")))?;
            (name.into_string(), row.bundle)
        }
        (None, false) => (
            "(preview)".to_owned(),
            definition::resolve(state, input, ctx)?.bundle,
        ),
    };

    let empty_params: BTreeMap<String, String> = BTreeMap::new();
    // Previews are not installed — validating the display_name against AppName
    // rules is overkill. Use new_unchecked so the "(preview)" placeholder is
    // allowed to stand in for an unnamed preview.
    let preview_app = AppName::new_unchecked(display_name.clone());
    let (app, err) = evaluate(&preview_app, &bundle, &empty_params, &state.script_limits);
    let def = app.def.load();

    let resources_json: Vec<Value> = def
        .resources
        .iter()
        .map(|(id, resource)| resource_static_json(id.kind, id.name.as_str(), resource, &def))
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
        "description": def.description,
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
        .map_err(internal)?
        .ok_or_else(|| OiError::not_found(format!("template not found: {template_name}")))?;

    // i[impl template.instantiate] — a verbatim copy of the bundle, carrying
    // the template's provenance.
    apps_handler::check_registrable(state, &app_name)?;
    let register_result = apps_handler::register_resolved(
        state,
        &app_name,
        Resolved {
            bundle: Arc::clone(&row.bundle),
            source: row.source,
        },
        ctx,
    )?;

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
