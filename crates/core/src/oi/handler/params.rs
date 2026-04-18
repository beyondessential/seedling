use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use seedling_protocol::error::{ErrorCode, OiError};

use crate::{
    oi::state::OiState,
    runtime::{
        AppPhase, generations,
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

fn current_param_value(state: &OiState, app: &str, name: &str) -> Result<Option<String>, OiError> {
    let db = state.db.lock();
    let map = crate::runtime::apps::load_params_for_app(&db, app)
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    Ok(map.get(name).cloned())
}

fn reject_if_op_in_progress(state: &OiState, app: &str) -> Result<(), OiError> {
    if state.scheduler.lock().has_operation_for(app) {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {app}"),
        ));
    }
    Ok(())
}

fn reload_and_persist_apperror(
    state: &OiState,
    app: &str,
    new_generation: generations::Generation,
) -> Result<(), OiError> {
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
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(app) {
            entry.current_generation = new_generation;
        }
    }
    {
        let reg = state.registry.read();
        let entry = reg.get(app).expect("confirmed registered");
        let db = state.db.lock();
        crate::runtime::apps::AppRegistry::persist_app(&db, entry)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db persist: {e}")))?;
    }
    Ok(())
}

fn schedule_on_change(
    state: &OiState,
    app: &str,
    param_name: &str,
    generation: generations::Generation,
) -> Result<&'static str, OiError> {
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

    if !(has_on_change && is_installed) {
        return Ok("not_scheduled");
    }
    let source_generation = generation.saturating_sub(1);
    let mut sched = state.scheduler.lock();
    let result = sched.request(
        app,
        param_name,
        serde_json::Map::new(),
        source_generation,
        generation,
        "param_change",
    );
    match result {
        ScheduleResult::Accepted => {
            // Attach the operation to the generation history entry so the
            // outcome can be recorded against it later. Active operation is
            // the one we just requested.
            let op_id = sched
                .active()
                .map(|a| a.operation_id.0.clone())
                .unwrap_or_default();
            drop(sched);
            if !op_id.is_empty() {
                let db = state.db.lock();
                if let Err(e) = generations::attach_operation(&db, app, generation, &op_id) {
                    tracing::warn!(app, generation, "failed to attach op to generation: {e}");
                }
            }
            tick_notify.notify_one();
            Ok("accepted")
        }
        ScheduleResult::Queued => {
            // Spec says script and param updates during in-flight ops are
            // rejected, not queued. The scheduler shouldn't reach this case
            // since we checked has_operation_for earlier; treat as accepted to
            // be defensive.
            drop(sched);
            tick_notify.notify_one();
            Ok("accepted")
        }
        ScheduleResult::Rejected(RejectReason::SameAppOperationInProgress) => Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {app}"),
        )),
        ScheduleResult::Rejected(RejectReason::SameAppAlreadyQueued) => Err(OiError::new(
            ErrorCode::AlreadyQueued,
            format!("already queued for app: {app}"),
        )),
    }
}

// i[param.store]
// i[param.set]
// i[param.unknown]
// l[impl param.on-change.transitions]
// l[impl param.on-change.not-on-install]
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

    reject_if_op_in_progress(state, app)?;

    let previous_value = current_param_value(state, app, param_name)?;

    // Same-value set is a no-op.
    if previous_value.as_deref() == Some(value) {
        let generation = {
            let reg = state.registry.read();
            reg.get(app).map(|e| e.current_generation).unwrap_or(0)
        };
        return Ok(json!({
            "schedule": "not_scheduled",
            "generation": generation,
        }));
    }

    let previous_generation = {
        let reg = state.registry.read();
        reg.get(app).map(|e| e.current_generation).unwrap_or(0)
    };

    {
        let db = state.db.lock();
        crate::runtime::apps::upsert_param(&db, app, param_name, value)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    }

    let generation = {
        let db = state.db.lock();
        generations::bump_param_set(&db, app, param_name, previous_value.as_deref(), value)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db generation: {e}")))?
    };

    reload_and_persist_apperror(state, app, generation)?;

    let schedule = schedule_on_change(state, app, param_name, generation)?;

    seedling_protocol::events::param_set(
        &state.event_tx,
        app,
        param_name,
        previous_value.as_deref(),
        value,
        generation,
        previous_generation,
    );

    tracing::info!(app, param = param_name, generation, schedule, "set_param");
    Ok(json!({ "schedule": schedule, "generation": generation }))
}

// i[param.unset]
// l[impl param.on-change.transitions]
// l[impl param.on-change.not-on-install]
pub(crate) fn unset_param(state: &OiState, params: UnsetParamParams) -> HandlerResult {
    let app = params.app.as_str();
    let param_name = params.name.as_str();

    {
        let reg = state.registry.read();
        if !reg.is_registered(app) {
            return Err(OiError::not_found(format!("app not found: {app}")));
        }
    }

    reject_if_op_in_progress(state, app)?;

    let previous_value = current_param_value(state, app, param_name)?;
    let Some(previous_value) = previous_value else {
        let generation = {
            let reg = state.registry.read();
            reg.get(app).map(|e| e.current_generation).unwrap_or(0)
        };
        return Ok(json!({
            "schedule": "not_scheduled",
            "generation": generation,
        }));
    };

    let previous_generation = {
        let reg = state.registry.read();
        reg.get(app).map(|e| e.current_generation).unwrap_or(0)
    };

    {
        let db = state.db.lock();
        crate::runtime::apps::delete_one_param(&db, app, param_name)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    }

    let generation = {
        let db = state.db.lock();
        generations::bump_param_unset(&db, app, param_name, &previous_value)
            .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db generation: {e}")))?
    };

    reload_and_persist_apperror(state, app, generation)?;

    let schedule = schedule_on_change(state, app, param_name, generation)?;

    seedling_protocol::events::param_unset(
        &state.event_tx,
        app,
        param_name,
        &previous_value,
        generation,
        previous_generation,
    );

    tracing::info!(app, param = param_name, generation, schedule, "unset_param");
    Ok(json!({ "schedule": schedule, "generation": generation }))
}
