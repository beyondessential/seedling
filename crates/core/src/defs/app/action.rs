use std::collections::BTreeMap;

use rhai::{EvalAltResult, FnPtr, Map, TypeBuilder};

use super::super::action::{Action, ActionDef};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl action.type]
    // l[impl action.option-description]
    builder
        .with_fn(
            "on_action",
            |this: &mut App, name: &str, closure: FnPtr| -> Result<Action, Box<EvalAltResult>> {
                super::super::validate_name(name)?;
                let app_name = this.def.load().name.clone();
                let name_owned: String = name.into();
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.actions.insert(
                        name_owned.clone(),
                        ActionDef {
                            name: name_owned.clone(),
                            description: None,
                            schedules: Vec::new(),
                            params: BTreeMap::new(),
                        },
                    );
                    d
                });
                super::capture_action(name.into(), closure);
                Ok(Action::new(name.into(), app_name))
            },
        )
        .with_fn(
            "on_action",
            |this: &mut App,
             name: &str,
             closure: FnPtr,
             options: Map|
             -> Result<Action, Box<EvalAltResult>> {
                super::super::validate_name(name)?;
                let app_name = this.def.load().name.clone();
                let desc = super::extract_description(&options);
                // l[impl action.option-params]
                let params = parse_action_params(&options)?;
                let name_owned: String = name.into();
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.actions.insert(
                        name_owned.clone(),
                        ActionDef {
                            name: name_owned.clone(),
                            description: desc.clone(),
                            schedules: Vec::new(),
                            params: params.clone(),
                        },
                    );
                    d
                });
                super::capture_action(name.into(), closure);
                Ok(Action::new(name.into(), app_name))
            },
        );

    // l[impl action.start]
    builder
        .with_fn("on_start", |this: &mut App, closure: FnPtr| -> Action {
            let app_name = this.def.load().name.clone();
            this.def.rcu(|d| {
                let mut d = (**d).clone();
                d.actions.insert(
                    "start".into(),
                    ActionDef {
                        name: "start".into(),
                        description: None,
                        schedules: Vec::new(),
                        params: BTreeMap::new(),
                    },
                );
                d
            });
            super::capture_action("start".into(), closure);
            Action::new("start".into(), app_name)
        })
        .with_fn(
            "on_start",
            |this: &mut App, closure: FnPtr, options: Map| -> Action {
                let app_name = this.def.load().name.clone();
                let desc = super::extract_description(&options);
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.actions.insert(
                        "start".into(),
                        ActionDef {
                            name: "start".into(),
                            description: desc.clone(),
                            schedules: Vec::new(),
                            params: BTreeMap::new(),
                        },
                    );
                    d
                });
                super::capture_action("start".into(), closure);
                Action::new("start".into(), app_name)
            },
        );
}

fn parse_action_params(
    options: &Map,
) -> Result<
    std::collections::BTreeMap<String, super::super::install::ParamDef>,
    Box<rhai::EvalAltResult>,
> {
    match options
        .get("params")
        .and_then(|v| v.read_lock::<Map>().map(|m| m.clone()))
    {
        Some(params_map) => super::install::parse_param_defs(&params_map),
        None => Ok(BTreeMap::new()),
    }
}
