use std::collections::BTreeMap;
use std::sync::Arc;

use seedling_protocol::error::{ErrorCode, OiError};
use seedling_protocol::names::{ActionName, AppName, ParamName, SiteVolumeName};
use serde::Deserialize;
use serde_json::json;

use self::lifecycle::spawn_accepted_operation;
use super::HandlerResult;
use crate::{
    defs::install::ParamDef,
    oi::{handler::RequestCtx, state::OiState},
    runtime::{
        AppPhase,
        action_params::{self, ParamValidationError, VolumeLookup},
        scheduler::{CancelOutcome, RejectReason, ScheduleResult},
    },
};

pub(crate) mod install;
pub mod lifecycle;

#[derive(Deserialize)]
pub(crate) struct InvokeActionParams {
    pub app: AppName,
    pub name: ActionName,
    #[serde(default)]
    pub params: Option<serde_json::Map<String, serde_json::Value>>,
}

// spec: ACI#cancellation
#[derive(Deserialize)]
pub(crate) struct CancelActionParams {
    pub app: AppName,
}

/// Request cancellation of the currently-active operation for `app`.
// spec: OPS#cancellation
// spec: ACI#cancellation
pub(crate) fn cancel_action(state: &Arc<OiState>, params: CancelActionParams) -> HandlerResult {
    let app_name = &params.app;
    let outcome = state.scheduler.lock().request_cancel(app_name);
    match outcome {
        CancelOutcome::Cancelled(op_id) | CancelOutcome::AlreadyCancelled(op_id) => {
            // spec: OPS#cancel-persistence
            // Persist the flag so a daemon crash between the in-memory flip
            // and cancel observation leaves the op in a pre-cancelled state
            // on the next start-up. Do this on AlreadyCancelled too so a
            // prior persist failure heals on retry.
            let op_id_for_persist = op_id.clone();
            let app_for_log = app_name.clone();
            state.db.call(move |db| {
                if let Err(e) =
                    crate::runtime::history::set_cancel_requested(db, &op_id_for_persist)
                {
                    tracing::warn!(
                        app = %app_for_log,
                        operation_id = %op_id_for_persist.0,
                        "failed to persist cancel_requested: {e}"
                    );
                }
            });
            tracing::info!(app = %app_name, operation_id = %op_id.0, "operation cancel requested");
            Ok(json!({ "cancelled": true }))
        }
        CancelOutcome::NoActiveOp => {
            // spec: OPS#stuck-install-recovery
            // The scheduler has no live operation, but the app's persisted
            // phase may be Installing from a prior run that crashed or whose
            // post-cancel cleanup failed to revert the phase. Without a
            // recovery path the operator can neither cancel nor uninstall
            // such an app — both gates check phase. Treat cancel as "unstick
            // the phase": drop into Uninstalling so the reconciler tears
            // down any half-installed resources (the reconciler skips
            // NotInstalled apps and would otherwise strand containers and
            // units that the previous install had already created).
            let stuck_phase = {
                let reg = state.registry.read();
                reg.get(app_name.as_str()).map(|e| e.phase.lock().clone())
            };
            match stuck_phase {
                Some(AppPhase::Installing) => {
                    lifecycle::set_phase_and_persist(state, app_name, AppPhase::Uninstalling);
                    tracing::warn!(
                        app = %app_name,
                        "no active operation but phase was Installing; transitioning to Uninstalling for teardown"
                    );
                    Ok(json!({ "cancelled": true, "recovered_stuck_phase": "installing" }))
                }
                _ => Err(OiError::not_found(format!(
                    "no active operation to cancel for app: {app_name}"
                ))),
            }
        }
    }
}

/// `OiState`-backed `VolumeLookup` adapter so the shared validation
/// can resolve site volumes through the live registry.
pub(crate) struct StateVolumeLookup<'a>(pub &'a OiState);

impl VolumeLookup for StateVolumeLookup<'_> {
    fn site_volume_exists(&self, name: &SiteVolumeName) -> bool {
        let name = name.clone();
        self.0
            .db
            .call(move |db| crate::runtime::site_volumes::get(db, &name).ok().flatten())
            .is_some()
    }
}

/// Map a [`ParamValidationError`] into the OI error envelope. Volume
/// not-found gets a distinct code; everything else is
/// `requirements_invalid`.
pub(crate) fn validation_error_to_oi(e: ParamValidationError) -> OiError {
    let code = match e {
        ParamValidationError::VolumeNotFound { .. } => ErrorCode::NotFound,
        _ => ErrorCode::RequirementsInvalid,
    };
    OiError::new(code, e.to_string())
}

