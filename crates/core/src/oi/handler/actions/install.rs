use std::{collections::BTreeMap, sync::Arc};

use seedling_protocol::error::{ErrorCode, OiError};
use seedling_protocol::names::{ActionName, AppName, ParamName};
use serde::Deserialize;
use serde_json::json;

use super::lifecycle::spawn_accepted_operation;
use crate::{
    defs::install::{ParamDef, ParamKind},
    oi::{
        handler::{HandlerResult, RequestCtx},
        state::OiState,
    },
    runtime::{
        AppPhase,
        scheduler::{RejectReason, ScheduleResult},
    },
};

#[derive(Deserialize)]
pub(crate) struct InvokeInstallParams {
    pub app: AppName,
    #[serde(default)]
    pub params: Option<BTreeMap<String, String>>,
}

// i[action.invoke.install.validation]
fn is_valid_email(email: &str) -> bool {
    let mut parts = email.splitn(2, '@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

// i[action.invoke.install.validation]
fn is_strong_password(password: &str) -> bool {
    zxcvbn::zxcvbn(password, &[]).score() >= zxcvbn::Score::Three
}

// i[action.invoke.install.validation]
pub(in crate::oi) fn validate_requirements(
    schema: &BTreeMap<ParamName, ParamDef>,
    submitted: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OiError> {
    let mut filled = submitted.clone();
    let mut errors: Vec<String> = Vec::new();

    for (field, req_def) in schema {
        let field_str = field.as_str();
        let raw = filled.get(field_str).map(|s| s.as_str()).unwrap_or("");

        if raw.is_empty() {
            if let Some(default) = &req_def.default_value {
                filled.insert(field_str.to_owned(), default.clone());
            } else if req_def.required {
                errors.push(format!("{field_str}: required field is missing"));
                continue;
            } else {
                continue;
            }
        }

        let value = filled.get(field_str).map(|s| s.as_str()).unwrap_or("");
        match req_def.kind {
            ParamKind::Email => {
                if !is_valid_email(value) {
                    errors.push(format!("{field_str}: invalid email address"));
                }
            }
            ParamKind::Password => {
                if !is_strong_password(value) {
                    errors.push(format!("{field_str}: password is too weak"));
                }
            }
            ParamKind::Text | ParamKind::Multiline | ParamKind::WeakPassword => {}
        }
    }

    if !errors.is_empty() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            errors.join("; "),
        ));
    }

    Ok(filled)
}

// i[action.invoke.install.validation]
fn validate_install_params(
    state: &OiState,
    app_name: &AppName,
    submitted: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OiError> {
    let reg = state.registry.read();
    let entry = reg.get(app_name.as_str()).expect("caller confirmed exists");
    let def = entry.app.def.load();
    match &def.install {
        None => {
            if submitted.is_empty() {
                Ok(BTreeMap::new())
            } else {
                Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    "app has no install params",
                ))
            }
        }
        Some(inst) => validate_requirements(&inst.requirements, submitted),
    }
}

// i[action.not-installed-gate]
// i[action.invoke.install]
// i[action.invoke.install.validation]
pub(crate) fn invoke_install(
    state: &Arc<OiState>,
    params: InvokeInstallParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let app_name = &params.app;

    let submitted = params.params.unwrap_or_default();

    let has_install_action = {
        let reg = state.registry.read();
        let entry = reg
            .get(app_name.as_str())
            .ok_or_else(|| OiError::not_found(format!("app not found: {app_name}")))?;

        // i[action.invoke.install] - only NotInstalled apps may start an install.
        // Installing: an install for this app is already running — distinct error.
        // Installed / Uninstalling: install already happened (or is unwinding).
        match *entry.phase.lock() {
            AppPhase::NotInstalled => {}
            AppPhase::Installing => {
                return Err(OiError::new(
                    ErrorCode::InstallInProgress,
                    format!("install already in progress for app: {app_name}"),
                ));
            }
            AppPhase::Installed | AppPhase::Uninstalling => {
                return Err(OiError::new(
                    ErrorCode::AlreadyInstalled,
                    format!("app is already installed: {app_name}"),
                ));
            }
        }

        // i[action.invoke.install] - reject if script_error fault is active
        if entry.script_error.is_some() {
            return Err(OiError::script_error(format!(
                "app has a script error: {app_name}"
            )));
        }

        entry.app.def.load().install.is_some()
    };

    let filled = validate_install_params(state, app_name, &submitted)?;

    if !has_install_action {
        {
            let mut reg = state.registry.write();
            if let Some(entry) = reg.get_mut(app_name.as_str()) {
                *entry.phase.lock() = AppPhase::Installed;
            }
        }
        {
            let reg = state.registry.read();
            if let Some(entry) = reg.get(app_name.as_str()) {
                use crate::oi::handler::apps::{extract_persist_fields, persist_app_fields};
                let (app_name_owned, generation_n, installed, uninstalling, installing) =
                    extract_persist_fields(entry);
                state
                    .db
                    .call(move |db| {
                        persist_app_fields(
                            db,
                            &app_name_owned,
                            generation_n,
                            installed,
                            uninstalling,
                            installing,
                        )
                    })
                    .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db persist: {e}")))?;
            }
        }
        // i[impl event.types]
        state
            .event_tx
            .app_phase_changed(app_name, "installed", None);
        state.tick_notify.notify_one();
        tracing::info!(app = %app_name, schedule = "accepted", "invoke_install (immediate)");
        return Ok(json!({ "schedule": "accepted" }));
    }

    let op_params: serde_json::Map<String, serde_json::Value> = filled
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();

    // Operator-invoked install: source and target generation are both the
    // current generation (install does not produce a new generation).
    let current_generation = {
        let reg = state.registry.read();
        reg.get(app_name.as_str())
            .map(|e| e.current_generation)
            .unwrap_or(0)
    };
    let install_action = ActionName::new_unchecked("install");
    let (result, op_id_str) = {
        let mut sched = state.scheduler.lock();
        let result = sched.request(
            app_name,
            &install_action,
            op_params.clone(),
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
                install_action,
                op_id,
                op_params,
                current_generation,
                current_generation,
                "operator".to_owned(),
                Some(Arc::clone(&ctx.events.actor)),
            );
            tracing::info!(app = %app_name, schedule = "accepted", "invoke_install");
            Ok(json!({ "schedule": "accepted", "operation_id": op_id_str }))
        }
        ScheduleResult::Queued => {
            tracing::info!(app = %app_name, schedule = "queued", "invoke_install");
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
mod tests;
