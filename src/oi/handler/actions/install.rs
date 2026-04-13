use std::{collections::BTreeMap, sync::Arc};

use serde_json::{Value, json};

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
pub(crate) fn invoke_install(state: &Arc<OiState>, params: Value) -> HandlerResult {
    let app_name = params
        .get("app")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: app"))?;

    let submitted: BTreeMap<String, String> = match params.get("requirements") {
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned()))
            .collect(),
        None | Some(Value::Null) => BTreeMap::new(),
        _ => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                "requirements must be an object",
            ));
        }
    };

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

    let install_reqs = if filled.is_empty() {
        None
    } else {
        Some(filled)
    };

    let (result, op_id_opt) = {
        let mut sched = state.scheduler.lock();
        let result = sched.request(app_name, "install", install_reqs.clone());
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
                    "install".to_owned(),
                    op_id,
                    install_reqs,
                );
            }
            tracing::info!(app = %app_name, schedule = "accepted", "invoke_install");
            Ok(json!({ "schedule": "accepted" }))
        }
        ScheduleResult::Queued => {
            tracing::info!(app = %app_name, schedule = "queued", "invoke_install");
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::defs::install::{InstallDef, InstallRequirementDef, InstallRequirementKind};

    use super::{is_strong_password, is_valid_email, validate_requirements};

    // i[verify action.invoke.install.validation]
    #[test]
    fn valid_email_basic() {
        assert!(is_valid_email("user@example.com"));
        assert!(is_valid_email("a@b.co"));
        assert!(is_valid_email("user+tag@sub.example.org"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_no_at() {
        assert!(!is_valid_email("notanemail"));
        assert!(!is_valid_email(""));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_no_dot_in_domain() {
        assert!(!is_valid_email("user@nodot"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_empty_local() {
        assert!(!is_valid_email("@example.com"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_domain_starts_with_dot() {
        assert!(!is_valid_email("user@.example.com"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_domain_ends_with_dot() {
        assert!(!is_valid_email("user@example.com."));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn strong_password_accepted() {
        assert!(is_strong_password("correct-horse-battery-staple-42!"));
        assert!(is_strong_password("Tr0ub4dor&3xtraL0ng"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn weak_password_rejected() {
        assert!(!is_strong_password("password"));
        assert!(!is_strong_password("123456"));
        assert!(!is_strong_password("abc"));
    }

    fn req(
        kind: InstallRequirementKind,
        required: bool,
        default: Option<&str>,
    ) -> InstallRequirementDef {
        InstallRequirementDef {
            kind,
            required,
            default_value: default.map(|s| s.to_owned()),
            description: None,
        }
    }

    fn install_def(fields: &[(&str, InstallRequirementDef)]) -> InstallDef {
        InstallDef {
            requirements: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn no_install_def_empty_requirements_ok() {
        let result = validate_requirements(None, &BTreeMap::new());
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn no_install_def_nonempty_requirements_rejected() {
        let mut submitted = BTreeMap::new();
        submitted.insert("key".to_owned(), "value".to_owned());
        let result = validate_requirements(None, &submitted);
        assert!(result.is_err());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn required_field_missing_returns_error() {
        let def = install_def(&[("email", req(InstallRequirementKind::Text, true, None))]);
        let result = validate_requirements(Some(&def), &BTreeMap::new());
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("email"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn required_field_with_default_filled_in() {
        let def = install_def(&[(
            "site",
            req(InstallRequirementKind::Text, true, Some("default-site")),
        )]);
        let result = validate_requirements(Some(&def), &BTreeMap::new());
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap().get("site").map(String::as_str),
            Some("default-site")
        );
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn optional_field_absent_is_ok() {
        let def = install_def(&[("note", req(InstallRequirementKind::Text, false, None))]);
        let result = validate_requirements(Some(&def), &BTreeMap::new());
        assert!(result.is_ok());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn invalid_email_field_returns_error() {
        let def = install_def(&[("email", req(InstallRequirementKind::Email, true, None))]);
        let mut submitted = BTreeMap::new();
        submitted.insert("email".to_owned(), "notanemail".to_owned());
        let result = validate_requirements(Some(&def), &submitted);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("email"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn valid_email_field_passes() {
        let def = install_def(&[("email", req(InstallRequirementKind::Email, true, None))]);
        let mut submitted = BTreeMap::new();
        submitted.insert("email".to_owned(), "user@example.com".to_owned());
        let result = validate_requirements(Some(&def), &submitted);
        assert!(result.is_ok());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn weak_password_field_returns_error() {
        let def = install_def(&[("pw", req(InstallRequirementKind::Password, true, None))]);
        let mut submitted = BTreeMap::new();
        submitted.insert("pw".to_owned(), "password".to_owned());
        let result = validate_requirements(Some(&def), &submitted);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("pw"));
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn strong_password_field_passes() {
        let def = install_def(&[("pw", req(InstallRequirementKind::Password, true, None))]);
        let mut submitted = BTreeMap::new();
        submitted.insert(
            "pw".to_owned(),
            "correct-horse-battery-staple-42!".to_owned(),
        );
        let result = validate_requirements(Some(&def), &submitted);
        assert!(result.is_ok());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn weak_password_kind_always_passes() {
        let def = install_def(&[("pw", req(InstallRequirementKind::WeakPassword, true, None))]);
        let mut submitted = BTreeMap::new();
        submitted.insert("pw".to_owned(), "password".to_owned());
        let result = validate_requirements(Some(&def), &submitted);
        assert!(result.is_ok());
    }

    // i[verify action.invoke.install.validation]
    #[test]
    fn multiple_errors_collected() {
        let def = install_def(&[
            ("email", req(InstallRequirementKind::Email, true, None)),
            ("name", req(InstallRequirementKind::Text, true, None)),
        ]);
        let result = validate_requirements(Some(&def), &BTreeMap::new());
        assert!(result.is_err());
        let msg = result.unwrap_err().message;
        assert!(msg.contains("email") || msg.contains("name"));
    }
}
