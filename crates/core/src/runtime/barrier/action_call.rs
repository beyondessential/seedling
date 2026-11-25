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

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rhai::{Dynamic, EvalAltResult, FnPtr, Map, NativeCallContext, Position};
use secrecy::{ExposeSecret, SecretString};
use seedling_protocol::names::{ActionName, ParamName, SiteVolumeName};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::defs::app::{SubActionFrame, action_call_lookup, action_call_stack};
use crate::defs::param_def::ParamDef;
use crate::defs::regex::{Budgets, with_evaluation_budget};
use crate::runtime::action_params::{self, ParamValidationError, VolumeLookup};
use crate::runtime::barrier::runtime::{RuntimeInstance, active_rt, is_in_action_closure};
use crate::runtime::barrier::{ActionLogEntry, CallKind, ReplayMismatch};
use crate::runtime::param_map::ParamMap;
use crate::runtime::redact::{REDACTED, SensitiveValues};
use crate::runtime::secrets::Cipher;

type Schema = BTreeMap<ParamName, ParamDef>;

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
    // spec: ACN#action-invocation
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
    // spec: REX#compiled-size
    let call_params = with_evaluation_budget(ctx, |budget| {
        resolve_call_params(&rt, action_name, params, &mut Budgets::Shared(budget))
    })??;

    let _frame = SubActionFrame::enter(action_name.clone());

    let rt_dyn = Dynamic::from(rt);
    let params_dyn = call_params
        .to_rhai()
        .map_err(|e| -> Box<EvalAltResult> { format!("Action.invoke: {e}").into() })?;

    let _result = fnptr.call_within_context::<Dynamic>(ctx, (rt_dyn, params_dyn))?;
    Ok(())
}

// spec: OPS#sub-action-parameters
/// Run the same validation pipeline the OI invocation path uses, then
/// convert the result into a JSON map suitable for the log entry.
fn validate_call_params(
    rt: &RuntimeInstance,
    schema: Option<&Schema>,
    params: Map,
    budgets: &mut Budgets,
) -> Result<ParamMap, Box<EvalAltResult>> {
    let json_params = crate::defs::json::from_map(params)
        .map_err(|e| -> Box<EvalAltResult> { format!("Action.invoke: param {e}").into() })?;
    action_params::reject_reserved_keys(&json_params).map_err(validation_to_eval)?;

    let Some(schema) = schema else {
        return Ok(ParamMap::from_values(json_params));
    };

    let params =
        action_params::apply_schema(schema, json_params, budgets).map_err(validation_to_eval)?;
    action_params::validate_volume_params(schema, &params, &RtVolumeLookup(rt))
        .map_err(validation_to_eval)?;
    Ok(params)
}

