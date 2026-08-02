use std::{collections::BTreeMap, str::FromStr as _};

use rhai::{EvalAltResult, FnPtr, Map, TypeBuilder};
use seedling_protocol::names::ParamName;

use super::super::install::{InstallDef, ParamDef, ParamKind};
use super::super::take::{take_bool, take_map, take_string};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl action.install]
    builder
        .with_fn("on_install", |this: &mut App, closure: FnPtr| {
            this.def.rcu(|d| {
                let mut d = (**d).clone();
                d.install = Some(InstallDef {
                    requirements: BTreeMap::new(),
                });
                d
            });
            super::capture_install(closure);
        })
        .with_fn(
            "on_install",
            |this: &mut App, closure: FnPtr, config: Map| -> Result<(), Box<EvalAltResult>> {
                // l[impl action.install.requirements]
                let params_map = config
                    .get("params")
                    .cloned()
                    .map(|v| take_map("on_install params", v))
                    .transpose()?
                    .unwrap_or_default();
                let reqs = parse_param_defs(&params_map, false)?;
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.install = Some(InstallDef {
                        requirements: reqs.clone(),
                    });
                    d
                });
                super::capture_install(closure);
                Ok(())
            },
        );
}

// l[impl action.install.requirements.kind-unknown]
// l[impl action.option-params]
// l[impl action.params.volume]
pub(super) fn parse_param_defs(
    map: &Map,
    allow_volume: bool,
) -> Result<BTreeMap<ParamName, ParamDef>, Box<EvalAltResult>> {
    let mut reqs = BTreeMap::new();
    for (key, value) in map {
        // A non-map entry used to be skipped outright, so the param simply
        // did not exist — and a non-string `kind` fell back to the default,
        // quietly installing a `password` param as plain text.
        let req_map = take_map(&format!("param `{key}`"), value.clone())?;

        let kind = match req_map.get("kind").cloned() {
            Some(v) => {
                let s = take_string(&format!("param `{key}` kind"), v)?;
                ParamKind::from_str(&s).map_err(|_| {
                    Box::<EvalAltResult>::from(format!("unknown param kind: \"{s}\""))
                })?
            }
            None => ParamKind::default(),
        };
        if !allow_volume && !kind.allowed_static() {
            return Err(format!(
                "param '{key}' uses kind '{}', which is only valid in action or shell \
                 param schemas; static params should use external_volume mappings instead",
                kind.as_str()
            )
            .into());
        }

        let required = req_map
            .get("required")
            .cloned()
            .map(|v| take_bool(&format!("param `{key}` required"), v))
            .transpose()?
            .unwrap_or(true);

        let default_value = req_map
            .get("default_value")
            .cloned()
            .map(|v| take_string(&format!("param `{key}` default_value"), v))
            .transpose()?;

        let description = req_map
            .get("description")
            .cloned()
            .map(|v| take_string(&format!("param `{key}` description"), v))
            .transpose()?;

        let secret = req_map
            .get("secret")
            .cloned()
            .map(|v| take_bool(&format!("param `{key}` secret"), v))
            .transpose()?
            .unwrap_or(false);

        let param_name = ParamName::new(key.as_str())
            .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
        reqs.insert(
            param_name,
            ParamDef {
                kind,
                required,
                default_value,
                description,
                secret,
            },
        );
    }
    Ok(reqs)
}
