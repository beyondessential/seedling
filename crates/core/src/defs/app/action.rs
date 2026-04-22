use std::collections::BTreeMap;

use rhai::{EvalAltResult, FnPtr, Map, TypeBuilder};
use seedling_protocol::names::{ActionName, ParamName};

use super::super::action::{Action, ActionDef};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl action.type]
    // l[impl action.option-description]
    builder
        .with_fn(
            "on_action",
            |this: &mut App, name: &str, closure: FnPtr| -> Result<Action, Box<EvalAltResult>> {
                let action_name: ActionName = ActionName::new(name)
                    .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
                let app_name = this.def.load().name.clone();
                let name_for_insert = action_name.clone();
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.actions.insert(
                        name_for_insert.clone(),
                        ActionDef {
                            name: name_for_insert.clone(),
                            description: None,
                            schedules: Vec::new(),
                            params: BTreeMap::new(),
                        },
                    );
                    d
                });
                super::capture_action(action_name.clone(), closure);
                Ok(Action::new(action_name, app_name))
            },
        )
        .with_fn(
            "on_action",
            |this: &mut App,
             name: &str,
             closure: FnPtr,
             options: Map|
             -> Result<Action, Box<EvalAltResult>> {
                let action_name: ActionName = ActionName::new(name)
                    .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
                let app_name = this.def.load().name.clone();
                let desc = super::extract_description(&options);
                // l[impl action.option-params]
                let params = parse_action_params(&options)?;
                let name_for_insert = action_name.clone();
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.actions.insert(
                        name_for_insert.clone(),
                        ActionDef {
                            name: name_for_insert.clone(),
                            description: desc.clone(),
                            schedules: Vec::new(),
                            params: params.clone(),
                        },
                    );
                    d
                });
                super::capture_action(action_name.clone(), closure);
                Ok(Action::new(action_name, app_name))
            },
        );

    // l[impl action.start]
    builder
        .with_fn("on_start", |this: &mut App, closure: FnPtr| -> Action {
            let app_name = this.def.load().name.clone();
            let start_name = ActionName::new_unchecked("start");
            let name_for_insert = start_name.clone();
            this.def.rcu(|d| {
                let mut d = (**d).clone();
                d.actions.insert(
                    name_for_insert.clone(),
                    ActionDef {
                        name: name_for_insert.clone(),
                        description: None,
                        schedules: Vec::new(),
                        params: BTreeMap::new(),
                    },
                );
                d
            });
            super::capture_action(start_name.clone(), closure);
            Action::new(start_name, app_name)
        })
        .with_fn(
            "on_start",
            |this: &mut App, closure: FnPtr, options: Map| -> Action {
                let app_name = this.def.load().name.clone();
                let desc = super::extract_description(&options);
                let start_name = ActionName::new_unchecked("start");
                let name_for_insert = start_name.clone();
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.actions.insert(
                        name_for_insert.clone(),
                        ActionDef {
                            name: name_for_insert.clone(),
                            description: desc.clone(),
                            schedules: Vec::new(),
                            params: BTreeMap::new(),
                        },
                    );
                    d
                });
                super::capture_action(start_name.clone(), closure);
                Action::new(start_name, app_name)
            },
        );
}

fn parse_action_params(
    options: &Map,
) -> Result<BTreeMap<ParamName, super::super::install::ParamDef>, Box<rhai::EvalAltResult>> {
    match options
        .get("params")
        .and_then(|v| v.read_lock::<Map>().map(|m| m.clone()))
    {
        Some(params_map) => super::install::parse_param_defs(&params_map),
        None => Ok(BTreeMap::new()),
    }
}
