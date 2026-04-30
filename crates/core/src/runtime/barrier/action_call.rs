//! Implementation of `Action.invoke(params?)` — sub-action invocation.
//!
//! The Rhai surface lives in `crate::defs::action::Action`; this module
//! holds the orchestration: cycle detection, param validation,
//! `SubActionInvoked` log entry, and dispatch through the captured
//! [`FnPtr`]. The method is named `invoke` rather than `call` because
//! the Rhai engine intercepts `.call(...)` for function-pointer
//! dispatch before user-registered methods are consulted.
//!
//! # Spec
//! - l[impl action.call]
//! - r[impl operation.composition]
//! - r[impl operation.composition.cycles]
//! - r[impl operation.composition.params]
//! - r[impl history.action-log.entries]

use std::collections::BTreeMap;

use rhai::{Dynamic, EvalAltResult, FnPtr, Map, NativeCallContext, Position};
use seedling_protocol::names::{ActionName, ParamName, SiteVolumeName};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::defs::app::{SubActionFrame, action_call_lookup, action_call_stack};
use crate::defs::install::ParamDef;
use crate::runtime::action_params::{self, ParamValidationError, VolumeLookup};
use crate::runtime::barrier::runtime::{RuntimeInstance, active_rt, is_in_action_closure};
use crate::runtime::barrier::{ActionLogEntry, CallKind};

// l[impl action.call]
// r[impl operation.composition]
/// Entry point for the Rhai-registered `Action.call(...)` function.
///
/// Validates the supplied params against the action's declared schema,
/// rejects cycles in the active call stack, records a
/// `SubActionInvoked` log entry, and finally invokes the captured
/// closure via `FnPtr::call_within_context` with `(rt, params)` —
/// matching the arity operator-invoked actions get.
pub fn call_action(
    ctx: &NativeCallContext<'_>,
    action_name: &ActionName,
    params: Map,
) -> Result<(), Box<EvalAltResult>> {
    // l[impl action.call]
    if !is_in_action_closure() {
        return Err(
            "Action.invoke() may only be called inside an action closure"
                .to_string()
                .into(),
        );
    }

    // r[impl operation.composition.cycles]
    if let Some(stack) = action_call_stack()
        && stack.iter().any(|n| n == action_name)
    {
        return Err(cycle_error(action_name, &stack));
    }

    let fnptr: FnPtr = action_call_lookup(action_name)
        .map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?
        .ok_or_else(|| -> Box<EvalAltResult> {
            format!("Action.invoke: no such action {:?}", action_name.as_str()).into()
        })?;

    let rt = active_rt().ok_or_else(|| -> Box<EvalAltResult> {
        // Should be impossible while is_in_action_closure() is true,
        // but defend against drift in the lifecycle wiring.
        "Action.invoke: active runtime instance is not set"
            .to_string()
            .into()
    })?;

    // r[impl operation.composition.params]
    let validated_params = validate_call_params(&rt, action_name, params)?;

    // r[impl history.action-log.entries]
    record_subaction_entry(&rt, action_name, &validated_params)?;

    let _frame = SubActionFrame::enter(action_name.clone());

    let rt_dyn = Dynamic::from(rt);
    let params_dyn = json_to_rhai_map(&validated_params);

    let _result = fnptr.call_within_context::<Dynamic>(ctx, (rt_dyn, params_dyn))?;
    Ok(())
}

// r[impl operation.composition.params]
/// Run the same validation pipeline the OI invocation path uses, then
/// convert the result into a JSON map suitable for the log entry.
fn validate_call_params(
    rt: &RuntimeInstance,
    action_name: &ActionName,
    params: Map,
) -> Result<JsonMap<String, JsonValue>, Box<EvalAltResult>> {
    let mut json_params = rhai_map_to_json(params)?;
    action_params::reject_reserved_keys(&json_params).map_err(validation_to_eval)?;

    let schema = match action_schema(rt, action_name)? {
        Some(s) => s,
        None => return Ok(json_params),
    };

    action_params::apply_schema(&schema, &mut json_params).map_err(validation_to_eval)?;
    action_params::validate_volume_params(&schema, &json_params, &RtVolumeLookup(rt))
        .map_err(validation_to_eval)?;
    Ok(json_params)
}

/// Pull the action's param schema out of the runtime context's frozen
/// AppDef. Returns `None` if the action has no schema.
fn action_schema(
    rt: &RuntimeInstance,
    action_name: &ActionName,
) -> Result<Option<BTreeMap<ParamName, ParamDef>>, Box<EvalAltResult>> {
    let def_holder =
        crate::runtime::barrier::runtime::action_def().ok_or_else(|| -> Box<EvalAltResult> {
            "Action.invoke: action-def context is missing"
                .to_string()
                .into()
        })?;
    let _ = rt; // rt is reserved for future use (e.g. cross-app calls).
    let def = def_holder.load();
    Ok(def
        .actions
        .get(action_name.as_str())
        .map(|a| a.params.clone()))
}

