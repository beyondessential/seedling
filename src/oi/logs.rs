use std::sync::Arc;

use serde::Deserialize;

use crate::{
    oi::{
        error::{ErrorCode, OiError},
        state::OiState,
    },
    system::journal::{InfraComponent, LogStreamOptions, LogTarget},
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
pub(crate) fn validate_params(
    state: &Arc<OiState>,
    params: LogStreamParams,
) -> Result<LogStreamOptions, OiError> {
    let target = match (params.app, params.infra) {
        (Some(app), None) => {
            // i[logs.not-found]
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
        follow: params.follow,
        tail: params.tail,
    })
}

/// Stream log entries from the journal to a QUIC unidirectional stream.
/// Called after the bidi handshake is complete.
// i[logs.stream]
pub(crate) async fn log_stream_task(
    mut send: quinn::SendStream,
    mut rx: tokio::sync::mpsc::Receiver<crate::system::journal::LogEntry>,
) {
    while let Some(entry) = rx.recv().await {
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
