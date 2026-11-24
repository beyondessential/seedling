//! The check of each whole response and event the Operator Interface sends,
//! which masks a sensitive value wherever no narrower rule withheld it.

use std::{collections::HashMap, sync::Arc};

use seedling_protocol::{
    error::{HandlerResult, OiError},
    events::OiEvent,
    names::AppName,
};
use serde_json::Value;
use tokio::sync::broadcast::{Receiver, error::TryRecvError};

use crate::{
    oi::state::OiState,
    runtime::redact::{SensitiveValues, mask_document},
};

/// The methods that return a definition or a template as stored.
// spec: SEC#responses-and-events
const RETURNED_AS_STORED: &[&str] = &["/apps/script", "/apps/bundle", "/templates/show"];

/// The sensitive values of `app`, or none for a name that is no app.
fn values_of(state: &OiState, app: &str) -> Result<SensitiveValues, OiError> {
    match AppName::new(app) {
        Ok(name) => crate::oi::handler::current_sensitive(state, &name),
        Err(_) => Ok(SensitiveValues::default()),
    }
}

// spec: SEC#responses-and-events
/// Check the response to `method`, whose request named `app`, before it is
/// sent: mask each sensitive value in it, in its result or its error message.
/// A response whose sensitive values cannot be read is not sent; an internal
/// error is sent in its place.
pub(crate) fn withhold_response(
    state: &OiState,
    method: &str,
    app: Option<&str>,
    result: HandlerResult,
) -> HandlerResult {
    if RETURNED_AS_STORED.contains(&method) {
        return result;
    }
    let mut cache: HashMap<String, SensitiveValues> = HashMap::new();
    let mut values_for = |app: &str| -> Result<SensitiveValues, OiError> {
        if let Some(values) = cache.get(app) {
            return Ok(values.clone());
        }
        let values = values_of(state, app)?;
        cache.insert(app.to_owned(), values.clone());
        Ok(values)
    };
    match result {
        Ok(mut value) => {
            mask_document(&mut value, app, &mut values_for)?;
            Ok(value)
        }
        Err(mut e) => {
            if let Some(app) = app {
                e.message = values_for(app)?.mask(&e.message).into_owned();
            }
            Err(e)
        }
    }
}

// spec: SEC#responses-and-events
/// `event` as it is sent, with each sensitive value of the app it names
/// masked, or `None` when the app's sensitive values cannot be read, so that
/// it is not sent at all.
pub(crate) fn withhold_event(
    sensitive: &mut SensitiveByApp,
    state: &OiState,
    event: &OiEvent,
) -> Option<Value> {
    masked_event(sensitive, state, event)
        .inspect_err(|e| tracing::warn!("an event is withheld: {e}"))
        .ok()
}

/// `event` with each sensitive value of the app it names masked, or why it
/// cannot be.
fn masked_event(
    sensitive: &mut SensitiveByApp,
    state: &OiState,
    event: &OiEvent,
) -> Result<Value, String> {
    let mut value =
        serde_json::to_value(event).map_err(|e| format!("event serialisation error: {e}"))?;
    let mut values_for = |app: &str| sensitive.of(state, app).cloned();
    mask_document(&mut value, None, &mut values_for)
        .map_err(|e| format!("its app's sensitive values cannot be read: {}", e.message))?;
    Ok(value)
}

// spec: AUD#entries
// spec: SEC#responses-and-events
/// The audit log's entry for each event: the event as the event feed sends
/// it, or why it cannot be written.
pub fn audit_entries(
    state: Arc<OiState>,
) -> impl FnMut(&OiEvent) -> Result<Value, String> + Send + 'static {
    let mut sensitive = SensitiveByApp::new(&state);
    move |event| masked_event(&mut sensitive, &state, event)
}

/// The sensitive values of each app a stream of log entries or events
/// serves, read again once a change to the app's parameters, definition or
/// seed is announced, so that a long stream picks up a value that became
/// sensitive.
pub(crate) struct SensitiveByApp {
    values: HashMap<String, SensitiveValues>,
    changes: Receiver<OiEvent>,
}

