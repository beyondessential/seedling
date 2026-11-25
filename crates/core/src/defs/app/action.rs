use std::collections::BTreeMap;

use rhai::{EvalAltResult, FnPtr, Map, NativeCallContext, TypeBuilder};
use seedling_protocol::names::{ActionName, ParamName};

use super::super::action::{Action, ActionDef};
use super::super::stdlib::helper_limits;
use super::super::take::{reject_unknown_keys, take_closure, take_map};
use super::App;
use crate::runtime::barrier::runtime::is_in_action_closure;

/// Declare `action_name` on the app and return its handle.
// spec: ACN#action-declaration
fn declare_action(
    this: &mut App,
    method: &str,
    action_name: ActionName,
    closure: FnPtr,
    description: Option<String>,
    params: BTreeMap<ParamName, super::super::param_def::ParamDef>,
) -> Result<Action, Box<EvalAltResult>> {
    // spec: BSL#arguments
    let closure = take_closure(method, "fn", closure)?;
    super::ensure_not_validating()?;
    super::ensure_not_in_action_closure(method)?;
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
                declare_action(
                    this,
                    "on_action",
                    action_name,
                    closure,
                    None,
                    BTreeMap::new(),
                )
            },
        )
        .with_fn(
            "on_action",
            |ctx: NativeCallContext,
             this: &mut App,
             name: &str,
             closure: FnPtr,
             options: Map|
             -> Result<Action, Box<EvalAltResult>> {
                let action_name: ActionName = ActionName::new(name)
                    .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
                // spec: ACN#action-declaration
                reject_unknown_keys("on_action options", &options, &["description", "params"])?;
                let desc = super::extract_description(&options)?;
                // spec: ACN#param-schemas
                let params = parse_action_params(&ctx, &options)?;
                declare_action(this, "on_action", action_name, closure, desc, params)
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
}

// spec: ACN#param-schemas
pub(super) fn parse_action_params(
    ctx: &NativeCallContext,
    options: &Map,
) -> Result<BTreeMap<ParamName, super::super::param_def::ParamDef>, Box<rhai::EvalAltResult>> {
    match options.get("params").cloned() {
        Some(params) => super::param::parse_param_defs(
            &take_map("params", params)?,
            helper_limits(ctx)?.regex_compiled_size,
        ),
        None => Ok(BTreeMap::new()),
    }
}
