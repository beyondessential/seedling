use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::json;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::watch;
use uuid::Uuid;
use wtransport::tls::server::build_default_tls_config;
use wtransport::{Endpoint, ServerConfig, VarInt};

use crate::state::AppState;
use crate::web_sessions::WebSessionEntry;
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
    state.web_sessions.insert(WebSessionEntry {
        id: session_id,
        connected_at,
        actor: Arc::clone(&actor),
    });
    state
        .event_broker
        .publish(Arc::from(
            json!({
                "type": "WebSessionStarted",
                "timestamp": connected_at.to_string(),
                "session_id": session_id.to_string(),
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

            // w[impl routes.sessions]
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
                let response = json!({
                    "result": { "web": web, "shells": shells, "forwards": forwards }
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

            // Intercept /shells/start: the daemon opens server-initiated uni
            // streams (stdout, stderr) that must be forwarded as WT uni streams
            // with an 8-byte BE stream ID prefix so the browser can demux them.
            // w[shells.wire]
            if peeked.method == "/shells/start" {
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
