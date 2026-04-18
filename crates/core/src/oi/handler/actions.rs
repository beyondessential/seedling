use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use seedling_protocol::error::{ErrorCode, OiError};

use crate::{
    oi::state::OiState,
    runtime::{
        AppPhase,
        scheduler::{RejectReason, ScheduleResult},
    },
};

use super::HandlerResult;

pub(crate) mod install;
pub mod lifecycle;

use lifecycle::spawn_accepted_operation;

#[derive(Deserialize)]
pub(crate) struct InvokeActionParams {
    pub app: String,
    pub name: String,
    #[serde(default)]
    pub params: Option<serde_json::Map<String, serde_json::Value>>,
}

// l[impl action.params]
fn validate_action_params(
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), OiError> {
    for key in params.keys() {
        if key.ends_with("_volume") {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!("param key {key:?} is reserved (keys ending in _volume are reserved)"),
            ));
        }
    }
    Ok(())
}

// i[action.not-installed-gate]
// i[action.invoke]
pub(crate) fn invoke_action(state: &Arc<OiState>, params: InvokeActionParams) -> HandlerResult {
    let app_name = &params.app;
    let action_name = &params.name;
    let action_params = params.params.unwrap_or_default();
    validate_action_params(&action_params)?;

    {
        let reg = state.registry.read();
        let entry = reg
            .get(app_name)
            .ok_or_else(|| OiError::not_found(format!("app not found: {app_name}")))?;

        // i[action.not-installed-gate]
        if !matches!(*entry.phase.lock(), AppPhase::Installed) {
            return Err(OiError::new(
                ErrorCode::NotInstalled,
                format!("app is not installed: {app_name}"),
            ));
        }

        let def = entry.app.def.lock();
        if def.shells.contains_key(action_name) {
            return Err(OiError::not_found(format!(
                "'{action_name}' is a shell action; use /shells/start"
            )));
        }
        if !def.actions.contains_key(action_name) {
            return Err(OiError::not_found(format!(
                "action not found: {action_name}"
            )));
        }
    }

    // Operator-invoked action: source and target generation are equal to the
    // app's current generation at dispatch.
    let current_generation = {
        let reg = state.registry.read();
        reg.get(app_name).map(|e| e.current_generation).unwrap_or(0)
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
                app_name.to_owned(),
                action_name.to_owned(),
                op_id,
                action_params,
                current_generation,
                current_generation,
                "operator".to_owned(),
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