impl SensitiveByApp {
    pub(crate) fn new(state: &OiState) -> Self {
        Self {
            values: HashMap::new(),
            changes: state.event_tx.subscribe(),
        }
    }

    /// Forget the values of every app whose sensitive values a change since
    /// the last call may have moved, and of every app where changes were
    /// missed.
    fn forget_changed(&mut self) {
        loop {
            match self.changes.try_recv() {
                Ok(
                    OiEvent::ParamsChanged { app, .. }
                    | OiEvent::AppUpdated { app, .. }
                    | OiEvent::AppSeedRotated { app, .. }
                    | OiEvent::AppRegistered { app, .. },
                ) => {
                    self.values.remove(app.as_str());
                }
                Ok(_) => {}
                Err(TryRecvError::Lagged(_)) => self.values.clear(),
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
    }

    pub(crate) fn of(&mut self, state: &OiState, app: &str) -> Result<&SensitiveValues, OiError> {
        self.forget_changed();
        if !self.values.contains_key(app) {
            let values = match AppName::new(app) {
                Ok(name) => tokio::task::block_in_place(|| {
                    crate::oi::handler::current_sensitive(state, &name)
                })?,
                Err(_) => SensitiveValues::default(),
            };
            self.values.insert(app.to_owned(), values);
        }
        Ok(&self.values[app])
    }
}

#[cfg(test)]
mod tests {
    use seedling_protocol::error::ErrorCode;
    use serde_json::json;

    use super::*;
    use crate::oi::test_support::TestOi;

    const SCRIPT: &str = r#"
        app.param("token").kind("password").required(false).default_value("Hoiho-Quarry-35");
        app.deployment("web").image("docker.io/library/nginx:1.29");
    "#;

    fn oi() -> TestOi {
        let oi = TestOi::new();
        oi.call("/apps/create", json!({ "app": "demo", "script": SCRIPT }))
            .unwrap();
        oi
    }

    // spec: SEC#responses-and-events
    #[test]
    fn a_response_and_an_error_are_masked_for_the_app_they_name() {
        let oi = oi();
        let masked = withhold_response(
            &oi.state,
            "/apps/show",
            Some("demo"),
            Ok(json!({ "name": "Hoiho-Quarry-35", "note": "token Hoiho-Quarry-35" })),
        )
        .unwrap();
        assert_eq!(
            masked,
            json!({ "name": "Hoiho-Quarry-35", "note": "token <redacted>" })
        );

        let error = withhold_response(
            &oi.state,
            "/apps/action/invoke",
            Some("demo"),
            Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                "bad value Hoiho-Quarry-35",
            )),
        )
        .unwrap_err();
        assert_eq!(error.message, "bad value <redacted>");
    }

    // spec: AUD#entries
    // spec: SEC#responses-and-events
    #[test]
    fn an_audit_entry_masks_a_sensitive_value_the_event_quotes() {
        let oi = oi();
        let mut entries = audit_entries(std::sync::Arc::clone(&oi.state));
        let event = OiEvent::FaultFiled {
            timestamp: jiff::Timestamp::now(),
            id: "f1".into(),
            app: AppName::new("demo").unwrap(),
            resource_type: None,
            resource_name: None,
            instance_id: None,
            kind: "probe_failed".into(),
            description: "login with Hoiho-Quarry-35 refused".into(),
            actor: None,
        };
        let entry = oi.block_on(async { entries(&event) }).unwrap();
        assert_eq!(entry["description"], "login with <redacted> refused");
    }

    // spec: SEC#responses-and-events
    #[test]
    fn a_definition_is_returned_as_stored() {
        let oi = oi();
        let script = oi.call("/apps/script", json!({ "app": "demo" })).unwrap();
        assert!(
            script["script"]
                .as_str()
                .unwrap()
                .contains("Hoiho-Quarry-35"),
            "{script}"
        );
    }
}
