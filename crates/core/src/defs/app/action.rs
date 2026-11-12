use std::collections::BTreeMap;

use rhai::{EvalAltResult, FnPtr, Map, TypeBuilder};
use seedling_protocol::names::{ActionName, ParamName};

use super::super::action::{Action, ActionDef};
use super::App;
use crate::runtime::barrier::runtime::is_in_action_closure;

/// Declare `action_name` on the app and return its handle.
// spec: ACN#action-declaration
fn declare_action(
    this: &mut App,
    action_name: ActionName,
    closure: FnPtr,
    description: Option<String>,
    params: BTreeMap<ParamName, super::super::install::ParamDef>,
) -> Result<Action, Box<EvalAltResult>> {
    if action_name == "install" {
        return Err(
            "the action name \"install\" is reserved for the install action; declare it with \
             app.on_install"
                .into(),
        );
    }
    if this.def.load().actions.contains_key(&action_name) {
        return Err(format!(
            "an action named {:?} is already declared",
            action_name.as_str()
        )
        .into());
    }
    let app_name = this.def.load().name.clone();
    this.def.rcu(|d| {
        let mut d = (**d).clone();
        d.actions.insert(
            action_name.clone(),
            ActionDef {
                name: action_name.clone(),
                description: description.clone(),
                schedules: Vec::new(),
                params: params.clone(),
            },
        );
        d
    });
    super::capture_action(action_name.clone(), closure);
    Ok(Action::new(action_name, app_name))
}

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // spec: ACN#action-declaration
    builder
        .with_fn(
            "on_action",
            |this: &mut App, name: &str, closure: FnPtr| -> Result<Action, Box<EvalAltResult>> {
                let action_name: ActionName = ActionName::new(name)
                    .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
                declare_action(this, action_name, closure, None, BTreeMap::new())
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
                let desc = super::extract_description(&options)?;
                // spec: ACN#param-schemas
                let params = parse_action_params(&options)?;
                declare_action(this, action_name, closure, desc, params)
            },
        );

    // spec: ACN#action-handles
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

    // spec: ACN#start-action
    builder
        .with_fn(
            "on_start",
            |this: &mut App, closure: FnPtr| -> Result<Action, Box<EvalAltResult>> {
                let start_name = ActionName::new_unchecked("start");
                declare_action(this, start_name, closure, None, BTreeMap::new())
            },
        )
        .with_fn(
            "on_start",
            |this: &mut App, closure: FnPtr, options: Map| -> Result<Action, Box<EvalAltResult>> {
                let desc = super::extract_description(&options)?;
                let start_name = ActionName::new_unchecked("start");
                declare_action(this, start_name, closure, desc, BTreeMap::new())
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
