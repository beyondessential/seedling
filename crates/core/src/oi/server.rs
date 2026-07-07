use std::{path::Path, sync::Arc};

use serde_json::json;
use tokio::sync::{Semaphore, mpsc};
use tracing::Instrument;

use seedling_protocol::{OI_ALPN, actor::Actor, events::EventSenderWithActor};

use super::{
    forwards::session::{forward_port_session, handle_forward_stream},
    handler::{RequestCtx, dispatch},
    shells::{open_shell_session, open_volume_shell_session},
    state::OiState,
};
use crate::transport::{
    TransportState,
    endpoint::{
        ConnectionContext, ConnectionFuture, ConnectionHandler, LabelLookup, extract_client_fp,
        read_json_line,
    },
};

/// Default maximum number of concurrently active OI bidirectional streams
/// per connection. Per-protocol; grove will set its own.
pub const DEFAULT_MAX_STREAMS: usize = 64;

/// Maximum size of a request body read (4 MiB).
const MAX_REQUEST_SIZE: usize = 4 * 1024 * 1024;

// i[wire.actor]
fn synthesise_actor(state: &OiState, fp: Option<&str>) -> Arc<Actor> {
    let id = fp.map(str::to_owned);
    let display = fp
        .and_then(|f| {
            let f = f.to_owned();
            state
                .db
                .call(move |db| super::auth::get_label(db, &f).ok().flatten())
        })
        .or_else(|| id.clone());
    Arc::new(Actor {
        kind: Some("ctl".to_owned()),
        id,
        display,
        session: None,
    })
}

/// Register the OI as the `bes.seedling/1` ALPN handler against the shared
/// transport state. Loads the authorised-keys set from the DB (and the
/// bootstrap file), installs the per-connection log-label resolver,
/// builds the OI connection handler, and registers both with
/// [`TransportState`].
///
/// The actual QUIC endpoint is started by
/// [`crate::transport::endpoint::run`] after every protocol has registered.
pub fn register(
    transport: &Arc<TransportState>,
    state: &Arc<OiState>,
    data_dir: &Path,
    max_streams: usize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    {
        let trusted_keys = Arc::clone(&state.trusted_keys);
        state
            .db
            .call(move |db| super::auth::load_from_db(db, &trusted_keys))
            .map_err(|e| format!("loading authorized keys: {e}"))?;
        let trusted_keys = Arc::clone(&state.trusted_keys);
        let data_dir = data_dir.to_owned();
        state
            .db
            .call(move |db| super::auth::import_bootstrap_file(&data_dir, db, &trusted_keys))
            .map_err(|e| format!("reading bootstrap file: {e}"))?;
    }

    if state.trusted_keys.read().is_empty() {
        tracing::warn!(
            "no authorized client keys — add fingerprints to {}/authorized_keys and restart",
            data_dir.display()
        );
    }

    transport
        .trust_registry
        .register(OI_ALPN, Arc::clone(&state.trusted_keys));

    let stream_semaphore = Arc::new(Semaphore::new(max_streams));
    let oi_handler: ConnectionHandler = {
        let state = Arc::clone(state);
        Arc::new(move |conn, ctx| -> ConnectionFuture {
            let state = Arc::clone(&state);
            let stream_semaphore = Arc::clone(&stream_semaphore);
            Box::pin(handle_connection(conn, ctx, state, stream_semaphore))
        })
    };
    transport.handlers.register(OI_ALPN, oi_handler);

    let label_lookup: LabelLookup = {
        let state = Arc::clone(state);
        Arc::new(move |fp: &str| {
            let fp = fp.to_owned();
            state
                .db
                .call(move |db| super::auth::get_label(db, &fp).ok().flatten())
        })
    };
    *transport.label_lookup.write() = Some(label_lookup);

    tracing::info!(max_streams, "OI concurrency limit configured");
    Ok(())
}

