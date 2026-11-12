use std::net::SocketAddr;
use std::sync::Arc;

use seedling_protocol::client::ClientError;
use serde_json::json;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::watch;
use uuid::Uuid;
use wtransport::endpoint::endpoint_side::Server;
use wtransport::tls::server::build_default_tls_config;
use wtransport::{Endpoint, ServerConfig};

use crate::EventBroker;
use crate::state::AppState;
use crate::web_sessions::{HeartbeatOutcome, SafetyMode, WebSessionEntry};
use crate::{proxy, state};

fn server_config(addr: SocketAddr, state: &AppState) -> ServerConfig {
    let identity = state.cert_store.read().current_identity();
    let mut tls = build_default_tls_config(identity);
    tls.key_log = Arc::new(rustls::KeyLogFile::new());
    ServerConfig::builder()
        .with_bind_address(addr)
        .with_custom_tls(tls)
        .build()
}

/// Bind the WebTransport endpoint for `addr` with the current certificate.
// spec: WEB#listen-addresses
pub fn bind_wt_endpoint(addr: SocketAddr, state: &AppState) -> std::io::Result<Endpoint<Server>> {
    Endpoint::server(server_config(addr, state))
}

/// Serve WebTransport sessions on a bound endpoint, presenting the new
/// certificate to new sessions whenever `rotation_rx` fires.
// spec: WEB#request-relay
pub async fn run_wt_server(
    endpoint: Endpoint<Server>,
    addr: SocketAddr,
    state: AppState,
    mut rotation_rx: watch::Receiver<()>,
) {
    tracing::info!(%addr, "WT server listening");
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let state2 = state.clone();
                tokio::spawn(async move {
                    handle_incoming(incoming, state2).await;
                });
            }
            Ok(()) = rotation_rx.changed() => {
                // spec: WEB#webtransport-certificate
                // Without a rebind the socket is kept, so this swaps the TLS
                // configuration in place and established sessions carry on.
                // Only rebinding can fail, so the error arm guards against a
                // wtransport upgrade changing that rather than a known path.
                match endpoint.reload_config(server_config(addr, &state), false) {
                    Ok(()) => tracing::info!(%addr, "WT certificate rotated"),
                    Err(e) => tracing::error!(%addr, "WT certificate reload failed: {e}"),
                }
            }
        }
    }
}