// spec: ACN#param-delivery
fn validate_action_params(
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), OiError> {
    action_params::reject_reserved_keys(params).map_err(validation_error_to_oi)
}

// spec: ACN#volume-params
fn validate_volume_params(
    state: &OiState,
    schema: &BTreeMap<ParamName, ParamDef>,
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), OiError> {
    action_params::validate_volume_params(schema, params, &StateVolumeLookup(state))
        .map_err(validation_error_to_oi)
}

// spec: ACI#invoking-an-action
fn apply_action_param_schema(
    action_params_schema: &BTreeMap<ParamName, ParamDef>,
    params: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<(), OiError> {
    action_params::apply_schema(action_params_schema, params).map_err(validation_error_to_oi)
}

// spec: ACI#invoking-an-action
pub(crate) fn invoke_action(
    state: &Arc<OiState>,
    params: InvokeActionParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let app_name = &params.app;
    let action_name = &params.name;
    let mut action_params = params.params.unwrap_or_default();
    validate_action_params(&action_params)?;

    let phase_handle = {
        let reg = state.registry.read();
        let entry = reg
            .get(app_name.as_str())
            .ok_or_else(|| OiError::not_found(format!("app not found: {app_name}")))?;

        // spec: ACI#invoking-an-action
        if !matches!(*entry.phase.lock(), AppPhase::Installed) {
            return Err(OiError::new(
                ErrorCode::NotInstalled,
                format!("app is not installed: {app_name}"),
            ));
        }

        // spec: ACI#invoking-an-action - reject if script_error fault is active
        if entry.script_error.is_some() {
            return Err(OiError::script_error(format!(
                "app has a script error: {app_name}"
            )));
        }

        // spec: ACN#shells
        // Shells and actions have separate namespaces: a shell of the same
        // name does not stand in the way of the action.
        let def = entry.app.def.load();
        // spec: ACN#start-action
        if action_name == "start" {
            return Err(OiError::not_found(
                "'start' is a lifecycle action and cannot be manually invoked".to_string(),
            ));
        }
        let action_def = def
            .actions
            .get(action_name.as_str())
            .ok_or_else(|| OiError::not_found(format!("action not found: {action_name}")))?;

        // spec: ACI#invoking-an-action
        apply_action_param_schema(&action_def.params, &mut action_params)?;
        // spec: ACN#volume-params
        validate_volume_params(state, &action_def.params, &action_params)?;

        Arc::clone(&entry.phase)
    };

    // Operator-invoked action: source and target generation are equal to the
    // app's current generation at dispatch.
    let current_generation = {
        let reg = state.registry.read();
        reg.get(app_name.as_str())
            .map(|e| e.current_generation)
            .unwrap_or(0)
    };
    let (result, op_id_str) = {
        let mut sched = state.scheduler.lock();
        // spec: TRD#uninstall-admission
        // The phase read above was taken and released before this lock, so an
        // uninstall could have landed in between. The scheduler reads the
        // phase under its own lock and refuses an app being torn down, so the
        // decision is made where every admission passes rather than here.
        let result = sched.admit(
            app_name,
            action_name,
            action_params.clone(),
            current_generation,
            current_generation,
            "operator",
            &phase_handle,
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
                action_name.clone(),
                op_id,
                action_params,
                current_generation,
                current_generation,
                "operator".to_owned(),
                Some(Arc::clone(&ctx.events.actor)),
            );
            tracing::info!(app = %app_name, action = %action_name, schedule = "accepted", "invoke_action");
            Ok(json!({ "schedule": "accepted", "operation_id": op_id_str }))
        }
        ScheduleResult::Queued => {
            // spec: OPS#concurrency
            lifecycle::persist_queued(
                state,
                &crate::runtime::barrier::OperationId(op_id_str.clone()),
                Some(Arc::clone(&ctx.events.actor)),
            )?;
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
        ScheduleResult::Rejected(RejectReason::ChangeBeingApplied) => Err(OiError::new(
            ErrorCode::OperationInProgress,
            format!("a change to {app_name} is being applied; try again once it is applied"),
        )),
        // spec: TRD#uninstall-admission
        ScheduleResult::Rejected(RejectReason::AppUninstalling) => Err(OiError::new(
            ErrorCode::NotInstalled,
            format!("app is being uninstalled: {app_name}"),
        )),
        // spec: OPS#unreadable-operation-state
        ScheduleResult::Rejected(RejectReason::OperationStateUnreadable) => Err(OiError::internal(
            "the persisted operation state cannot be read, so no operation can start",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: ACN#param-delivery
    #[test]
    fn rejects_reserved_volume_suffix() {
        let mut params = serde_json::Map::new();
        params.insert("source_volume".into(), json!("anything"));
        let err = validate_action_params(&params).unwrap_err();
        assert!(matches!(err.code, ErrorCode::RequirementsInvalid));
    }

    // spec: ACN#param-delivery
    #[test]
    fn rejects_reserved_filename_suffix() {
        let mut params = serde_json::Map::new();
        params.insert("output_filename".into(), json!("anything"));
        let err = validate_action_params(&params).unwrap_err();
        assert!(matches!(err.code, ErrorCode::RequirementsInvalid));
    }

    #[test]
    fn accepts_unreserved_keys() {
        let mut params = serde_json::Map::new();
        params.insert("volume".into(), json!("ok"));
        params.insert("filename".into(), json!("ok"));
        params.insert("source".into(), json!("ok"));
        params.insert("backup".into(), json!({ "strategy": "s" }));
        validate_action_params(&params).unwrap();
    }

    use crate::oi::test_support::TestOi;

    // spec: ACI#invoking-an-action
    #[test]
    fn invoke_on_unknown_app_is_not_found() {
        let oi = TestOi::new();
        let (code, _) = oi
            .call(
                "/apps/action/invoke",
                json!({ "app": "ghost", "name": "backup" }),
            )
            .unwrap_err();
        assert_eq!(code, "not_found");
    }

    // spec: ACI#invoking-an-action
    #[test]
    fn invoke_on_uninstalled_app_is_rejected() {
        let oi = TestOi::with_app("demo");
        let (code, _) = oi
            .call(
                "/apps/action/invoke",
                json!({ "app": "demo", "name": "backup" }),
            )
            .unwrap_err();
        assert_eq!(code, "not_installed");
    }

    // spec: ACI#invoking-an-action
    #[test]
    fn invoke_unknown_action_on_installed_app_is_not_found() {
        let oi = TestOi::with_app("demo");
        oi.install("demo");
        let (code, message) = oi
            .call(
                "/apps/action/invoke",
                json!({ "app": "demo", "name": "nope" }),
            )
            .unwrap_err();
        assert_eq!(code, "not_found");
        assert!(message.contains("action not found"), "message: {message}");
    }

    // spec: ACN#start-action
    #[test]
    fn invoke_start_is_rejected() {
        let oi = TestOi::with_app("demo");
        oi.install("demo");
        let (code, message) = oi
            .call(
                "/apps/action/invoke",
                json!({ "app": "demo", "name": "start" }),
            )
            .unwrap_err();
        assert_eq!(code, "not_found");
        assert!(message.contains("lifecycle action"), "message: {message}");
    }

    // spec: ACN#param-delivery
    #[test]
    fn invoke_with_reserved_param_key_is_rejected() {
        let oi = TestOi::with_app("demo");
        oi.install("demo");
        let (code, _) = oi
            .call(
                "/apps/action/invoke",
                json!({
                    "app": "demo",
                    "name": "backup",
                    "params": { "dest_volume": "anything" },
                }),
            )
            .unwrap_err();
        assert_eq!(code, "requirements_invalid");
    }

    // spec: ACN#shells
    #[test]
    fn an_action_that_shares_a_shells_name_can_be_invoked() {
        let oi = TestOi::new();
        oi.call(
            "/apps/create",
            json!({
                "app": "demo",
                "script": r#"
                    app.deployment("web").image("docker.io/library/nginx:1.29");
                    app.on_action("tools", |rt, p| {});
                    app.on_shell("tools", |rt, shell, p| { shell.error("no"); });
                "#,
            }),
        )
        .unwrap();
        oi.install("demo");
        let result = oi
            .call(
                "/apps/action/invoke",
                json!({ "app": "demo", "name": "tools" }),
            )
            .expect("the action is reachable under the shell's name");
        assert_eq!(result["schedule"], "accepted");
    }

    // spec: ACI#cancellation
    #[test]
    fn cancel_without_active_operation_is_not_found() {
        let oi = TestOi::with_app("demo");
        let (code, message) = oi
            .call("/apps/action/cancel", json!({ "app": "demo" }))
            .unwrap_err();
        assert_eq!(code, "not_found");
        assert!(
            message.contains("no active operation"),
            "message: {message}"
        );
    }
}
