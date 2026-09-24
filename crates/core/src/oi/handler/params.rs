use std::{collections::BTreeMap, sync::Arc};

use seedling_protocol::error::{ErrorCode, OiError};
use seedling_protocol::names::{AppName, ParamName};
use serde::Deserialize;
use serde_json::json;

use super::HandlerResult;
use crate::{
    oi::{handler::RequestCtx, state::OiState},
    runtime::{
        AppPhase, generations,
        scheduler::{RejectReason, ScheduleResult},
    },
};

#[derive(Deserialize)]
pub(crate) struct SetParamParams {
    pub app: AppName,
    pub name: ParamName,
    pub value: String,
}

#[derive(Deserialize)]
pub(crate) struct UnsetParamParams {
    pub app: AppName,
    pub name: ParamName,
}

fn current_param_value(
    state: &OiState,
    app: &AppName,
    name: &ParamName,
) -> Result<Option<String>, OiError> {
    let app_owned = app.clone();
    let name_owned = name.clone();
    let cipher = Arc::clone(&state.cipher);
    let map = state
        .db
        .call(move |db| -> rusqlite::Result<BTreeMap<String, String>> {
            Ok(crate::runtime::apps::load_all_params_for_app(
                db, &cipher, &app_owned,
            ))
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    Ok(map.get(name_owned.as_str()).cloned())
}

fn param_is_secret(state: &OiState, app: &AppName, name: &ParamName) -> bool {
    state
        .registry
        .read()
        .get(app.as_str())
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
fn reject_if_op_in_progress(state: &OiState, app: &AppName) -> Result<(), OiError> {
    use crate::runtime::apps::AppPhase;
    if state.scheduler.lock().has_operation_for(app) {
        return Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("operation in progress for app: {app}"),
        ));
    }
    let reg = state.registry.read();
    if let Some(entry) = reg.get(app.as_str())
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
    app: &AppName,
    new_generation: generations::Generation,
) -> Result<(), OiError> {
    use crate::oi::handler::apps::{extract_persist_fields, persist_app_fields, sync_fault_state};

    let app_owned = app.clone();
    let cipher = Arc::clone(&state.cipher);
    let loaded_params = state
        .db
        .call(move |db| -> rusqlite::Result<BTreeMap<String, String>> {
            Ok(crate::runtime::apps::load_all_params_for_app(
                db, &cipher, &app_owned,
            ))
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;
    // The outcome needs no gating here: this re-evaluates the app's existing
    // definition under new param values and derives nothing from the result
    // but the fault state synced below, which is filed on either outcome.
    let _ = state
        .registry
        .write()
        .reload(app, &loaded_params, &state.script_limits);
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(app.as_str()) {
            sync_fault_state(&state.db, entry);
        }
    }
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(app.as_str()) {
            entry.current_generation = new_generation;
        }
    }
    {
        let reg = state.registry.read();
        let entry = reg.get(app.as_str()).expect("confirmed registered");
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

/// Validate the values a parameter set or unset would leave the app with,
/// before anything is persisted.
///
/// The validators are those of the current definition evaluated with the
/// proposed values; when that evaluation fails, those of the app's most
/// recent successful evaluation, which the registry holds as its running
/// `App` together with the bundle and values it came from.
// i[impl param.validation]
fn validate_proposed(
    state: &OiState,
    app: &AppName,
    change: (&ParamName, Option<&str>),
) -> Result<(), OiError> {
    let (bundle, last_good) = {
        let reg = state.registry.read();
        let entry = reg.get(app.as_str()).expect("confirmed registered");
        (
            Arc::clone(&entry.bundle),
            (
                Arc::clone(&entry.app.bundle),
                entry.app.stored.lock().clone(),
            ),
        )
    };
    let app_owned = app.clone();
    let cipher = Arc::clone(&state.cipher);
    let mut proposed = state
        .db
        .call(move |db| crate::runtime::apps::load_all_params_for_app(db, &cipher, &app_owned));
    match change.1 {
        Some(v) => proposed.insert(change.0.as_str().to_owned(), v.to_owned()),
        None => proposed.remove(change.0.as_str()),
    };
    let limits = &state.script_limits;
    let (_, result) =
        crate::runtime::apps::evaluate_validated(app, &bundle, &proposed, &proposed, limits);
    let rejections = match result {
        Ok(r) => r,
        Err(_) => {
            let (last_bundle, last_values) = last_good;
            let (_, fallback) = crate::runtime::apps::evaluate_validated(
                app,
                &last_bundle,
                &last_values,
                &proposed,
                limits,
            );
            // With no evaluation that succeeds there are no validators to
            // run; the value is stored and the failure surfaces as
            // `script_error`, as it would without validation.
            fallback.unwrap_or_default()
        }
    };
    if rejections.is_empty() {
        Ok(())
    } else {
        Err(super::apps::validation_error(&rejections))
    }
}

pub(crate) fn schedule_on_change(
    state: &OiState,
    app: &AppName,
    param_name: &ParamName,
    generation: generations::Generation,
) -> Result<&'static str, OiError> {
    let (has_on_change, is_installed, tick_notify) = {
        let reg = state.registry.read();
        let entry = reg.get(app.as_str()).expect("confirmed registered");
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
    // The on_change handler is dispatched by using the param name itself as an
    // action identifier. Param names follow the same bsl.name rules.
    let param_action = seedling_protocol::names::ActionName::new_unchecked(param_name.as_str());
    let mut sched = state.scheduler.lock();
    let result = sched.request(
        app,
        &param_action,
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
                let app_owned = app.clone();
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
    let app = &params.app;
    let param_name = &params.name;
    let value = params.value.as_str();

    {
        let reg = state.registry.read();
        if !reg.is_registered(app.as_str()) {
            return Err(OiError::not_found(format!("app not found: {app}")));
        }
    }

    reject_if_op_in_progress(state, app)?;

    let previous_value = current_param_value(state, app, param_name)?;

    // Same-value set is a no-op.
    if previous_value.as_deref() == Some(value) {
        let generation = {
            let reg = state.registry.read();
            reg.get(app.as_str())
                .map(|e| e.current_generation)
                .unwrap_or(0)
        };
        return Ok(json!({
            "schedule": "not_scheduled",
            "generation": generation,
        }));
    }

    let previous_generation = {
        let reg = state.registry.read();
        reg.get(app.as_str())
            .map(|e| e.current_generation)
            .unwrap_or(0)
    };

    validate_proposed(state, app, (param_name, Some(value)))?;

    let is_secret = param_is_secret(state, app, param_name);
    let app_owned = app.clone();
    let param_name_owned = param_name.clone();
    let value_owned = value.to_owned();
    let prev_owned = previous_value.clone();
    let cipher = Arc::clone(&state.cipher);

    let (generation, previous_is_secret) = state
        .db
        .call(move |db| -> rusqlite::Result<_> {
            // r[impl secret.history] — read before the write replaces it.
            let previous_is_secret = crate::runtime::apps::secret_params::is_stored_secret(
                db,
                &app_owned,
                &param_name_owned,
            )?;
            crate::runtime::apps::store_param_value(
                db,
                &cipher,
                &app_owned,
                &param_name_owned,
                Some(&value_owned),
                is_secret,
            )?;
            let change = generations::ParamChange {
                name: &param_name_owned,
                previous: prev_owned.as_deref(),
                new_value: Some(&value_owned),
                is_secret,
                previous_is_secret,
            };
            let generation = generations::bump_param_change(db, &app_owned, &change, &cipher)?;
            Ok((generation, previous_is_secret))
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;

    reload_and_persist_apperror(state, app, generation)?;

    // r[impl image.pin.update-reconcile]
    super::images::reconcile_pins_post_update(state, app);

    let schedule = schedule_on_change(state, app, param_name, generation)?;

    // i[impl param.store.secret]
    // r[impl secret.history] — the event carries one redaction flag for both
    // values, so a previous value that was secret withholds the pair.
    if is_secret || previous_is_secret {
        ctx.events
            .param_change(app.clone(), generation, previous_generation)
            .set_redacted(param_name);
    } else {
        ctx.events
            .param_change(app.clone(), generation, previous_generation)
            .set(param_name, previous_value.as_deref(), value);
    }

    tracing::info!(app = %app, param = %param_name, generation, schedule, "set_param");
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
    let app = &params.app;
    let param_name = &params.name;

    {
        let reg = state.registry.read();
        if !reg.is_registered(app.as_str()) {
            return Err(OiError::not_found(format!("app not found: {app}")));
        }
    }

    reject_if_op_in_progress(state, app)?;

    let previous_value = current_param_value(state, app, param_name)?;
    let Some(previous_value) = previous_value else {
        let generation = {
            let reg = state.registry.read();
            reg.get(app.as_str())
                .map(|e| e.current_generation)
                .unwrap_or(0)
        };
        return Ok(json!({
            "schedule": "not_scheduled",
            "generation": generation,
        }));
    };

    let previous_generation = {
        let reg = state.registry.read();
        reg.get(app.as_str())
            .map(|e| e.current_generation)
            .unwrap_or(0)
    };

    validate_proposed(state, app, (param_name, None))?;

    let is_secret = param_is_secret(state, app, param_name);

    let app_owned = app.clone();
    let param_name_owned = param_name.clone();
    let prev_owned = previous_value.clone();
    let cipher = Arc::clone(&state.cipher);
    let (generation, previous_is_secret) = state
        .db
        .call(move |db| -> rusqlite::Result<_> {
            // r[impl secret.history] — read before the delete removes it.
            let previous_is_secret = crate::runtime::apps::secret_params::is_stored_secret(
                db,
                &app_owned,
                &param_name_owned,
            )?;
            crate::runtime::apps::store_param_value(
                db,
                &cipher,
                &app_owned,
                &param_name_owned,
                None,
                is_secret,
            )?;
            let change = generations::ParamChange {
                name: &param_name_owned,
                previous: Some(&prev_owned),
                new_value: None,
                is_secret,
                previous_is_secret,
            };
            let generation = generations::bump_param_change(db, &app_owned, &change, &cipher)?;
            Ok((generation, previous_is_secret))
        })
        .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db error: {e}")))?;

    reload_and_persist_apperror(state, app, generation)?;

    // r[impl image.pin.update-reconcile]
    super::images::reconcile_pins_post_update(state, app);

    let schedule = schedule_on_change(state, app, param_name, generation)?;

    // i[impl param.store.secret]
    // r[impl secret.history]
    if previous_is_secret {
        ctx.events
            .param_change(app.clone(), generation, previous_generation)
            .unset_redacted(param_name);
    } else {
        ctx.events
            .param_change(app.clone(), generation, previous_generation)
            .unset(param_name, &previous_value);
    }

    tracing::info!(app = %app, param = %param_name, generation, schedule, "unset_param");
    Ok(json!({ "schedule": schedule, "generation": generation }))
}

#[cfg(test)]
mod tests;
