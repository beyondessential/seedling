use std::{collections::BTreeMap, sync::Arc};

use secrecy::SecretString;
use serde::Deserialize;
use serde_json::json;

use seedling_protocol::error::{ErrorCode, OiError};

use crate::{
    oi::{handler::RequestCtx, state::OiState},
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
    let app_owned = app.to_owned();
    let name_owned = name.to_owned();
    let cipher = Arc::clone(&state.cipher);
    let map = state
        .db
        .call(move |db| -> rusqlite::Result<BTreeMap<String, String>> {
            Ok(crate::runtime::apps::load_all_params_for_app(
                db, &cipher, &app_owned,
            ))
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    Ok(map.get(&name_owned).cloned())
}

fn param_is_secret(state: &OiState, app: &str, name: &str) -> bool {
    state
        .registry
        .read()
        .get(app)
        .and_then(|e| e.app.def.load().params.get(name).map(|d| d.is_secret()))
        .unwrap_or(false)
}

// i[impl param.set] i[impl param.unset]
// r[impl operation.lifecycle.param-change]
// Params cannot be mutated while an operation is in flight for the app:
// captured-closure state inside the operation would become inconsistent with
// the new param value. The scheduler is the primary source of truth; the
// phase check covers the narrow window between boot (phase = Installing
// persisted by a prior process) and the replay path re-registering the
// operation with the in-memory scheduler.
fn reject_if_op_in_progress(state: &OiState, app: &str) -> Result<(), OiError> {
    use crate::runtime::apps::AppPhase;
    if state.scheduler.lock().has_operation_for(app) {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {app}"),
        ));
    }
    let reg = state.registry.read();
    if let Some(entry) = reg.get(app)
        && matches!(*entry.phase.lock(), AppPhase::Installing)
    {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("install is in progress for app: {app}"),
        ));
    }
    Ok(())
}

fn reload_and_persist_apperror(
    state: &OiState,
    app: &str,
    new_generation: generations::Generation,
) -> Result<(), OiError> {
    use crate::oi::handler::apps::{extract_persist_fields, persist_app_fields, sync_fault_state};

    let script = {
        let reg = state.registry.read();
        reg.get(app).expect("confirmed registered").script.clone()
    };
    let app_owned = app.to_owned();
    let cipher = Arc::clone(&state.cipher);
    let loaded_params = state
        .db
        .call(move |db| -> rusqlite::Result<BTreeMap<String, String>> {
            Ok(crate::runtime::apps::load_all_params_for_app(
                db, &cipher, &app_owned,
            ))
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    state
        .registry
        .write()
        .reload(app, script, &loaded_params, &state.script_limits);
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(app) {
            sync_fault_state(&state.db, entry);
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
        let (app_name, generation_n, installed, uninstalling, installing) =
            extract_persist_fields(entry);
        state
            .db
            .call(move |db| {
                persist_app_fields(
                    db,
                    &app_name,
                    generation_n,
                    installed,
                    uninstalling,
                    installing,
                )
            })
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
        let has = entry.app.def.load().param_changes.contains(param_name);
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
                let app_owned = app.to_owned();
                let op_id_owned = op_id.clone();
                state.db.call(move |db| {
                    if let Err(e) = generations::attach_operation(db, &app_owned, generation, &op_id_owned) {
                        tracing::warn!(app = %app_owned, generation, "failed to attach op to generation: {e}");
                    }
                });
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
// i[impl param.store.secret]
// l[impl param.on-change.transitions]
// l[impl param.on-change.not-on-install]
pub(crate) fn set_param(
    state: &OiState,
    params: SetParamParams,
    ctx: &RequestCtx,
) -> HandlerResult {
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

    let is_secret = param_is_secret(state, app, param_name);
    let app_owned = app.to_owned();
    let param_name_owned = param_name.to_owned();
    let value_owned = value.to_owned();
    let prev_owned = previous_value.clone();
    let cipher = Arc::clone(&state.cipher);

    let generation = state
        .db
        .call(move |db| -> rusqlite::Result<_> {
            if is_secret {
                let secret_val = SecretString::new(value_owned.clone().into());
                crate::runtime::apps::secret_params::upsert_secret_param(
                    db,
                    &cipher,
                    &app_owned,
                    &param_name_owned,
                    &secret_val,
                )?;
                crate::runtime::apps::delete_one_param(db, &app_owned, &param_name_owned)?;
            } else {
                crate::runtime::apps::upsert_param(
                    db,
                    &app_owned,
                    &param_name_owned,
                    &value_owned,
                )?;
                crate::runtime::apps::secret_params::delete_one_secret_param(
                    db,
                    &app_owned,
                    &param_name_owned,
                )?;
            }
            generations::bump_param_set(
                db,
                &app_owned,
                &param_name_owned,
                prev_owned.as_deref(),
                &value_owned,
                &cipher,
                is_secret,
            )
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;

    reload_and_persist_apperror(state, app, generation)?;

    let schedule = schedule_on_change(state, app, param_name, generation)?;

    // i[impl param.store.secret]
    if is_secret {
        ctx.events
            .param_change(app, generation, previous_generation)
            .set_redacted(param_name);
    } else {
        ctx.events
            .param_change(app, generation, previous_generation)
            .set(param_name, previous_value.as_deref(), value);
    }

    tracing::info!(app, param = param_name, generation, schedule, "set_param");
    Ok(json!({ "schedule": schedule, "generation": generation }))
}

// i[param.unset]
// l[impl param.on-change.transitions]
// l[impl param.on-change.not-on-install]
pub(crate) fn unset_param(
    state: &OiState,
    params: UnsetParamParams,
    ctx: &RequestCtx,
) -> HandlerResult {
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

    let is_secret = param_is_secret(state, app, param_name);

    let app_owned = app.to_owned();
    let param_name_owned = param_name.to_owned();
    let prev_owned = previous_value.clone();
    let cipher = Arc::clone(&state.cipher);
    let generation = state
        .db
        .call(move |db| -> rusqlite::Result<_> {
            // Delete from both tables to handle any migration state.
            crate::runtime::apps::delete_one_param(db, &app_owned, &param_name_owned)?;
            crate::runtime::apps::secret_params::delete_one_secret_param(
                db,
                &app_owned,
                &param_name_owned,
            )?;
            generations::bump_param_unset(
                db,
                &app_owned,
                &param_name_owned,
                &prev_owned,
                &cipher,
                is_secret,
            )
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;

    reload_and_persist_apperror(state, app, generation)?;

    let schedule = schedule_on_change(state, app, param_name, generation)?;

    // i[impl param.store.secret]
    if is_secret {
        ctx.events
            .param_change(app, generation, previous_generation)
            .unset_redacted(param_name);
    } else {
        ctx.events
            .param_change(app, generation, previous_generation)
            .unset(param_name, &previous_value);
    }

    tracing::info!(app, param = param_name, generation, schedule, "unset_param");
    Ok(json!({ "schedule": schedule, "generation": generation }))
}
