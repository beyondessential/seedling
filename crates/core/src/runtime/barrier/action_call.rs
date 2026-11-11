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
//! - spec: ACN#action-invocation
//! - spec: OPS#action-composition
//! - spec: OPS#cycles
//! - spec: OPS#sub-action-parameters
//! - spec: HIST#action-calls

use std::collections::BTreeMap;

use rhai::{Dynamic, EvalAltResult, FnPtr, Map, NativeCallContext, Position};
use seedling_protocol::names::{ActionName, ParamName, SiteVolumeName};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::defs::app::{SubActionFrame, action_call_lookup, action_call_stack};
use crate::defs::install::ParamDef;
use crate::runtime::action_params::{self, ParamValidationError, VolumeLookup};
use crate::runtime::barrier::runtime::{RuntimeInstance, active_rt, is_in_action_closure};
use crate::runtime::barrier::{ActionLogEntry, CallKind, ReplayMismatch};

// spec: ACN#action-invocation
// spec: OPS#action-composition
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
    // spec: ACN#action-invocation
    if !is_in_action_closure() {
        return Err(
            "Action.invoke() may only be called inside an action closure"
                .to_string()
                .into(),
        );
    }

    // spec: OPS#cycles
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

    // spec: OPS#sub-action-parameters
    // spec: HIST#action-calls
    let call_params = resolve_call_params(&rt, action_name, params)?;

    let _frame = SubActionFrame::enter(action_name.clone());

    let rt_dyn = Dynamic::from(rt);
    let params_dyn = json_to_rhai_map(&call_params);

    let _result = fnptr.call_within_context::<Dynamic>(ctx, (rt_dyn, params_dyn))?;
    Ok(())
}

// spec: OPS#sub-action-parameters
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

// spec: OPS#sub-action-parameters
// spec: HIST#action-calls
/// The params the called closure runs with, and the log entry recording the
/// call.
///
/// A call already in the committed log takes its params from that entry
/// rather than validating the supplied map again. Validation resolves
/// `kind: "volume"` params against the site-volume registry and applies the
/// action's current schema, so it answers to the world rather than to the
/// script alone: deleting the referenced site volume, or updating the
/// sub-action's schema, makes it fail on the next pass. The call was
/// already committed with valid params by then, and the payload exists so
/// that it can replay with the params it was committed with — which is also
/// what `OPS#sub-action-parameters` requires the closure to observe.
fn resolve_call_params(
    rt: &RuntimeInstance,
    action_name: &ActionName,
    params: Map,
) -> Result<JsonMap<String, JsonValue>, Box<EvalAltResult>> {
    let Some(ctx) = rt.ctx.as_ref() else {
        // Stub / language-only context — nothing to record and nothing to
        // recover. Tests that exercise the call surface without a real
        // runtime context still execute the closure inline.
        return validate_call_params(rt, action_name, params);
    };

    // spec: OPS#sub-action-parameters
    // spec: SUSP#positional-replay
    // The recorded action name is a literal in the script, so unlike the
    // params it must match the log: a script edit that changes which
    // sub-action is called at this position is a divergence, not a replay.
    {
        let mut g = ctx.lock();
        match g.replay_step(CallKind::SubAction, None) {
            Ok(Some(entry)) => {
                drop(g);
                return recover_recorded_params(action_name, &entry);
            }
            Ok(None) => {}
            Err(mismatch) => return Err(Box::<EvalAltResult>::from(mismatch.to_string())),
        }
    }

    let validated = validate_call_params(rt, action_name, params)?;

    let mut g = ctx.lock();
    let idx = g.call_index;
    let payload = serde_json::json!({
        "action": action_name.as_str(),
        "params": &validated,
    })
    .to_string();
    g.pending.push(ActionLogEntry {
        call_index: idx,
        call_kind: CallKind::SubAction,
        resources: Vec::new(),
        barriers: Vec::new(),
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
            params: &validated,
        },
        script_pos: None,
    }
    .emit();
    Ok(validated)
}

// spec: OPS#sub-action-parameters
/// Read the called action's name and its post-validation params back out of
/// a committed `SubAction` entry.
fn recover_recorded_params(
    action_name: &ActionName,
    entry: &ActionLogEntry,
) -> Result<JsonMap<String, JsonValue>, Box<EvalAltResult>> {
    let payload: JsonValue = entry
        .extra
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| -> Box<EvalAltResult> {
            format!(
                "replay diverged at call {}: the log records a sub-action call whose \
                 payload cannot be read ({e})",
                entry.call_index
            )
            .into()
        })?
        .ok_or_else(|| -> Box<EvalAltResult> {
            // An entry with no payload cannot say what it called or with
            // what, so there is nothing to replay and nothing that could
            // stand in for it.
            format!(
                "replay diverged at call {}: the log records a sub-action call with no \
                 recorded action or params",
                entry.call_index
            )
            .into()
        })?;

    let recorded_action = payload.get("action").and_then(JsonValue::as_str);
    if recorded_action != Some(action_name.as_str()) {
        return Err(ReplayMismatch::Extra {
            call_index: entry.call_index,
            kind: CallKind::SubAction,
            expected: action_name.as_str().to_owned(),
            found: recorded_action.map(str::to_owned),
        }
        .to_string()
        .into());
    }

    match payload.get("params") {
        Some(JsonValue::Object(params)) => Ok(params.clone()),
        // A call committed with no params is a call with no params.
        None | Some(JsonValue::Null) => Ok(JsonMap::new()),
        Some(other) => Err(format!(
            "replay diverged at call {}: the log records sub-action params that are not a \
             map but a {}",
            entry.call_index,
            match other {
                JsonValue::Array(_) => "list",
                JsonValue::String(_) => "string",
                JsonValue::Bool(_) => "boolean",
                _ => "number",
            }
        )
        .into()),
    }
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

// spec: ACN#volume-params
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
