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

#[derive(Deserialize)]
pub(crate) struct SetParamParams {
    pub app: String,
    pub name: String,
    pub value: String,
}

#[derive(Deserialize)]
pub(crate) struct UnsetParamParams {
    pub app: String,
    pub name: String,
}

// i[param.store]
// i[param.set]
// i[param.unknown]
pub(crate) fn set_param(state: &OiState, params: SetParamParams) -> HandlerResult {
    let app = params.app.as_str();
    let param_name = params.name.as_str();
    let value = params.value.as_str();

    {
        let reg = state.registry.read();
        if !reg.is_registered(app) {
            return Err(OiError::not_found(format!("app not found: {app}")));
        }
    }

    {
        let db = state.db.lock();
        crate::runtime::apps::upsert_param(&db, app, param_name, value)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    }

    let script = {
        let reg = state.registry.read();
        reg.get(app).expect("confirmed registered").script.clone()
    };
    let loaded_params = {
        let db = state.db.lock();
        crate::runtime::apps::load_params_for_app(&db, app)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?
    };
    state
        .registry
        .write()
        .reload(app, script, &loaded_params, &state.script_limits);

    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(app) {
            let db = state.db.lock();
            crate::runtime::apps::sync_script_error_fault(&db, entry);
            crate::runtime::apps::sync_registry_faults(&db, entry);
        }
    }

    let (has_on_change, is_installed, tick_notify) = {
        let reg = state.registry.read();
        let entry = reg.get(app).expect("confirmed registered");
        let has = entry.app.def.lock().param_changes.contains(param_name);
        let installed = matches!(
            *entry.phase.lock(),
            AppPhase::Installed | AppPhase::Uninstalling
        );
        let notify = Arc::clone(&entry.tick_notify);
        (has, installed, notify)
    };

    // Only schedule the on_change handler when the app is installed — there is
    // nothing running to respond to the change before that. The stored value
    // takes effect automatically when the app is next evaluated (e.g. at install).
    if has_on_change && is_installed {
        let result = state.scheduler.lock().request(app, param_name, None);
        match result {
            ScheduleResult::Accepted => {
                tracing::info!(app = %app, param = %param_name, schedule = "accepted", "set_param");
                tick_notify.notify_one();
                Ok(json!({ "schedule": "accepted" }))
            }
            ScheduleResult::Queued => {
                tracing::info!(app = %app, param = %param_name, schedule = "queued", "set_param");
                tick_notify.notify_one();
                Ok(json!({ "schedule": "queued" }))
            }
            ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress) => {
                tracing::info!(app = %app, param = %param_name, schedule = "rejected_in_progress", "set_param");
                Err(OiError::new(
                    ErrorCode::OperationInProgress,
                    format!("operation in progress for app: {app}"),
                ))
            }
            ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued) => {
                tracing::info!(app = %app, param = %param_name, schedule = "rejected_queued", "set_param");
                Err(OiError::new(
                    ErrorCode::AlreadyQueued,
                    format!("already queued for app: {app}"),
                ))
            }
        }
    } else {
        Ok(json!({ "schedule": "accepted" }))
    }
}

// i[param.unset]
pub(crate) fn unset_param(state: &OiState, params: UnsetParamParams) -> HandlerResult {
    let app = params.app.as_str();
    let param_name = params.name.as_str();

    {
        let reg = state.registry.read();
        if !reg.is_registered(app) {
            return Err(OiError::not_found(format!("app not found: {app}")));
        }
    }

    {
        let db = state.db.lock();
        crate::runtime::apps::delete_one_param(&db, app, param_name)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    }

    let script = {
        let reg = state.registry.read();
        reg.get(app).expect("confirmed registered").script.clone()
    };
    let loaded_params = {
        let db = state.db.lock();
        crate::runtime::apps::load_params_for_app(&db, app)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?
    };
    state
        .registry
        .write()
        .reload(app, script, &loaded_params, &state.script_limits);

    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(app) {
            let db = state.db.lock();
            crate::runtime::apps::sync_script_error_fault(&db, entry);
            crate::runtime::apps::sync_registry_faults(&db, entry);
        }
    }

    let (has_on_change, is_installed, tick_notify) = {
        let reg = state.registry.read();
        let entry = reg.get(app).expect("confirmed registered");
        let has = entry.app.def.lock().param_changes.contains(param_name);
        let installed = matches!(
            *entry.phase.lock(),
            AppPhase::Installed | AppPhase::Uninstalling
        );
        let notify = Arc::clone(&entry.tick_notify);
        (has, installed, notify)
    };

    if has_on_change && is_installed {
        let result = state.scheduler.lock().request(app, param_name, None);
        match result {
            ScheduleResult::Accepted => {
                tracing::info!(app = %app, param = %param_name, schedule = "accepted", "unset_param");
                tick_notify.notify_one();
                Ok(json!({ "schedule": "accepted" }))
            }
            ScheduleResult::Queued => {
                tracing::info!(app = %app, param = %param_name, schedule = "queued", "unset_param");
                tick_notify.notify_one();
                Ok(json!({ "schedule": "queued" }))
            }
            ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress) => {
                Err(OiError::new(
                    ErrorCode::OperationInProgress,
                    format!("operation in progress for app: {app}"),
                ))
            }
            ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued) => Err(OiError::new(
                ErrorCode::AlreadyQueued,
                format!("already queued for app: {app}"),
            )),
        }
    } else {
        tracing::info!(app = %app, param = %param_name, "unset_param");
        Ok(json!({ "schedule": "accepted" }))
    }
}