// i[stream.control]
// i[stream.dispatch]
// i[stream.concurrency-limit]
// i[datagram.forward]
async fn handle_connection(
    conn: quinn::Connection,
    ctx: ConnectionContext,
    state: Arc<OiState>,
    stream_semaphore: Arc<Semaphore>,
) {
    let conn_id = conn.stable_id();
    let peer = conn.remote_address();
    let client = ctx.client_label;

    loop {
        tokio::select! {
            stream_result = conn.accept_bi() => {
                match stream_result {
                    Ok(stream) => {
                        let state = Arc::clone(&state);
                        let conn = conn.clone();
                        let client = client.clone();
                        let stream_semaphore = Arc::clone(&stream_semaphore);
                        tokio::spawn(
                            handle_bidi_stream(stream, conn, state, stream_semaphore)
                                .instrument(tracing::info_span!("oi", %peer, %client)),
                        );
                    }
                    Err(quinn::ConnectionError::ApplicationClosed { .. }) => break,
                    Err(e) => {
                        tracing::warn!("connection error: {e}");
                        break;
                    }
                }
            }
            datagram = conn.read_datagram() => {
                match datagram {
                    Ok(data) if data.len() >= 2 => {
                        let key = u16::from_be_bytes([data[0], data[1]]);
                        let payload = data[2..].to_vec();
                        let fwds = state.forwards.lock();
                        if let Some(sender) = fwds.get_udp_sender(conn_id, key) {
                            if let Err(mpsc::error::TrySendError::Closed(_)) = sender.try_send(payload) {
                                tracing::debug!(key, "UDP relay channel closed, dropping datagram");
                            }
                        } else {
                            tracing::debug!(key, "datagram for unknown forward key");
                        }
                    }
                    Ok(data) => {
                        tracing::debug!(len = data.len(), "datagram too short, ignoring");
                    }
                    Err(quinn::ConnectionError::ApplicationClosed { .. }) => break,
                    Err(e) => {
                        tracing::warn!("read_datagram error: {e}");
                        break;
                    }
                }
            }
        }
    }

    // i[forward.lifetime] — tear down all port forwards belonging to this connection.
    let entries = state.forwards.lock().remove_by_conn(conn_id);
    for entry in entries {
        let _ = entry.stop_tx.send(true);
    }
}