async fn handle_incoming(incoming: wtransport::endpoint::IncomingSession, state: AppState) {
    let session_request = match incoming.await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("WT incoming session error: {e}");
            return;
        }
    };

    let path = session_request.path().to_owned();

    // Only accept /wt path.
    if !path.starts_with("/wt") {
        session_request.not_found().await;
        return;
    }

    // spec: WEB#session-establishment — resolve actor from the single-use token embedded in the URL.
    let token = extract_query_param(&path, "t");
    let actor = match token.and_then(|t| state::consume_wt_token(&state.wt_tokens, &t)) {
        Some(a) => a,
        None => {
            tracing::debug!("WT session rejected: missing or invalid token");
            session_request.forbidden().await;
            return;
        }
    };

    let conn = match session_request.accept().await {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("WT session accept error: {e}");
            return;
        }
    };

    tracing::info!(
        actor_kind = ?actor.kind,
        actor_id = ?actor.id,
        "WT session established"
    );

    // spec: WEB#session-tracking
    let session_id = Uuid::new_v4();
    let connected_at = jiff::Timestamp::now();
    // spec: UI#guarded-controls
    // Sessions register as `read` until the browser's first heartbeat reports
    // its current mode (the elevation is sessionStorage-backed, so a refresh
    // can resume in `write` or `dangerous`). This matches the server-side
    // default and means peers can never see an elevated mode that the
    // operator hasn't actually opted into for *this* WT session.
    let initial_mode = SafetyMode::default();
    state.web_sessions.insert(WebSessionEntry {
        id: session_id,
        connected_at,
        // spec: WEB#session-tracking
        last_seen: connected_at,
        actor: Arc::clone(&actor),
        safety_mode: initial_mode,
    });
    state
        .event_broker
        .publish(Arc::from(
            json!({
                "type": "WebSessionStarted",
                "timestamp": connected_at.to_string(),
                "session_id": session_id.to_string(),
                "safety_mode": initial_mode.as_str(),
            })
            .to_string()
            .as_str(),
        ))
        .await;

    // spec: WEB#request-relay
    // spec: UI#event-feed
    // spec: UI#operator-capabilities
    // spec: WEB#shell-relay
    while let Ok((mut wt_send, wt_recv)) = conn.accept_bi().await {
        let state3 = state.clone();
        let actor2 = Arc::clone(&actor);
        let conn2 = conn.clone();
        tokio::spawn(async move {
            let peeked = match proxy::peek_request(wt_recv, &actor2).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::debug!("peek request failed: {e}");
                    return;
                }
            };

            // spec: WEB#session-tracking
            if peeked.method == "/connected-clients/heartbeat" {
                let now = jiff::Timestamp::now();
                // spec: UI#guarded-controls
                let reported_mode = parse_heartbeat_mode(&peeked.modified_line);
                let outcome = state3.web_sessions.touch(&session_id, now, reported_mode);
                if let HeartbeatOutcome::Alive {
                    mode_change: Some(new_mode),
                } = outcome
                {
                    state3
                        .event_broker
                        .publish(Arc::from(
                            json!({
                                "type": "WebSessionModeChanged",
                                "timestamp": now.to_string(),
                                "session_id": session_id.to_string(),
                                "safety_mode": new_mode.as_str(),
                            })
                            .to_string()
                            .as_str(),
                        ))
                        .await;
                }
                let response = json!({
                    "result": {
                        "alive": outcome.alive(),
                        "now": now.to_string(),
                        // spec: UI#guarded-controls
                        // Echoing the session id lets the browser identify
                        // its own row in /connected-clients/list and skip
                        // it when computing peer-elevation warnings.
                        "session_id": session_id.to_string(),
                    }
                });
                let _ = wt_send
                    .write_all((response.to_string() + "\n").as_bytes())
                    .await;
                let _ = wt_send.shutdown().await;
                return;
            }

            // spec: WEB#connected-clients
            if peeked.method == "/connected-clients/list" {
                let web = state3.web_sessions.list();
                let shells = listed(
                    state3.daemon.request("/shells/list", json!({})).await,
                    "shells",
                );
                let forwards = listed(
                    state3.daemon.request("/forwards/list", json!({})).await,
                    "forwards",
                );
                let actors = state3.actor_activity.list_recent();
                let response = match (shells, forwards) {
                    (Ok(shells), Ok(forwards)) => json!({
                        "result": {
                            "web": web,
                            "shells": shells,
                            "forwards": forwards,
                            "actors": actors,
                        }
                    }),
                    // spec: WEB#connected-clients
                    // An empty list would claim nothing is connected.
                    (Err(e), _) | (_, Err(e)) => json!({
                        "error": { "code": "daemon_unavailable", "message": e }
                    }),
                };
                let _ = wt_send
                    .write_all((response.to_string() + "\n").as_bytes())
                    .await;
                let _ = wt_send.shutdown().await;
                return;
            }

            if peeked.method == "/events/subscribe" {
                let _ = wt_send.write_all(b"{\"result\":{}}\n").await;
                state3.event_broker.serve_client(wt_send).await;
                return;
            }

            // Intercept /shells/start and /volumes/shell: the daemon opens
            // server-initiated uni streams (stdout, stderr) that must be
            // forwarded as WT uni streams with an 8-byte BE stream ID prefix so
            // the browser can demux them.
            // spec: WEB#shell-relay, UI#shells
            if peeked.method == "/shells/start" || peeked.method == "/volumes/shell" {
                crate::shell::handle_shell_start(state3, conn2, wt_send, peeked).await;
                return;
            }

            // Intercept /logs/stream: the daemon opens a server-initiated uni
            // stream to push entries, which the transparent proxy cannot forward.
            // Accept that uni stream here and relay entries over the WT bidi send.
            if peeked.method == "/logs/stream" {
                match state3.daemon.start_log_stream(&peeked.modified_line).await {
                    Ok((_log_client, mut log_recv)) => {
                        let _ = wt_send.write_all(b"{\"result\":{}}\n").await;
                        // Only send FIN on clean daemon EOF; on client abort (STOP_SENDING)
                        // just drop wt_send to send RESET_STREAM instead of an invalid FIN.
                        if tokio::io::copy(&mut log_recv, &mut wt_send).await.is_ok() {
                            let _ = wt_send.shutdown().await;
                        }
                        // _log_client dropped here — dedicated connection closed cleanly.
                    }
                    Err(e) => {
                        tracing::error!("log stream setup failed: {e}");
                        // A rejection by the daemon is the daemon's answer, not
                        // a gateway failure: relay its code and message so the
                        // browser can tell "no such app" from "daemon down".
                        let msg = match &e {
                            ClientError::Api { code, message } => serde_json::json!({
                                "error": { "code": code, "message": message }
                            }),
                            _ => serde_json::json!({
                                "error": { "code": "daemon_unavailable", "message": e.to_string() }
                            }),
                        };
                        let _ = wt_send.write_all((msg.to_string() + "\n").as_bytes()).await;
                        let _ = wt_send.shutdown().await;
                    }
                }
                return;
            }

            match state3.daemon.open_bi().await {
                Ok(crate::daemon::DaemonBi {
                    send: mut daemon_send,
                    recv: mut daemon_recv,
                    ..
                }) => {
                    if let Err(e) = proxy::proxy_from_peeked(
                        &mut wt_send,
                        peeked,
                        &mut daemon_send,
                        &mut daemon_recv,
                    )
                    .await
                    {
                        tracing::debug!("proxy stream: {e}");
                    }
                }
                Err(e) => {
                    tracing::error!("daemon stream open failed: {e}");
                    let _ = wt_send
                        .write_all(b"{\"error\":{\"code\":\"daemon_unavailable\",\"message\":\"daemon connection failed\"}}\n")
                        .await;
                    let _ = wt_send.shutdown().await;
                }
            }
        });
    }

    // Only the removal that actually took the session out announces it: the
    // reaper may already have dropped this session and published its stop,
    // and a second event for the same id makes a client see two closes.
    if state.web_sessions.remove(&session_id) {
        // spec: WEB#session-tracking
        state
            .event_broker
            .publish(Arc::from(
                json!({
                    "type": "WebSessionStopped",
                    "timestamp": jiff::Timestamp::now().to_string(),
                    "session_id": session_id.to_string(),
                })
                .to_string()
                .as_str(),
            ))
            .await;
    }
}

