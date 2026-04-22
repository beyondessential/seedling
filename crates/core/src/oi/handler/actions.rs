use std::sync::Arc;

use seedling_protocol::error::{ErrorCode, OiError};
use seedling_protocol::names::{ActionName, AppName};
use serde::Deserialize;
use serde_json::json;

use self::install::validate_requirements;
use self::lifecycle::spawn_accepted_operation;
use super::HandlerResult;
use crate::{
    oi::{handler::RequestCtx, state::OiState},
    runtime::{
        AppPhase,
        scheduler::{CancelOutcome, RejectReason, ScheduleResult},
    },
};

pub(crate) mod install;
pub mod lifecycle;

#[derive(Deserialize)]
pub(crate) struct InvokeActionParams {
    pub app: AppName,
    pub name: ActionName,
    #[serde(default)]
    pub params: Option<serde_json::Map<String, serde_json::Value>>,
}

// i[action.cancel]
#[derive(Deserialize)]
pub(crate) struct CancelActionParams {
    pub app: AppName,
}

/// Request cancellation of the currently-active operation for `app`.
// r[impl operation.cancel]
// i[action.cancel]
pub(crate) fn cancel_action(state: &Arc<OiState>, params: CancelActionParams) -> HandlerResult {
    let app_name = &params.app;
    let outcome = state.scheduler.lock().request_cancel(app_name);
    match outcome {
        CancelOutcome::Cancelled(op_id) | CancelOutcome::AlreadyCancelled(op_id) => {
            // r[impl operation.cancel.persistence]
            // Persist the flag so a daemon crash between the in-memory flip
            // and cancel observation leaves the op in a pre-cancelled state
            // on the next start-up. Do this on AlreadyCancelled too so a
            // prior persist failure heals on retry.
            let op_id_for_persist = op_id.clone();
            let app_for_log = app_name.clone();
            state.db.call(move |db| {
                if let Err(e) =
                    crate::runtime::history::set_cancel_requested(db, &op_id_for_persist)
                {
                    tracing::warn!(
                        app = %app_for_log,
                        operation_id = %op_id_for_persist.0,
                        "failed to persist cancel_requested: {e}"
                    );
                }
            });
            tracing::info!(app = %app_name, operation_id = %op_id.0, "operation cancel requested");
            Ok(json!({ "cancelled": true }))
        }
        CancelOutcome::NoActiveOp => Err(OiError::not_found(format!(
            "no active operation to cancel for app: {app_name}"
        ))),
    }
}

// l[impl action.params]
// r[impl operation.volume-param.reserved]
fn validate_action_params(
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), OiError> {
    for key in params.keys() {
        if key.ends_with("_volume") || key.ends_with("_filename") {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "param key {key:?} is reserved (keys ending in _volume or _filename are reserved)"
                ),
            ));
        }
    }
    Ok(())
}

