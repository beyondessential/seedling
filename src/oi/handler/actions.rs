use std::{collections::BTreeMap, sync::Arc, time::Duration};

use serde_json::{Value, json};

use crate::{
    defs::install::InstallRequirementKind,
    oi::{
        error::{ErrorCode, OiError},
        state::OiState,
    },
    runtime::{
        AppPhase,
        apps::AppRegistry,
        scheduler::{RejectReason, ScheduleResult},
    },
};

use super::HandlerResult;

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
fn validate_requirements(
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
// i[action.invoke]
pub(crate) fn invoke_action(state: &Arc<OiState>, params: Value) -> HandlerResult {
    let app_name = params
        .get("app")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: app"))?;
    let action_name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| OiError::new(ErrorCode::RequirementsInvalid, "missing param: name"))?;

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
                "'{action_name}' is a shell action; use OpenShell"
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

    // If there is no on_install closure: mark installed immediately and start the reconciler.
    // The reconciler will run the start action on its first tick.
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

/// Spawn an async task that runs a lifecycle operation to completion, then
/// handles queued follow-on operations and install completion bookkeeping.
pub(crate) fn spawn_accepted_operation(
    state: Arc<OiState>,
    app_name: String,
    action_name: String,
    operation_id: crate::runtime::barrier::OperationId,
    install_requirements: Option<BTreeMap<String, String>>,
) {
    use crate::runtime::{
        AppRegistry, InstanceRegistry,
        barrier::oracle::DbWorldOracle,
        barrier::replay::{DbActionLog, OperationContext, OperationResult, run_operation},
        registry::DbInstanceRegistry,
    };

    let (app, active_progress, tick_notify, script) = {
        let reg = state.registry.read();
        match reg.get(&app_name) {
            Some(e) => (
                e.app.clone(),
                Arc::clone(&e.active_progress),
                Arc::clone(&e.tick_notify),
                e.script.clone(),
            ),
            None => {
                tracing::error!(app = %app_name, "spawn_accepted_operation: app not found");
                return;
            }
        }
    };
    let db_path = state.db_path.clone();
    let event_tx = state.event_tx.clone();
    let is_install = action_name == "install";

    tokio::spawn(async move {
        crate::oi::events::operation_started(&event_tx, &app_name, &action_name, &operation_id.0);
        let event_tx_bl = event_tx.clone();
        let app_name_bl = app_name.clone();
        let action_name_bl = action_name.clone();
        let active_progress_bl = Arc::clone(&active_progress);
        let tick_notify_bl = Arc::clone(&tick_notify);
        let operation_id_str = operation_id.0.clone();

        let success = tokio::task::spawn_blocking(move || {
            let (engine, mut scope, _) = crate::setup_language();
            let ast = match engine.compile(&script) {
                Ok(a) => a,
                Err(e) => {
                    tracing::error!(
                        app = %app_name_bl, action = %action_name_bl,
                        "script compile error: {e}"
                    );
                    return false;
                }
            };

            let action_log_db = match crate::runtime::db::Db::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!(app = %app_name_bl, "open action-log db: {e}");
                    return false;
                }
            };
            let world_db = match crate::runtime::db::Db::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!(app = %app_name_bl, "open world-oracle db: {e}");
                    return false;
                }
            };
            let instance_db = match crate::runtime::db::Db::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!(app = %app_name_bl, "open instance-registry db: {e}");
                    return false;
                }
            };
            let dynamic_db = match crate::runtime::db::Db::open(&db_path) {
                Ok(db) => Arc::new(parking_lot::Mutex::new(db)),
                Err(e) => {
                    tracing::error!(app = %app_name_bl, "open dynamic-resources db: {e}");
                    return false;
                }
            };

            let log = DbActionLog::new(
                action_log_db,
                operation_id.clone(),
                app_name_bl.clone(),
                action_name_bl.clone(),
            );
            let world = Arc::new(DbWorldOracle::new(world_db));
            let registry: Arc<dyn InstanceRegistry> =
                Arc::new(DbInstanceRegistry::new(instance_db));

            loop {
                let result = run_operation(
                    OperationContext {
                        engine: &engine,
                        script_ast: &ast,
                        operation_id: operation_id.clone(),
                        app: &app,
                        action_name: &action_name_bl,
                        log: &log,
                        world: Arc::clone(&world),
                        registry: Arc::clone(&registry),
                        active_progress: Some(Arc::clone(&active_progress_bl)),
                        tick_notify: Some(Arc::clone(&tick_notify_bl)),
                        install_requirements: install_requirements.clone(),
                        is_shell: false,
                        db: Some(Arc::clone(&dynamic_db)),
                    },
                    &mut scope,
                );
                match result {
                    OperationResult::Completed => {
                        crate::oi::events::operation_completed(
                            &event_tx_bl,
                            &app_name_bl,
                            &action_name_bl,
                            &operation_id.0,
                        );
                        return true;
                    }
                    OperationResult::Failed(e) => {
                        tracing::error!(
                            app = %app_name_bl, action = %action_name_bl,
                            "operation failed: {e}"
                        );
                        crate::oi::events::operation_failed(
                            &event_tx_bl,
                            &app_name_bl,
                            &action_name_bl,
                            &operation_id.0,
                            &e.to_string(),
                        );
                        return false;
                    }
                    OperationResult::Suspended(_) => {
                        tick_notify_bl.notify_one();
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        })
        .await
        .unwrap_or(false);

        // Tear down dynamic resources created during this operation.
        //
        // Load the records, build a cleanup OperationProgress with all
        // dynamic instances at Unscheduled, let the reconciler stop them,
        // then delete the DB records and clear active_progress.
        {
            use crate::defs::deployment::Deployment;
            use crate::defs::job::Job;
            use crate::defs::resource::Resource;
            use crate::defs::resource::ResourceKind;
            use crate::runtime::LifecycleState;
            use crate::runtime::barrier::oracle::derive_lifecycle_state;
            use crate::runtime::desired::{
                OperationProgress, delete_dynamic_resources_for_operation, list_dynamic_resources,
            };
            use crate::runtime::history::query_observations;
            use crate::runtime::identity::{InstanceId, InstanceVariant, ResourceInstance};

            let dynamic_records: Vec<_> = {
                let db = state.db.lock();
                list_dynamic_resources(&db)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|r| r.operation_id == operation_id_str)
                    .collect()
            };

            if !dynamic_records.is_empty() {
                let mut cleanup = OperationProgress::new();

                for record in &dynamic_records {
                    let uuid = match uuid::Uuid::parse_str(&record.instance_id) {
                        Ok(u) => u,
                        Err(e) => {
                            tracing::warn!(
                                instance_id = %record.instance_id,
                                "dynamic cleanup: bad instance_id: {e}"
                            );
                            continue;
                        }
                    };

                    let kind = match record.kind.as_str() {
                        "Deployment" => ResourceKind::Deployment,
                        "Job" => ResourceKind::Job,
                        _ => continue, // services are virtual; volumes cleaned by pod stop
                    };

                    let instance = ResourceInstance {
                        id: InstanceId(uuid),
                        app: record.app.clone(),
                        kind,
                        name: None,
                        variant: InstanceVariant::Singleton,
                        display_name: record.display_name.clone(),
                    };

                    // Minimal Resource so compute_during_operation can dispatch
                    // to the correct actuator.stop() variant.
                    // NOTE: anonymous volumes mounted on dynamic deployments may
                    // not be cleaned up here if the full definition is unavailable.
                    let minimal = match kind {
                        ResourceKind::Deployment => Resource::Deployment(Deployment {
                            name: std::sync::Arc::new(String::new()),
                            def: Default::default(),
                            frozen: false,
                        }),
                        ResourceKind::Job => Resource::Job(Job {
                            name: std::sync::Arc::new(String::new()),
                            def: Default::default(),
                            frozen: false,
                        }),
                        _ => unreachable!(),
                    };

                    cleanup.stopped(instance.clone());
                    cleanup.dynamic_defs.insert(instance, minimal);
                }

                if !cleanup.is_empty() {
                    *active_progress.write() = Some(cleanup);
                    tick_notify.notify_one();

                    // Poll until all instances reach Terminated or beyond, or
                    // we hit the timeout and let startup orphan cleanup handle it.
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
                    loop {
                        if tokio::time::Instant::now() >= deadline {
                            tracing::warn!(
                                operation_id = %operation_id_str,
                                "dynamic resource cleanup timed out"
                            );
                            break;
                        }

                        let all_stopped = {
                            let guard = active_progress.read();
                            if let Some(p) = &*guard {
                                p.dynamic_defs.keys().all(|inst| {
                                    let db = state.db.lock();
                                    let obs = query_observations(&db, inst).unwrap_or_default();
                                    derive_lifecycle_state(inst, &obs)
                                        .has_reached(LifecycleState::Terminated)
                                })
                            } else {
                                true
                            }
                        };

                        if all_stopped {
                            break;
                        }

                        tokio::time::sleep(Duration::from_secs(2)).await;
                        tick_notify.notify_one();
                    }
                }
            }

            // Delete the DB records regardless of whether cleanup succeeded,
            // so startup orphan cleanup can handle any stragglers.
            {
                let db = state.db.lock();
                if let Err(e) = delete_dynamic_resources_for_operation(&db, &operation_id_str) {
                    tracing::error!(
                        operation_id = %operation_id_str,
                        "failed to delete dynamic resource records: {e}"
                    );
                }
            }
        }

        // Clear active progress and wake the reconciler.
        *active_progress.write() = None;
        tick_notify.notify_one();

        // i[action.invoke.install.completion]
        if is_install && success {
            {
                let mut reg = state.registry.write();
                if let Some(entry) = reg.get_mut(&app_name) {
                    *entry.phase.lock() = AppPhase::Installed;
                    let db = state.db.lock();
                    if let Err(e) = AppRegistry::persist_app(&db, entry) {
                        tracing::error!(app = %app_name, "persist installed flag: {e}");
                    }
                }
            }
            state.tick_notify.notify_one();
            tracing::info!(app = %app_name, "install completed; app is now installed");
        }

        // Start the next queued operation, if any.
        let next = state.scheduler.lock().complete_current();
        if let Some(queued) = next {
            spawn_accepted_operation(
                Arc::clone(&state),
                queued.app,
                queued.action,
                queued.operation_id,
                queued.install_requirements,
            );
        }
    });
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
