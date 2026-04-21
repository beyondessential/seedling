//! Shared AppDef → JSON helpers used by any handler that returns a view of a
//! parsed BSL script — currently `/apps/show` (which layers runtime state on
//! top) and `/templates/preview` (which returns this output verbatim).

use serde_json::{Value, json, to_value};

use crate::defs::{
    action::{ActionDef, ShellDef},
    install::{InstallDef, ParamDef},
    resource::{Resource, ResourceKind},
};

use super::apps::{install_requirement_kind_str, serialize_param_schema};

/// Declared scale bounds for a deployment resource. Returns `None` for
/// every other kind, which is all that any caller needs: scaling applies
/// only to deployments.
pub(crate) fn scale_bounds_of(resource: &Resource) -> Option<(u16, u16)> {
    if let Resource::Deployment(deployment) = resource {
        let dep_def = deployment.def.lock();
        Some((dep_def.scale.start, dep_def.scale.end))
    } else {
        None
    }
}

/// JSON for a resource's declared state: name, type, def, scale bounds for
/// deployments, export metadata for volumes.
///
/// Callers building a richer view (/apps/show) augment the returned object
/// with live `instances`, `faults`, `stopped`, and a `scale.current`.
pub(crate) fn resource_static_json(kind: ResourceKind, name: &str, resource: &Resource) -> Value {
    let type_str = format!("{:?}", kind).to_lowercase();
    let mut obj = json!({
        "name": name,
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
}

/// Schema-only JSON for a single param: no runtime `value` or `is_set`.
pub(crate) fn param_schema_entry_json(name: &str, schema: &ParamDef) -> Value {
    json!({
        "name": name,
        "kind": install_requirement_kind_str(schema.kind),
        "required": schema.required,
        "description": schema.description,
        "default_value": schema.default_value,
        "secret": schema.is_secret(),
    })
}

/// JSON for a lifecycle / scheduled action. The Start action is tagged as
/// `lifecycle` (not `action`) per `l[action.start.no-manual-invoke]`.
pub(crate) fn action_entry_json(a: &ActionDef) -> Value {
    // l[impl action.start.no-manual-invoke]
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
}

pub(crate) fn shell_entry_json(s: &ShellDef) -> Value {
    json!({
        "name": s.name,
        "description": s.description,
        "kind": "shell",
        "params": serialize_param_schema(&s.params),
    })
}

pub(crate) fn install_entry_json(install: &InstallDef) -> Value {
    json!({
        "name": "install",
        "description": null,
        "kind": "install",
        "params": serialize_param_schema(&install.requirements),
    })
}