// i[action.invoke]
fn apply_action_param_schema(
    action_params_schema: &std::collections::BTreeMap<
        seedling_protocol::names::ParamName,
        crate::defs::install::ParamDef,
    >,
    params: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<(), OiError> {
    if action_params_schema.is_empty() {
        return Ok(());
    }

    let submitted: std::collections::BTreeMap<String, String> = action_params_schema
        .keys()
        .filter_map(|k| {
            params
                .get(k.as_str())
                .and_then(|v| v.as_str())
                .map(|s| (k.as_str().to_owned(), s.to_owned()))
        })
        .collect();

    let filled = validate_requirements(action_params_schema, &submitted)?;

    for (k, v) in filled {
        params.insert(k, serde_json::Value::String(v));
    }

    Ok(())
}

// i[action.not-installed-gate]
// i[action.invoke]
pub(crate) fn invoke_action(
    state: &Arc<OiState>,
    params: InvokeActionParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let app_name = &params.app;
    let action_name = &params.name;
    let mut action_params = params.params.unwrap_or_default();
    validate_action_params(&action_params)?;

    {
        let reg = state.registry.read();
        let entry = reg
            .get(app_name.as_str())
            .ok_or_else(|| OiError::not_found(format!("app not found: {app_name}")))?;

        // i[action.not-installed-gate]
        if !matches!(*entry.phase.lock(), AppPhase::Installed) {
            return Err(OiError::new(
                ErrorCode::NotInstalled,
                format!("app is not installed: {app_name}"),
            ));
        }

        // i[action.invoke] - reject if script_error fault is active
        if entry.script_error.is_some() {
            return Err(OiError::script_error(format!(
                "app has a script error: {app_name}"
            )));
        }

        let def = entry.app.def.load();
        if def.shells.contains_key(action_name.as_str()) {
            return Err(OiError::not_found(format!(
                "'{action_name}' is a shell action; use /shells/start"
            )));
        }
        // l[impl action.start.no-manual-invoke]
        if action_name == "start" {
            return Err(OiError::not_found(
                "'start' is a lifecycle action and cannot be manually invoked".to_string(),
            ));
        }
        let action_def = def
            .actions
            .get(action_name.as_str())
            .ok_or_else(|| OiError::not_found(format!("action not found: {action_name}")))?;

        // i[action.invoke]
        apply_action_param_schema(&action_def.params, &mut action_params)?;
    }

    // Operator-invoked action: source and target generation are equal to the
    // app's current generation at dispatch.
    let current_generation = {
        let reg = state.registry.read();
        reg.get(app_name.as_str())
            .map(|e| e.current_generation)
            .unwrap_or(0)
    };
    let (result, op_id_str) = {
        let mut sched = state.scheduler.lock();
        let result = sched.request(
            app_name,
            action_name,
            action_params.clone(),
            current_generation,
            current_generation,
            "operator",
        );
        let op_id = match &result {
            ScheduleResult::Accepted => sched.active().map(|a| a.operation_id.clone()),
            ScheduleResult::Queued => sched
                .queue_iter()
                .find(|q| q.app == *app_name)
                .map(|q| q.operation_id.clone()),
            ScheduleResult::Rejected(_) => None,
        };
        (result, op_id.map(|id| id.0.clone()).unwrap_or_default())
    };

    match result {
        ScheduleResult::Accepted => {
            let op_id = crate::runtime::barrier::OperationId(op_id_str.clone());
            spawn_accepted_operation(
                Arc::clone(state),
                app_name.clone(),
                action_name.clone(),
                op_id,
                action_params,
                current_generation,
                current_generation,
                "operator".to_owned(),
                Some(Arc::clone(&ctx.events.actor)),
            );
            tracing::info!(app = %app_name, action = %action_name, schedule = "accepted", "invoke_action");
            Ok(json!({ "schedule": "accepted", "operation_id": op_id_str }))
        }
        ScheduleResult::Queued => {
            tracing::info!(app = %app_name, action = %action_name, schedule = "queued", "invoke_action");
            Ok(json!({ "schedule": "queued", "operation_id": op_id_str }))
        }
        ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress) => Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {app_name}"),
        )),
        ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued) => Err(OiError::new(
            ErrorCode::AlreadyQueued,
            format!("already queued for app: {app_name}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // l[impl action.params] r[impl operation.volume-param.reserved]
    #[test]
    fn rejects_reserved_volume_suffix() {
        let mut params = serde_json::Map::new();
        params.insert("source_volume".into(), json!("anything"));
        let err = validate_action_params(&params).unwrap_err();
        assert!(matches!(err.code, ErrorCode::RequirementsInvalid));
    }

    // l[impl action.params] r[impl operation.volume-param.reserved]
    #[test]
    fn rejects_reserved_filename_suffix() {
        let mut params = serde_json::Map::new();
        params.insert("output_filename".into(), json!("anything"));
        let err = validate_action_params(&params).unwrap_err();
        assert!(matches!(err.code, ErrorCode::RequirementsInvalid));
    }

    #[test]
    fn accepts_unreserved_keys() {
        let mut params = serde_json::Map::new();
        params.insert("volume".into(), json!("ok"));
        params.insert("filename".into(), json!("ok"));
        params.insert("source".into(), json!("ok"));
        params.insert("backup".into(), json!({ "strategy": "s" }));
        validate_action_params(&params).unwrap();
    }
}
