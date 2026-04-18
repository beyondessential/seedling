use std::{collections::BTreeMap, sync::Arc};

use serde::Deserialize;
use serde_json::json;

use crate::{
    defs::install::InstallRequirementKind,
    oi::{
        error::{ErrorCode, OiError},
        handler::HandlerResult,
        state::OiState,
    },
    runtime::{
        AppPhase,
        apps::AppRegistry,
        scheduler::{RejectReason, ScheduleResult},
    },
};

use super::lifecycle::spawn_accepted_operation;

#[derive(Deserialize)]
pub(crate) struct InvokeInstallParams {
    pub app: String,
    #[serde(default)]
    pub requirements: Option<BTreeMap<String, String>>,
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
    zxcvbn::zxcvbn(password, &[])
        .map(|e| e.score() >= 3)
        .unwrap_or(false)
}

// i[action.invoke.install.validation]
pub(in crate::oi) fn validate_requirements(
    install_def: Option<&crate::defs::install::InstallDef>,
    submitted: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OiError> {
    let install_def = match install_def {
        Some(d) => d,
        None => {
            return if submitted.is_empty() {
                Ok(BTreeMap::new())
            } else {
                Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    "app has no install requirements",
                ))
            };
        }
    };

    let mut filled = submitted.clone();
    let mut errors: Vec<String> = Vec::new();

    for (field, req_def) in &install_def.requirements {
        let raw = filled.get(field).map(|s| s.as_str()).unwrap_or("");

        if raw.is_empty() {
            if let Some(default) = &req_def.default_value {
                filled.insert(field.clone(), default.clone());
            } else if req_def.required {
                errors.push(format!("{field}: required field is missing"));
                continue;
            } else {
                continue;
            }
        }

        let value = filled.get(field).map(|s| s.as_str()).unwrap_or("");
        match req_def.kind {
            InstallRequirementKind::Email => {
                if !is_valid_email(value) {
                    errors.push(format!("{field}: invalid email address"));
                }
            }
            InstallRequirementKind::Password => {
                if !is_strong_password(value) {
                    errors.push(format!("{field}: password is too weak"));
                }
            }
            InstallRequirementKind::Text | InstallRequirementKind::WeakPassword => {}
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
fn validate_install_requirements(
    state: &OiState,
    app_name: &str,
    submitted: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OiError> {
    let reg = state.registry.read();
    let entry = reg.get(app_name).expect("caller confirmed exists");
    let def = entry.app.def.lock();
    validate_requirements(def.install.as_ref(), submitted)
}

// i[action.not-installed-gate]
// i[action.invoke.install]
// i[action.invoke.install.validation]
pub(crate) fn invoke_install(state: &Arc<OiState>, params: InvokeInstallParams) -> HandlerResult {
    let app_name = &params.app;

    let submitted = params.requirements.unwrap_or_default();

    let has_install_action = {
        let reg = state.registry.read();
        let entry = reg
            .get(app_name)
            .ok_or_else(|| OiError::not_found(format!("app not found: {app_name}")))?;

        // i[action.invoke.install] - reject if already installed or uninstalling
        if !matches!(*entry.phase.lock(), AppPhase::NotInstalled) {
            return Err(OiError::new(
                ErrorCode::AlreadyInstalled,
                format!("app is already installed: {app_name}"),
            ));
        }

        entry.app.def.lock().install.is_some()
    };

    let filled = validate_install_requirements(state, app_name, &submitted)?;

    if !has_install_action {
        {
            let mut reg = state.registry.write();
            if let Some(entry) = reg.get_mut(app_name) {
                *entry.phase.lock() = AppPhase::Installed;
                let db = state.db.lock();
                AppRegistry::persist_app(&db, entry)
                    .map_err(|e| OiError::new(ErrorCode::NotFound, format!("db persist: {e}")))?;
            }
        }
        state.tick_notify.notify_one();
        tracing::info!(app = %app_name, schedule = "accepted", "invoke_install (immediate)");
        return Ok(json!({ "schedule": "accepted" }));
    }

    let params: serde_json::Map<String, serde_json::Value> = filled
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();

    // Operator-invoked install: source and target generation are both the
    // current generation (install does not produce a new generation).
    let current_generation = {
        let reg = state.registry.read();
        reg.get(app_name).map(|e| e.current_generation).unwrap_or(0)
    };
    let (result, op_id_str) = {
        let mut sched = state.scheduler.lock();
        let result = sched.request(
            app_name,
            "install",
            params.clone(),
            current_generation,
            current_generation,
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
                "install".to_owned(),
                op_id,
                params,
                current_generation,
                current_generation,
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
