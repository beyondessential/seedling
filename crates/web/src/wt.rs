use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::watch;
use wtransport::{Endpoint, ServerConfig, VarInt};

use crate::proxy;
use crate::state::{self, AppState};

/// Run a WebTransport server on `addr`, restarting when `rotation_rx` fires (cert rotated).
// w[transport.webtransport]
pub async fn run_wt_server(
    addr: SocketAddr,
    state: AppState,
    mut rotation_rx: watch::Receiver<()>,
) {
    loop {
        let identity = state.cert_store.read().current_identity();
        let config = ServerConfig::builder()
            .with_bind_address(addr)
            .with_identity(identity)
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

    // w[transport.webtransport]
    while let Ok((wt_send, wt_recv)) = conn.accept_bi().await {
        let state3 = state.clone();
        let actor2 = Arc::clone(&actor);
        tokio::spawn(async move {
            match state3.daemon.open_bi().await {
                Ok((daemon_send, daemon_recv)) => {
                    proxy::proxy_stream(wt_send, wt_recv, daemon_send, daemon_recv, actor2).await;
                }
                Err(e) => {
                    tracing::error!("daemon stream open failed: {e}");
                    let mut wt_send = wt_send;
                    let _ = tokio::io::AsyncWriteExt::write_all(
                        &mut wt_send,
                        b"{\"error\":{\"code\":\"daemon_unavailable\",\"message\":\"daemon connection failed\"}}\n",
                    ).await;
                }
            }
        });
    }
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
