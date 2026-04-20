use std::collections::BTreeMap;

use rhai::{EvalAltResult, FnPtr, Map, TypeBuilder};

use super::super::action::ShellDef;
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl action.shell]
    builder
        .with_fn(
            "on_shell",
            |this: &mut App, name: &str, closure: FnPtr| -> Result<(), Box<EvalAltResult>> {
                super::super::validate_name(name)?;
                let name_owned: String = name.into();
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.shells.insert(
                        name_owned.clone(),
                        ShellDef {
                            name: name_owned.clone(),
                            description: None,
                            params: BTreeMap::new(),
                        },
                    );
                    d
                });
                super::capture_shell(name.into(), closure);
                Ok(())
            },
        )
        .with_fn(
            "on_shell",
            |this: &mut App,
             name: &str,
             closure: FnPtr,
             options: Map|
             -> Result<(), Box<EvalAltResult>> {
                super::super::validate_name(name)?;
                let desc = super::extract_description(&options);
                // l[impl action.option-params]
                let params = parse_shell_params(&options)?;
                let name_owned: String = name.into();
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.shells.insert(
                        name_owned.clone(),
                        ShellDef {
                            name: name_owned.clone(),
                            description: desc.clone(),
                            params: params.clone(),
                        },
                    );
                    d
                });
                super::capture_shell(name.into(), closure);
                Ok(())
            },
        );
}

fn parse_shell_params(
    options: &Map,
) -> Result<BTreeMap<String, super::super::install::ParamDef>, Box<rhai::EvalAltResult>> {
    match options
        .get("params")
        .and_then(|v| v.read_lock::<Map>().map(|m| m.clone()))
    {
        Some(params_map) => super::install::parse_param_defs(&params_map),
        None => Ok(BTreeMap::new()),
    }
}
