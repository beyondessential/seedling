use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use crate::{
    oi::{
        error::{ErrorCode, OiError},
        state::OiState,
    },
    runtime::{
        AppPhase,
        scheduler::{RejectReason, ScheduleResult},
    },
};

use super::HandlerResult;

pub(crate) mod install;
pub(crate) mod lifecycle;

use lifecycle::spawn_accepted_operation;

#[derive(Deserialize)]
pub(crate) struct InvokeActionParams {
    pub app: String,
    pub name: String,
}

// i[action.not-installed-gate]
// i[action.invoke]
pub(crate) fn invoke_action(state: &Arc<OiState>, params: InvokeActionParams) -> HandlerResult {
    let app_name = &params.app;
    let action_name = &params.name;

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

    let (result, op_id_opt) = {
        let mut sched = state.scheduler.lock();
        let result = sched.request(app_name, action_name, None);
        let op_id = if matches!(result, ScheduleResult::Accepted) {
            sched.active().map(|a| a.operation_id.clone())
        } else {
            None
        };
        (result, op_id)
    };

    match result {
        ScheduleResult::Accepted => {
            if let Some(op_id) = op_id_opt {
                spawn_accepted_operation(
                    Arc::clone(state),
                    app_name.to_owned(),
                    action_name.to_owned(),
                    op_id,
                    None,
                );
            }
            tracing::info!(app = %app_name, action = %action_name, schedule = "accepted", "invoke_action");
            Ok(json!({ "schedule": "accepted" }))
        }
        ScheduleResult::Queued => {
            tracing::info!(app = %app_name, action = %action_name, schedule = "queued", "invoke_action");
            Ok(json!({ "schedule": "queued" }))
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
