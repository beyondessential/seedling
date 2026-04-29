use std::collections::BTreeMap;

use rhai::{EvalAltResult, FnPtr, Map, TypeBuilder};
use seedling_protocol::names::{ActionName, ParamName};

use super::super::action::{Action, ActionDef};
use super::App;
use crate::runtime::barrier::runtime::is_in_action_closure;

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

    // l[impl action.lookup]
    builder.with_fn(
        "action",
        |this: &mut App, name: &str| -> Result<Action, Box<EvalAltResult>> {
            // The lookup must run inside an action body; it has no
            // meaningful semantics in the static context (the call
            // table that backs `.call()` only exists for the duration
            // of an action's invocation).
            if !is_in_action_closure() {
                return Err("app.action() may only be called inside an action closure".into());
            }
            let action_name = ActionName::new(name)
                .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
            let def = this.def.load();
            if !def.actions.contains_key(&action_name) {
                return Err(format!("no such action: {name:?}").into());
            }
            Ok(Action::new(action_name, def.name.clone()))
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
        Some(params_map) => super::install::parse_param_defs(&params_map, true),
        None => Ok(BTreeMap::new()),
    }
}
