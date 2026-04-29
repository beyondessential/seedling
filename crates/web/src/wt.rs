use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::json;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::watch;
use uuid::Uuid;
use wtransport::tls::server::build_default_tls_config;
use wtransport::{Endpoint, ServerConfig, VarInt};

use crate::EventBroker;
use crate::state::AppState;
use crate::web_sessions::{HeartbeatOutcome, SafetyMode, WebSessionEntry};
use crate::{proxy, state};

/// Run a WebTransport server on `addr`, restarting when `rotation_rx` fires (cert rotated).
// w[transport.webtransport]
pub async fn run_wt_server(
    addr: SocketAddr,
    state: AppState,
    mut rotation_rx: watch::Receiver<()>,
) {
    loop {
        let identity = state.cert_store.read().current_identity();
        let mut tls = build_default_tls_config(identity);
        tls.key_log = Arc::new(rustls::KeyLogFile::new());
        let config = ServerConfig::builder()
            .with_bind_address(addr)
            .with_custom_tls(tls)
            .build();

        let endpoint = match Endpoint::server(config) {
            Ok(ep) => ep,
            Err(e) => {
                tracing::error!(%addr, "WT endpoint creation failed: {e}");
                return;
            }
        };

        tracing::info!(%addr, "WT server listening");

        loop {
            tokio::select! {
                incoming = endpoint.accept() => {
                    let state2 = state.clone();
                    tokio::spawn(async move {
                        handle_incoming(incoming, state2).await;
                    });
                }
                _ = rotation_rx.changed() => {
                    tracing::info!(%addr, "WT cert rotated — restarting endpoint");
                    endpoint.close(VarInt::from_u32(0), b"cert rotation");
                    break;
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

    // w[wt.actor] — resolve actor from the single-use token embedded in the URL.
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

    // w[impl sessions.events]
    let session_id = Uuid::new_v4();
    let connected_at = jiff::Timestamp::now();
    // w[impl sessions.safety-mode]
    // Sessions register as `read` until the browser's first heartbeat reports
    // its current mode (the elevation is sessionStorage-backed, so a refresh
    // can resume in `write` or `dangerous`). This matches the server-side
    // default and means peers can never see an elevated mode that the
    // operator hasn't actually opted into for *this* WT session.
    let initial_mode = SafetyMode::default();
    state.web_sessions.insert(WebSessionEntry {
        id: session_id,
        connected_at,
        // w[impl sessions.stale-cutoff]
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

    // w[transport.webtransport]
    // w[routes.events]
    // w[routes.logs]
    // w[shells.wire]
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

            // w[impl sessions.heartbeat]
            if peeked.method == "/connected-clients/heartbeat" {
                let now = jiff::Timestamp::now();
                // w[impl sessions.safety-mode]
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
                        // w[impl sessions.safety-mode]
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

            // w[impl routes.sessions]
            // w[impl sessions.actor-activity]
            if peeked.method == "/connected-clients/list" {
                let web = state3.web_sessions.list();
                let shells = match state3.daemon.request("/shells/list", json!({})).await {
                    Ok(v) => v.get("shells").cloned().unwrap_or(json!([])),
                    Err(_) => json!([]),
                };
                let forwards = match state3.daemon.request("/forwards/list", json!({})).await {
                    Ok(v) => v.get("forwards").cloned().unwrap_or(json!([])),
                    Err(_) => json!([]),
                };
                let actors = state3.actor_activity.list_recent();
                let response = json!({
                    "result": {
                        "web": web,
                        "shells": shells,
                        "forwards": forwards,
                        "actors": actors,
                    }
                });
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
            // w[shells.wire] w[volumes.shell-ui]
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
                        let msg = serde_json::json!({
                            "error": { "code": "daemon_unavailable", "message": e.to_string() }
                        });
                        let _ = wt_send.write_all((msg.to_string() + "\n").as_bytes()).await;
                        let _ = wt_send.shutdown().await;
                    }
                }
                return;
            }

            match state3.daemon.open_bi().await {
                Ok((daemon_send, daemon_recv)) => {
                    let mut daemon_send = daemon_send;
                    let mut daemon_recv = daemon_recv;
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

    state.web_sessions.remove(&session_id);
    // w[impl sessions.events]
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

// w[impl sessions.safety-mode]
/// Extract the optional `safety_mode` field from a heartbeat request body.
/// `peeked_line` is the JSON wire bytes after actor injection — the
/// `safety_mode` value sits under `params`. Unknown or absent values fall back
/// to `read` so a malformed client can never silently elevate the recorded
/// mode for its session.
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
// w[impl sessions.stale-cutoff]
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
// w[wt.cert.rotation]
pub async fn run_cert_rotation(
    cert_store: Arc<parking_lot::RwLock<crate::wt_cert::CertStore>>,
    rotation_tx: watch::Sender<()>,
) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        let rotated = cert_store.write().rotate_if_needed();
        if rotated {
            let _ = rotation_tx.send(());
        }
    }
}