// spec: UI#guarded-controls
/// Extract the optional `safety_mode` field from a heartbeat request body.
/// `peeked_line` is the JSON wire bytes after actor injection — the
/// `safety_mode` value sits under `params`. Unknown or absent values fall back
/// to `read` so a malformed client can never silently elevate the recorded
/// mode for its session.
/// The `member` array of a daemon list response, or why it could not be read.
// spec: WEB#connected-clients
fn listed(
    response: Result<serde_json::Value, ClientError>,
    member: &str,
) -> Result<serde_json::Value, String> {
    let value = response.map_err(|e| format!("cannot read {member} from the daemon: {e}"))?;
    match value.get(member) {
        Some(list @ serde_json::Value::Array(_)) => Ok(list.clone()),
        _ => Err(format!(
            "the daemon's {member} list holds no {member} array"
        )),
    }
}

fn parse_heartbeat_mode(peeked_line: &[u8]) -> Option<SafetyMode> {
    let value: serde_json::Value = serde_json::from_slice(peeked_line).ok()?;
    let raw = value.get("params")?.get("safety_mode")?.as_str();
    Some(SafetyMode::parse(raw))
}

fn extract_query_param(path_with_query: &str, key: &str) -> Option<String> {
    let query = path_with_query.split_once('?')?.1;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == key
        {
            return Some(v.to_owned());
        }
    }
    None
}

/// Background task: drop web sessions whose `last_seen` has aged past the
/// stale cutoff and emit `WebSessionStopped` events for each so other clients
/// see the change without polling.
// spec: WEB#session-tracking
pub async fn run_session_reaper(
    sessions: Arc<crate::web_sessions::WebSessionRegistry>,
    event_broker: Arc<EventBroker>,
) {
    let mut ticker = tokio::time::interval(crate::web_sessions::REAPER_TICK);
    // Skip the immediate first tick — fresh sessions can never be stale at
    // startup and the message is just noise.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let now = jiff::Timestamp::now();
        let stale = sessions.reap_stale(now);
        for id in stale {
            tracing::info!(session_id = %id, "reaped stale web session");
            event_broker
                .publish(Arc::from(
                    json!({
                        "type": "WebSessionStopped",
                        "timestamp": now.to_string(),
                        "session_id": id.to_string(),
                    })
                    .to_string()
                    .as_str(),
                ))
                .await;
        }
    }
}

/// Background task: check for cert rotation every hour.
// spec: WEB#webtransport-certificate
pub async fn run_cert_rotation(
    cert_store: Arc<parking_lot::RwLock<crate::wt_cert::CertStore>>,
    rotation_tx: watch::Sender<()>,
) {
    loop {
        tokio::time::sleep(crate::wt_cert::ROTATION_TICK).await;
        let rotated = cert_store.write().rotate_if_needed();
        if rotated {
            let _ = rotation_tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // spec: WEB#connected-clients
    #[test]
    fn a_list_the_daemon_could_not_give_is_an_error_not_an_empty_list() {
        assert_eq!(
            listed(Ok(json!({ "shells": [1] })), "shells").unwrap(),
            json!([1])
        );
        assert!(listed(Ok(json!({})), "shells").is_err());
        assert!(listed(Err(ClientError::Protocol("closed".into())), "forwards").is_err());
    }
}