// r[impl history.action-log.entries]
fn record_subaction_entry(
    rt: &RuntimeInstance,
    action_name: &ActionName,
    params: &JsonMap<String, JsonValue>,
) -> Result<(), Box<EvalAltResult>> {
    let Some(ctx) = rt.ctx.as_ref() else {
        // Stub / language-only context — nothing to record. Tests that
        // exercise the call surface without a real runtime context
        // still execute the closure inline.
        return Ok(());
    };

    let mut g = ctx.lock();
    if g.is_replaying() {
        // The committed entry at this call_index already describes
        // this sub-action invocation; advance past it without
        // emitting a duplicate. Match the order rt.start / rt.exec
        // / rt.signal use: check is_replaying *before* incrementing
        // so the boundary case (consuming the last committed entry)
        // doesn't flip into fresh mode and double-push.
        g.call_index += 1;
        return Ok(());
    }

    let idx = g.call_index;
    let payload = serde_json::json!({
        "action": action_name.as_str(),
        "params": params,
    })
    .to_string();
    g.pending.push(ActionLogEntry {
        call_index: idx,
        call_kind: CallKind::SubAction,
        resources: Vec::new(),
        barrier: None,
        extra: Some(payload),
    });
    g.call_index += 1;
    drop(g);
    crate::system::breadcrumb::Breadcrumb {
        app: Some(&seedling_protocol::names::AppName::new_unchecked(
            rt.app_name.as_str(),
        )),
        kind: crate::system::breadcrumb::BreadcrumbKind::SubAction {
            name: action_name,
            params,
        },
        script_pos: None,
    }
    .emit();
    Ok(())
}

fn cycle_error(action_name: &ActionName, stack: &[ActionName]) -> Box<EvalAltResult> {
    let chain = stack
        .iter()
        .map(|n| n.as_str().to_string())
        .collect::<Vec<_>>()
        .join(" → ");
    format!(
        "Action.invoke: cycle detected — {chain} → {} would re-enter an action already on the call stack",
        action_name.as_str()
    )
    .into()
}

fn validation_to_eval(e: ParamValidationError) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(e.to_string()),
        Position::NONE,
    ))
}

fn rhai_map_to_json(map: Map) -> Result<JsonMap<String, JsonValue>, Box<EvalAltResult>> {
    let mut out = JsonMap::new();
    for (k, v) in map {
        out.insert(k.to_string(), rhai_dynamic_to_json(v)?);
    }
    Ok(out)
}

fn rhai_dynamic_to_json(v: Dynamic) -> Result<JsonValue, Box<EvalAltResult>> {
    if v.is_unit() {
        Ok(JsonValue::Null)
    } else if v.is_bool() {
        Ok(JsonValue::Bool(v.cast::<bool>()))
    } else if v.is_int() {
        Ok(JsonValue::from(v.cast::<i64>()))
    } else if v.is_float() {
        Ok(serde_json::Number::from_f64(v.cast::<f64>())
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null))
    } else if v.is_string() {
        Ok(JsonValue::String(v.into_string().unwrap_or_default()))
    } else if v.is_array() {
        let arr: rhai::Array = v.cast();
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            out.push(rhai_dynamic_to_json(item)?);
        }
        Ok(JsonValue::Array(out))
    } else if v.is_map() {
        let map: Map = v.cast();
        Ok(JsonValue::Object(rhai_map_to_json(map)?))
    } else {
        Err(format!(
            "Action.invoke: param value is of unsupported type {}",
            v.type_name()
        )
        .into())
    }
}

fn json_to_rhai_map(map: &JsonMap<String, JsonValue>) -> Map {
    map.iter()
        .map(|(k, v)| {
            (
                k.as_str().into(),
                super::replay::json_value_to_rhai_dynamic(v),
            )
        })
        .collect()
}

// l[impl action.params.volume]
/// `RuntimeInstance`-backed `VolumeLookup` so script-side validation
/// resolves volume references against the same site-volume registry
/// the OI path uses.
struct RtVolumeLookup<'a>(&'a RuntimeInstance);

impl VolumeLookup for RtVolumeLookup<'_> {
    fn site_volume_exists(&self, name: &SiteVolumeName) -> bool {
        let Some(db) = self.0.db.as_ref() else {
            // Language-only / stub context — let validation pass; tests
            // that exercise volume-binding behaviour configure a real
            // db handle.
            return true;
        };
        let name = name.clone();
        db.call(move |db| crate::runtime::site_volumes::get(db, &name).ok().flatten())
            .is_some()
    }
}
