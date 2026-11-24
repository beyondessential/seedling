use std::sync::Arc;

use serde::Deserialize;

use seedling_protocol::error::{ErrorCode, OiError};

use crate::{
    oi::{redaction::SensitiveByApp, state::OiState},
    runtime::redact::SensitiveValues,
    system::journal::{InfraComponent, LogEntry, LogStreamOptions, LogTarget},
};

#[derive(Debug, Deserialize)]
pub(crate) struct LogStreamParams {
    app: Option<String>,
    resource: Option<String>,
    instance: Option<String>,
    infra: Option<String>,
    #[serde(default)]
    follow: bool,
    #[serde(default = "default_tail")]
    tail: u64,
}

fn default_tail() -> u64 {
    100
}

/// Validate log stream params and produce a `LogStreamOptions`, or return an OI error.
// spec: LOG#log-stream-request
pub(crate) fn validate_params(
    state: &Arc<OiState>,
    params: LogStreamParams,
) -> Result<LogStreamOptions, OiError> {
    let target = match (params.app, params.infra) {
        (Some(app), None) => {
            // spec: LOG#log-stream-request
            {
                let reg = state.registry.read();
                if !reg.is_registered(&app) {
                    return Err(OiError::not_found(format!("app not found: {app}")));
                }
            }
            if params.instance.is_some() && params.resource.is_none() {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    "instance requires resource",
                ));
            }
            LogTarget::App {
                app,
                resource: params.resource,
                instance: params.instance,
            }
        }
        (None, Some(infra)) => {
            if params.resource.is_some() || params.instance.is_some() {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    "resource/instance not valid with infra target",
                ));
            }
            let component = match infra.as_str() {
                "proxy" => InfraComponent::Proxy,
                "resolver" => InfraComponent::Resolver,
                other => {
                    return Err(OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("unknown infra component: {other}"),
                    ));
                }
            };
            LogTarget::Infra(component)
        }
        (Some(_), Some(_)) => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                "app and infra are mutually exclusive",
            ));
        }
        (None, None) => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                "one of app or infra is required",
            ));
        }
    };

    Ok(LogStreamOptions {
        target,
        // spec: LOG#entry-selection
        follow: params.follow,
        // spec: LOG#entry-selection
        tail: params.tail,
    })
}

// spec: SEC#matching
// spec: ACTU#breadcrumbs
/// Mask a breadcrumb against `sensitive`, the values of its app as they stand
/// now, which a value it recorded in the clear may since have joined.
/// Container output is relayed as it was written.
fn mask_breadcrumb(entry: &mut LogEntry, sensitive: &SensitiveValues) {
    if entry.rt_call.is_some()
        && let std::borrow::Cow::Owned(masked) = sensitive.mask(&entry.message)
    {
        entry.message = masked;
    }
}

/// Stream log entries from the journal to a QUIC unidirectional stream.
/// Called after the bidi handshake is complete.
// spec: LOG#log-stream-request
// spec: LOG#log-entries
pub(crate) async fn log_stream_task(
    state: Arc<OiState>,
    mut send: quinn::SendStream,
    mut rx: tokio::sync::mpsc::Receiver<LogEntry>,
) {
    let mut sensitive = SensitiveByApp::new(&state);
    while let Some(mut entry) = rx.recv().await {
        if entry.rt_call.is_some()
            && let Some(app) = entry.app.clone()
        {
            match sensitive.of(&state, &app) {
                Ok(values) => mask_breadcrumb(&mut entry, values),
                // spec: SEC#redaction
                // A breadcrumb that cannot be matched is not served in the
                // clear.
                Err(e) => {
                    tracing::warn!(%app, "cannot read the app's sensitive values for its log: {}", e.message);
                    entry.message = crate::runtime::redact::REDACTED.to_owned();
                }
            }
        }
        let mut bytes = match serde_json::to_vec(&entry) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("log entry serialisation error: {e}");
                continue;
            }
        };
        bytes.push(b'\n');
        if let Err(e) = send.write_all(&bytes).await {
            tracing::debug!("log stream closed: {e}");
            break;
        }
    }
    let _ = send.finish();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(message: &str, rt_call: Option<&str>) -> LogEntry {
        LogEntry {
            timestamp: String::new(),
            message: message.to_owned(),
            unit: String::new(),
            stream: "stdout".to_owned(),
            app: Some("demo".to_owned()),
            resource_kind: None,
            resource: None,
            instance: None,
            infra: None,
            rt_call: rt_call.map(str::to_owned),
            script_pos: None,
        }
    }

    // spec: SEC#matching
    #[test]
    fn a_breadcrumb_is_masked_against_values_current_when_served() {
        let mut sensitive = SensitiveValues::default();
        sensitive.insert("Quokka-Ledger-77");
        let mut crumb = entry(
            "rt.exec(db, [psql, postgres://app:Quokka-Ledger-77@db])",
            Some("exec"),
        );
        mask_breadcrumb(&mut crumb, &sensitive);
        assert_eq!(
            crumb.message,
            "rt.exec(db, [psql, postgres://app:<redacted>@db])"
        );
    }

    // spec: SEC#matching
    #[test]
    fn a_value_that_becomes_sensitive_mid_stream_is_masked_from_then_on() {
        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "demo",
                "script": r#"
                    app.param("db-password").kind("password").required(false);
                    app.deployment("web").image("docker.io/library/nginx:1.29");
                "#,
            }),
        )
        .unwrap();
        let mut sensitive = SensitiveByApp::new(&oi.state);
        let line = "rt.exec(db, [psql, postgres://app:Kiwi-Anchor-63@db])";
        let served = |sensitive: &mut SensitiveByApp| {
            oi.block_on(async {
                let mut crumb = entry(line, Some("exec"));
                mask_breadcrumb(&mut crumb, sensitive.of(&oi.state, "demo").unwrap());
                crumb.message
            })
        };
        assert_eq!(served(&mut sensitive), line);

        oi.call(
            "/apps/params/set",
            serde_json::json!({ "app": "demo", "params": { "db-password": "Kiwi-Anchor-63" } }),
        )
        .unwrap();
        assert_eq!(
            served(&mut sensitive),
            "rt.exec(db, [psql, postgres://app:<redacted>@db])"
        );
    }

    // spec: SEC#redaction
    #[test]
    fn container_output_is_relayed_as_written() {
        let mut sensitive = SensitiveValues::default();
        sensitive.insert("Quokka-Ledger-77");
        let mut output = entry("password is Quokka-Ledger-77", None);
        mask_breadcrumb(&mut output, &sensitive);
        assert_eq!(output.message, "password is Quokka-Ledger-77");
    }
}
