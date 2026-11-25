use std::collections::BTreeMap;

use rhai::{EvalAltResult, Map, TypeBuilder};
use seedling_protocol::names::ParamName;

use super::super::param_def::{ParamDef, ParamKind, SuppliedValue, take_kind};
use super::super::regex::Budget;
use super::super::take::{reject_unknown_keys, take_bool, take_map, take_string};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // spec: PARAM#declaration
    // spec: PARAM#schema
    builder.with_fn(
        "param",
        |this: &mut App, name: &str| -> Result<super::super::param::Param, Box<EvalAltResult>> {
            let param_name =
                ParamName::new(name).map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
            let value = this.stored.lock().get(name).cloned();
            // A validator may look up a parameter the script declared, but
            // naming one that is not declared would declare it.
            if !this.def.load().params.contains_key(&param_name) {
                super::ensure_not_validating()?;
            }
            let name_for_insert = param_name.clone();
            this.def.rcu(|d| {
                let mut d = (**d).clone();
                d.params
                    .entry(name_for_insert.clone())
                    .or_insert_with(|| ParamDef {
                        kind: ParamKind::Text,
                        required: false,
                        default_value: None,
                        description: None,
                        secret: None,
                    });
                d
            });
            Ok(super::super::param::Param {
                name: param_name,
                value,
                app: this.clone(),
            })
        },
    );
}

// spec: ACN#param-kinds
// spec: ACN#param-schemas
// spec: ACN#volume-params
pub(super) fn parse_param_defs(
    map: &Map,
    regex_limit: usize,
) -> Result<BTreeMap<ParamName, ParamDef>, Box<EvalAltResult>> {
    let mut reqs = BTreeMap::new();
    for (key, value) in map {
        // A non-map entry used to be skipped outright, so the param simply
        // did not exist — and a non-string `kind` fell back to the default,
        // quietly declaring a `password` param as plain text.
        let req_map = take_map(&format!("param `{key}`"), value.clone())?;
        reject_unknown_keys(
            &format!("param `{key}`"),
            &req_map,
            &["kind", "required", "default_value", "description", "secret"],
        )?;

        let kind = req_map
            .get("kind")
            .cloned()
            .map(|v| take_kind(&format!("param `{key}` kind"), v))
            .transpose()?
            .unwrap_or_default();
        let required = req_map
            .get("required")
            .cloned()
            .map(|v| take_bool(&format!("param `{key}` required"), v))
            .transpose()?
            .unwrap_or(true);

        // spec: ACN#param-schemas
        let default_value = req_map
            .get("default_value")
            .cloned()
            .map(|v| SuppliedValue::take(&format!("param `{key}` default_value"), v))
            .transpose()?
            .filter(|v| !v.is_empty());

        let description = req_map
            .get("description")
            .cloned()
            .map(|v| take_string(&format!("param `{key}` description"), v))
            .transpose()?;

        let secret = req_map
            .get("secret")
            .cloned()
            .map(|v| take_bool(&format!("param `{key}` secret"), v))
            .transpose()?;

        let param_name = ParamName::new(key.as_str())
            .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
        let def = ParamDef {
            kind,
            required,
            default_value,
            description,
            secret,
        };
        // spec: ACN#param-schemas
        def.check_default(&mut Budget::fresh(regex_limit))
            .map_err(|e| -> Box<EvalAltResult> {
                format!("param `{key}` default_value: {e}").into()
            })?;
        // spec: ACN#param-schemas
        def.check_secret()
            .map_err(|e| -> Box<EvalAltResult> { format!("param `{key}` secret: {e}").into() })?;
        reqs.insert(param_name, def);
    }
    Ok(reqs)
}