async fn handle_bidi_stream(
    (mut send, mut recv): (quinn::SendStream, quinn::RecvStream),
    conn: quinn::Connection,
    state: Arc<OiState>,
    stream_semaphore: Arc<Semaphore>,
) {
    // i[stream.concurrency-limit]
    let stream_permit = match stream_semaphore.try_acquire() {
        Ok(permit) => permit,
        Err(_) => {
            tracing::warn!(
                "stream concurrency limit reached; rejecting stream \
                 (adjust with --max-streams)"
            );
            state
                .event_tx
                .server_busy("stream concurrency limit reached");
            let resp = json!({
                "error": {
                    "code": "server_busy",
                    "message": "stream concurrency limit reached; retry after a delay",
                }
            });
            let bytes = serde_json::to_vec(&resp).expect("response serialisation never fails");
            let _ = send.write_all(&bytes).await;
            let _ = send.finish();
            return;
        }
    };

    let (line, leftover) = match read_json_line(&mut recv, 64 * 1024).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("stream read error: {e}");
            return;
        }
    };

    if line.is_empty() {
        return;
    }

    let first_obj = serde_json::from_slice::<serde_json::Value>(&line).unwrap_or_default();

    tracing::trace!(line = %String::from_utf8_lossy(&line).trim_end(), "bidi stream dispatch");

    // i[stream.forward] — forward data stream, identified by the "forward" key.
    if let Some(forward_id) = first_obj
        .get("forward")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
    {
        drop(stream_permit);
        handle_forward_stream((send, recv), forward_id, leftover, state).await;
        return;
    }

    let maybe_method = first_obj
        .get("method")
        .and_then(|m| m.as_str())
        .map(str::to_owned);

    // i[event.subscribe]
    if maybe_method.as_deref() == Some("/events/subscribe") {
        drop(stream_permit);
        // Send the response on the bidi stream first.
        let response = serde_json::to_vec(&serde_json::json!({ "result": {} }))
            .expect("response serialisation never fails");
        if let Err(e) = send.write_all(&response).await {
            tracing::warn!("subscribe: write error: {e}");
            return;
        }
        let _ = send.finish();

        // i[stream.events]
        // Open a server-initiated unidirectional stream and push events.
        let uni = match conn.open_uni().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("subscribe: open_uni failed: {e}");
                return;
            }
        };

        let rx = state.event_tx.subscribe();
        tokio::spawn(event_stream_task(uni, rx));
        return;
    }

    // i[logs.stream]
    if maybe_method.as_deref() == Some("/logs/stream") {
        drop(stream_permit);
        #[derive(serde::Deserialize)]
        struct Req {
            #[serde(default)]
            params: serde_json::Value,
        }
        let params: super::logs::LogStreamParams = match serde_json::from_slice::<Req>(&line)
            .map_err(|e| format!("invalid request: {e}"))
            .and_then(|r| {
                serde_json::from_value(r.params).map_err(|e| format!("invalid params: {e}"))
            }) {
            Ok(p) => p,
            Err(e) => {
                let resp = serde_json::to_vec(&json!({
                    "error": { "code": "requirements_invalid", "message": e }
                }))
                .expect("serialisation");
                let _ = send.write_all(&resp).await;
                let _ = send.finish();
                return;
            }
        };

        let opts = match super::logs::validate_params(&state, params) {
            Ok(o) => o,
            Err(e) => {
                let resp = serde_json::to_vec(&json!({
                    "error": { "code": e.code, "message": e.message }
                }))
                .expect("serialisation");
                let _ = send.write_all(&resp).await;
                let _ = send.finish();
                return;
            }
        };

        let rx = match crate::system::journal::spawn_log_reader(opts) {
            Ok(rx) => rx,
            Err(e) => {
                tracing::error!("failed to open journal: {e}");
                let resp = serde_json::to_vec(&json!({
                    "error": { "code": "server_busy", "message": format!("journal: {e}") }
                }))
                .expect("serialisation");
                let _ = send.write_all(&resp).await;
                let _ = send.finish();
                return;
            }
        };

        let response = serde_json::to_vec(&json!({ "result": {} })).expect("serialisation");
        if let Err(e) = send.write_all(&response).await {
            tracing::warn!("logs: write error: {e}");
            return;
        }
        let _ = send.finish();

        // i[stream.logs]
        let uni = match conn.open_uni().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("logs: open_uni failed: {e}");
                return;
            }
        };

        tokio::spawn(super::logs::log_stream_task(uni, rx));
        return;
    }

    // i[impl shell.concurrent]
    if maybe_method.as_deref() == Some("/shells/start") {
        drop(stream_permit);
        open_shell_session(conn, send, recv, leftover, line, state).await;
        return;
    }

    // i[volumes.shell]
    if maybe_method.as_deref() == Some("/volumes/shell") {
        drop(stream_permit);
        open_volume_shell_session(conn, send, recv, leftover, line, state).await;
        return;
    }

    // i[status.infra]
    if maybe_method.as_deref() == Some("/infra/status") {
        let result = super::handler::get_infra_status(&state).await;
        let response = match result {
            Ok(v) => serde_json::to_vec(&json!({ "result": v })).expect("serialisation"),
            Err(e) => serde_json::to_vec(&json!({
                "error": { "code": e.code, "message": e.message }
            }))
            .expect("serialisation"),
        };
        if let Err(e) = send.write_all(&response).await {
            tracing::warn!("stream write error: {e}");
        }
        let _ = send.finish();
        return;
    }

    // i[forward.request] — /forwards/start keeps the control stream open for the
    // duration of the forward; it must be handled outside the normal req/resp path.
    if maybe_method.as_deref() == Some("/forwards/start") {
        drop(stream_permit);
        forward_port_session(conn, send, recv, line, state).await;
        return;
    }

    // i[wire.actor] — extract actor from request or synthesise from client identity.
    let client_fp = extract_client_fp(&conn);
    let actor = first_obj
        .get("actor")
        .and_then(|v| serde_json::from_value::<Actor>(v.clone()).ok())
        .map(Arc::new)
        .unwrap_or_else(|| synthesise_actor(&state, client_fp.as_deref()));
    let ctx = RequestCtx {
        events: EventSenderWithActor::new(state.event_tx.clone(), actor),
    };

    let rest = recv.read_to_end(MAX_REQUEST_SIZE).await.unwrap_or_default();
    let buf = [line, rest].concat();

    let response = tokio::task::block_in_place(|| dispatch(&state, &buf, &ctx));
    if let Err(e) = send.write_all(&response).await {
        tracing::warn!("stream write error: {e}");
    }
    let _ = send.finish();
}

// i[stream.events]
// i[event.ordering]
async fn event_stream_task(
    mut send: quinn::SendStream,
    mut rx: tokio::sync::broadcast::Receiver<seedling_protocol::events::OiEvent>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let mut bytes = match serde_json::to_vec(&event) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!("event serialisation error: {e}");
                        continue;
                    }
                };
                bytes.push(b'\n');
                if let Err(e) = send.write_all(&bytes).await {
                    tracing::debug!("event stream closed: {e}");
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("event subscriber lagged, dropped {n} events");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                break;
            }
        }
    }
    let _ = send.finish();
}
