use std::collections::BTreeMap;

use rhai::{EvalAltResult, FnPtr, Map, TypeBuilder};
use seedling_protocol::names::{ParamName, ShellName};

use super::super::action::ShellDef;
use super::App;

/// Declare the shell `shell_name` on the app.
// spec: ACN#shells
fn declare_shell(
    this: &mut App,
    shell_name: ShellName,
    closure: FnPtr,
    description: Option<String>,
    params: BTreeMap<ParamName, super::super::install::ParamDef>,
) -> Result<(), Box<EvalAltResult>> {
    if this.def.load().shells.contains_key(&shell_name) {
        return Err(format!(
            "a shell named {:?} is already declared",
            shell_name.as_str()
        )
        .into());
    }
    this.def.rcu(|d| {
        let mut d = (**d).clone();
        d.shells.insert(
            shell_name.clone(),
            ShellDef {
                name: shell_name.clone(),
                description: description.clone(),
                params: params.clone(),
            },
        );
        d
    });
    super::capture_shell(shell_name, closure);
    Ok(())
}

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // spec: ACN#shells
    builder
        .with_fn(
            "on_shell",
            |this: &mut App, name: &str, closure: FnPtr| -> Result<(), Box<EvalAltResult>> {
                let shell_name = ShellName::new(name)
                    .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
                declare_shell(this, shell_name, closure, None, BTreeMap::new())
            },
        )
        .with_fn(
            "on_shell",
            |this: &mut App,
             name: &str,
             closure: FnPtr,
             options: Map|
             -> Result<(), Box<EvalAltResult>> {
                let shell_name = ShellName::new(name)
                    .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
                let desc = super::extract_description(&options)?;
                // spec: ACN#param-schemas
                let params = parse_shell_params(&options)?;
                declare_shell(this, shell_name, closure, desc, params)
            },
        );
}

fn parse_shell_params(
    options: &Map,
) -> Result<BTreeMap<ParamName, super::super::install::ParamDef>, Box<rhai::EvalAltResult>> {
    match options
        .get("params")
        .and_then(|v| v.read_lock::<Map>().map(|m| m.clone()))
    {
        Some(params_map) => super::install::parse_param_defs(&params_map, true),
        None => Ok(BTreeMap::new()),
    }
}