/// Pull the action's param schema out of the runtime context's frozen
/// AppDef. Returns `None` if the action has no schema.
fn action_schema(
    rt: &RuntimeInstance,
    action_name: &ActionName,
) -> Result<Option<Schema>, Box<EvalAltResult>> {
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
///
/// The entry holds secret params encrypted under the secret key, apart from
/// the others, and the journal breadcrumb shows them redacted.
fn resolve_call_params(
    rt: &RuntimeInstance,
    action_name: &ActionName,
    params: Map,
    budgets: &mut Budgets,
) -> Result<ParamMap, Box<EvalAltResult>> {
    let schema = action_schema(rt, action_name)?;
    let Some(ctx) = rt.ctx.as_ref() else {
        // Stub / language-only context — nothing to record and nothing to
        // recover. Tests that exercise the call surface without a real
        // runtime context still execute the closure inline.
        return validate_call_params(rt, schema.as_ref(), params, budgets);
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
                let cipher = g.cipher.clone();
                drop(g);
                let params = recover_recorded_params(action_name, &entry, cipher.as_deref())?;
                hold_secret_params(schema.as_ref(), params.values());
                return Ok(params);
            }
            Ok(None) => {}
            Err(mismatch) => return Err(Box::<EvalAltResult>::from(mismatch.to_string())),
        }
    }

    let validated = validate_call_params(rt, schema.as_ref(), params, budgets)?;
    hold_secret_params(schema.as_ref(), validated.values());

    let SplitParams {
        plain,
        secret,
        journal,
    } = split_secret_params(
        schema.as_ref(),
        &validated,
        &crate::runtime::seeds::operation_scope()
            .map(|scope| scope.values())
            .unwrap_or_default(),
    );

    let mut g = ctx.lock();
    let idx = g.call_index;
    let mut payload = serde_json::json!({
        "action": action_name.as_str(),
        "params": &plain,
    });
    // spec: OPS#persistence
    if !validated.types().is_empty() {
        payload["types"] =
            serde_json::to_value(validated.types()).expect("parsed types serialise as strings");
    }
    if !secret.is_empty() {
        let cipher = g.cipher.clone().ok_or_else(|| -> Box<EvalAltResult> {
            "Action.invoke: no secret key to protect the call's secret params"
                .to_string()
                .into()
        })?;
        payload["secret_params"] = JsonValue::String(seal_secret_params(&cipher, &secret)?);
    }
    let payload = payload.to_string();
    g.pending.push(ActionLogEntry {
        call_index: idx,
        call_kind: CallKind::SubAction,
        resources: Vec::new(),
        barriers: Vec::new(),
        extra: Some(payload),
        begun: false,
    });
    g.call_index += 1;
    drop(g);
    crate::system::breadcrumb::Breadcrumb {
        app: Some(&seedling_protocol::names::AppName::new_unchecked(
            rt.app_name.as_str(),
        )),
        kind: crate::system::breadcrumb::BreadcrumbKind::SubAction {
            name: action_name,
            params: &journal,
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
    cipher: Option<&Cipher>,
) -> Result<ParamMap, Box<EvalAltResult>> {
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

    let mut params = match payload.get("params") {
        Some(JsonValue::Object(params)) => params.clone(),
        // A call committed with no params is a call with no params.
        None | Some(JsonValue::Null) => JsonMap::new(),
        Some(other) => {
            return Err(format!(
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
            .into());
        }
    };

    // spec: SEC#storage
    match payload.get("secret_params") {
        None => {}
        Some(JsonValue::String(sealed)) => {
            let cipher = cipher.ok_or_else(|| -> Box<EvalAltResult> {
                format!(
                    "replay diverged at call {}: the log records secret sub-action params and \
                     there is no secret key to read them",
                    entry.call_index
                )
                .into()
            })?;
            params.extend(open_secret_params(cipher, sealed).map_err(
                |e| -> Box<EvalAltResult> {
                    format!(
                        "replay diverged at call {}: the log records secret sub-action params \
                         that cannot be read ({e})",
                        entry.call_index
                    )
                    .into()
                },
            )?);
        }
        Some(_) => {
            return Err(format!(
                "replay diverged at call {}: the log records secret sub-action params that are \
                 not sealed",
                entry.call_index
            )
            .into());
        }
    }

    // spec: OPS#persistence
    let types = payload
        .get("types")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| -> Box<EvalAltResult> {
            format!(
                "replay diverged at call {}: the log records typed params that cannot be \
                 read ({e})",
                entry.call_index
            )
            .into()
        })?
        .unwrap_or_default();
    Ok(ParamMap::with_types(params, types))
}

// spec: SEC#matching
/// Hold the values of the called action's secret params, supplied or
/// defaulted, as sensitive values of the running operation.
fn hold_secret_params(schema: Option<&Schema>, params: &JsonMap<String, JsonValue>) {
    let (Some(schema), Some(scope)) = (schema, crate::runtime::seeds::operation_scope()) else {
        return;
    };
    scope.insert_secret_params(params, |name| {
        schema.get(name).is_some_and(ParamDef::is_secret)
    });
}

/// A call's validated params, divided for recording.
struct SplitParams {
    /// The params recorded readable in the log entry.
    plain: JsonMap<String, JsonValue>,
    /// The params recorded encrypted: the secret ones, and those whose value
    /// contains a sensitive value.
    secret: JsonMap<String, JsonValue>,
    /// Every param, with each secret one's value redacted and each sensitive
    /// value another holds masked, for the journal.
    journal: JsonMap<String, JsonValue>,
}

// spec: SEC#storage
// spec: SEC#redaction
// spec: HIST#action-calls
fn split_secret_params(
    schema: Option<&Schema>,
    validated: &ParamMap,
    sensitive: &SensitiveValues,
) -> SplitParams {
    let mut plain = JsonMap::new();
    let mut secret = JsonMap::new();
    let mut journal = JsonMap::new();
    for (name, value) in validated.values() {
        let is_secret = schema
            .and_then(|schema| schema.get(name.as_str()))
            .is_some_and(ParamDef::is_secret);
        if is_secret {
            secret.insert(name.clone(), value.clone());
            journal.insert(name.clone(), JsonValue::from(REDACTED));
        } else if sensitive.found_in_json(value) {
            secret.insert(name.clone(), value.clone());
            journal.insert(name.clone(), sensitive.mask_json(value));
        } else {
            plain.insert(name.clone(), value.clone());
            journal.insert(name.clone(), value.clone());
        }
    }
    SplitParams {
        plain,
        secret,
        journal,
    }
}

// spec: SEC#storage
fn seal_secret_params(
    cipher: &Cipher,
    secret: &JsonMap<String, JsonValue>,
) -> Result<String, Box<EvalAltResult>> {
    let plaintext = SecretString::new(JsonValue::Object(secret.clone()).to_string().into());
    let sealed = cipher
        .encrypt(&plaintext)
        .map_err(|e| -> Box<EvalAltResult> {
            format!("Action.invoke: could not protect the call's secret params: {e}").into()
        })?;
    Ok(BASE64.encode(sealed))
}

fn open_secret_params(cipher: &Cipher, sealed: &str) -> Result<JsonMap<String, JsonValue>, String> {
    let ciphertext = BASE64.decode(sealed).map_err(|e| e.to_string())?;
    let plaintext = cipher.decrypt(&ciphertext).map_err(|e| e.to_string())?;
    match serde_json::from_str(plaintext.expose_secret()).map_err(|e| e.to_string())? {
        JsonValue::Object(map) => Ok(map),
        _ => Err("not a map".to_owned()),
    }
}

// spec: ACN#action-invocation
fn cycle_error(action_name: &ActionName, stack: &[ActionName]) -> Box<EvalAltResult> {
    let start = stack.iter().position(|n| n == action_name).unwrap_or(0);
    let chain = stack[start..]
        .iter()
        .chain(std::iter::once(action_name))
        .map(ActionName::as_str)
        .collect::<Vec<_>>()
        .join(" → ");
    format!(
        "Action.invoke: cycle detected — {chain} would re-enter an action already on the call stack"
    )
    .into()
}

fn validation_to_eval(e: ParamValidationError) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(e.to_string()),
        Position::NONE,
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    // spec: ACN#action-invocation
    #[test]
    fn cycle_error_names_only_the_actions_that_form_the_cycle() {
        let names = |list: &[&str]| -> Vec<ActionName> {
            list.iter().map(|n| ActionName::new_unchecked(*n)).collect()
        };
        let stack = names(&["outer", "alpha", "bravo"]);
        let message = cycle_error(&ActionName::new_unchecked("alpha"), &stack).to_string();
        assert!(message.contains("alpha → bravo → alpha"), "{message}");
        assert!(!message.contains("outer"), "{message}");

        let stack = names(&["loop"]);
        let message = cycle_error(&ActionName::new_unchecked("loop"), &stack).to_string();
        assert!(message.contains("loop → loop"), "{message}");
    }

    // spec: SEC#redaction
    #[test]
    fn journal_shows_secret_params_redacted() {
        let def = |secret| ParamDef {
            kind: crate::defs::param_def::ParamKind::Text,
            required: false,
            default_value: None,
            description: None,
            secret,
        };
        let mut schema = Schema::new();
        schema.insert(ParamName::new_unchecked("token"), def(Some(true)));
        schema.insert(ParamName::new_unchecked("label"), def(None));
        let validated: JsonMap<String, JsonValue> = serde_json::from_value(serde_json::json!({
            "token": "s3cret-value",
            "label": "plain-label",
        }))
        .unwrap();

        let split = split_secret_params(
            Some(&schema),
            &ParamMap::from_values(validated),
            &SensitiveValues::default(),
        );
        assert_eq!(split.journal["token"], REDACTED);
        assert_eq!(split.journal["label"], "plain-label");
        assert_eq!(split.secret["token"], "s3cret-value");
        assert!(!split.plain.contains_key("token"));
    }

    // spec: HIST#action-calls
    #[test]
    fn a_plain_param_holding_a_sensitive_value_is_sealed_and_masked() {
        let validated: JsonMap<String, JsonValue> = serde_json::from_value(serde_json::json!({
            "url": "postgres://app:hunter2@db/app",
            "port": 5432,
            "label": "plain-label",
        }))
        .unwrap();
        let mut sensitive = SensitiveValues::default();
        sensitive.insert("hunter2");
        sensitive.insert("5432");

        let split = split_secret_params(None, &ParamMap::from_values(validated), &sensitive);
        assert_eq!(split.secret["url"], "postgres://app:hunter2@db/app");
        assert_eq!(split.secret["port"], 5432);
        assert_eq!(split.journal["url"], "postgres://app:<redacted>@db/app");
        assert_eq!(split.journal["port"], REDACTED);
        assert_eq!(split.plain.keys().collect::<Vec<_>>(), ["label"]);
        assert_eq!(split.journal["label"], "plain-label");
    }

    // spec: HIST#action-calls
    #[test]
    fn a_nested_value_its_json_text_would_escape_is_sealed() {
        let validated: JsonMap<String, JsonValue> = serde_json::from_value(serde_json::json!({
            "conn": { "user": "app", "password": r#"Tok"en\99"# },
        }))
        .unwrap();
        let mut sensitive = SensitiveValues::default();
        sensitive.insert(r#"Tok"en\99"#);

        let split = split_secret_params(None, &ParamMap::from_values(validated), &sensitive);
        assert!(split.plain.is_empty(), "the param is not recorded readable");
        assert_eq!(split.secret["conn"]["password"], r#"Tok"en\99"#);
        assert_eq!(
            split.journal["conn"],
            serde_json::json!({ "user": "app", "password": REDACTED })
        );
    }
}
