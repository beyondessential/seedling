//! Shared AppDef → JSON helpers used by any handler that returns a view of a
//! parsed BSL script — currently `/apps/show` (which layers runtime state on
//! top) and `/templates/preview` (which returns this output verbatim).

use serde_json::{Value, json, to_value};

use crate::defs::{
    action::{ActionDef, ShellDef},
    param_def::ParamDef,
    resource::{Resource, ResourceKind},
};

use super::apps::serialize_param_schema;

/// JSON entries for an action's declared schedules with no live timing data.
/// Used directly by template previews (no associated app means no DB rows to
/// look up); the `/apps/show` handler post-processes the same entries to fill
/// in `last_fired_at` and `next_fire_at` from the schedule table.
fn schedule_entries_json(schedules: &[String]) -> Vec<Value> {
    schedules
        .iter()
        .map(|expr| {
            json!({
                "cronexpr": expr,
                "last_fired_at": Value::Null,
                "next_fire_at": Value::Null,
            })
        })
        .collect()
}

/// Declared scale bounds for a deployment resource. Returns `None` for
/// every other kind, which is all that any caller needs: scaling applies
/// only to deployments.
pub(crate) fn scale_bounds_of(resource: &Resource) -> Option<(u16, u16)> {
    if let Resource::Deployment(deployment) = resource {
        let dep_def = deployment.def.lock();
        Some((*dep_def.scale.start(), *dep_def.scale.end()))
    } else {
        None
    }
}

/// JSON for a resource's declared state: name, type, def, scale bounds for
/// deployments, export metadata for volumes.
///
/// Callers building a richer view (/apps/show) augment the returned object
/// with live `instances`, `faults`, `stopped`, and a `scale.current`.
/// Report as null each string in the described resources' `def` that holds
/// a sensitive value: every object member's value and every array element,
/// at any depth, whatever the member.
// spec: APPS#redacted-values
pub(crate) fn redact_def_strings(
    resources: &mut [Value],
    sensitive: &crate::runtime::redact::SensitiveValues,
) {
    fn walk(value: &mut Value, sensitive: &crate::runtime::redact::SensitiveValues) {
        match value {
            Value::String(s) if sensitive.found_in(s) => *value = Value::Null,
            Value::Array(items) => items.iter_mut().for_each(|v| walk(v, sensitive)),
            Value::Object(members) => members.values_mut().for_each(|v| walk(v, sensitive)),
            _ => {}
        }
    }
    if sensitive.is_empty() {
        return;
    }
    for resource in resources {
        if let Some(def) = resource.get_mut("def") {
            walk(def, sensitive);
        }
        redact_member(resource.pointer_mut("/export/description"), sensitive);
    }
}

/// Report as null the description and default value a definition gives
/// anything described outside a resource's `def`: an app, a dynamic
/// resource, a parameter, or an action or shell and its params.
// spec: APPS#redacted-values
pub(crate) fn redact_definition_text(
    entry: &mut Value,
    sensitive: &crate::runtime::redact::SensitiveValues,
) {
    redact_member(entry.get_mut("description"), sensitive);
    redact_member(entry.get_mut("default_value"), sensitive);
    if let Some(Value::Object(params)) = entry.get_mut("params") {
        for param in params.values_mut() {
            redact_member(param.get_mut("description"), sensitive);
            redact_member(param.get_mut("default_value"), sensitive);
        }
    }
}

fn redact_member(member: Option<&mut Value>, sensitive: &crate::runtime::redact::SensitiveValues) {
    if let Some(value) = member
        && value.as_str().is_some_and(|s| sensitive.found_in(s))
    {
        *value = Value::Null;
    }
}

pub(crate) fn resource_static_json(
    kind: ResourceKind,
    name: &str,
    resource: &Resource,
    app: &crate::defs::app::AppDef,
) -> Value {
    let type_str = kind.type_name();
    let mut obj = json!({
        "name": name,
        "type": type_str,
        "def": to_value(resource.summary(app)).unwrap_or(Value::Null),
    });
    if let Resource::Deployment(deployment) = resource {
        let dep_def = deployment.def.lock();
        obj["scale"] = json!({
            "low": *dep_def.scale.start(),
            "high": *dep_def.scale.end(),
        });
        // spec: CTL#app-priority
        obj["priority"] = json!(dep_def.priority.as_str());
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
}

/// Schema-only JSON for a single param: no runtime `value` or `is_set`.
pub(crate) fn param_schema_entry_json(name: &str, schema: &ParamDef) -> Value {
    json!({
        "name": name,
        "kind": schema.kind.as_str(),
        "required": schema.required,
        "description": schema.description,
        "default_value": schema.default_value,
        "secret": schema.is_secret(),
    })
}

/// JSON for an action.
pub(crate) fn action_entry_json(a: &ActionDef) -> Value {
    json!({
        "name": a.name,
        "description": a.description,
        "kind": "action",
        "params": serialize_param_schema(&a.params),
        "schedules": schedule_entries_json(&a.schedules),
    })
}

pub(crate) fn shell_entry_json(s: &ShellDef) -> Value {
    json!({
        "name": s.name,
        "description": s.description,
        "kind": "shell",
        "params": serialize_param_schema(&s.params),
        "schedules": Value::Array(Vec::new()),
    })
}
