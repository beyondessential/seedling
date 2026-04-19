use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use quinn::Endpoint;
use rustls::ServerConfig as TlsServerConfig;
use serde_json::json;
use tokio::sync::{Semaphore, mpsc};
use tracing::Instrument;

use seedling_protocol::{actor::Actor, keys};

use super::{
    auth::{SeedlingClientVerifier, TrustedKeys},
    forwards::session::{forward_port_session, handle_forward_stream},
    handler::{RequestCtx, dispatch},
    shells::open_shell_session,
    state::OiState,
};

/// Default maximum number of concurrently active bidirectional streams.
pub const DEFAULT_MAX_STREAMS: usize = 64;

/// Maximum size of a request body read (4 MiB).
const MAX_REQUEST_SIZE: usize = 4 * 1024 * 1024;

/// Default OI listen port.
pub const DEFAULT_PORT: u16 = 7891;

fn build_tls_config(
    key: &ed25519_dalek::SigningKey,
    spki: Vec<u8>,
    trusted: TrustedKeys,
) -> Result<TlsServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use rustls::{server::AlwaysResolvesServerRawPublicKeys, sign::CertifiedKey};
    use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

    let pkcs8 = key
        .to_pkcs8_der()
        .map_err(|e| format!("key encoding: {e}"))?;
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8.as_bytes().to_vec()));
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&private_key)
        .map_err(|e| format!("signing key: {e}"))?;

    let cert = CertificateDer::from(spki);
    let certified_key = Arc::new(CertifiedKey::new(vec![cert], signing_key));
    let resolver = Arc::new(AlwaysResolvesServerRawPublicKeys::new(certified_key));

    let client_verifier = Arc::new(SeedlingClientVerifier { trusted });

    let mut tls_config = TlsServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_cert_resolver(resolver);

    tls_config.key_log = Arc::new(rustls::KeyLogFile::new());

    Ok(tls_config)
}

fn extract_client_fp(conn: &quinn::Connection) -> Option<String> {
    let id = conn.peer_identity()?;
    let certs = id
        .downcast::<Vec<rustls_pki_types::CertificateDer<'static>>>()
        .ok()?;
    certs.first().map(|c| keys::fingerprint(c.as_ref()))
}

// i[wire.actor]
fn synthesise_actor(state: &OiState, fp: Option<&str>) -> Actor {
    let id = fp.map(str::to_owned);
    let display = fp
        .and_then(|f| {
            let db = state.db.lock();
            super::auth::get_label(&db, f).ok().flatten()
        })
        .or_else(|| id.clone());
    Actor {
        kind: Some("ctl".to_owned()),
        id,
        display,
        session: None,
    }
}

// i[transport.quic]
// i[transport.local]
// i[transport.client-auth]
// i[transport.listen]
pub async fn run(
    state: Arc<OiState>,
    addrs: &[SocketAddr],
    data_dir: &Path,
    max_streams: usize,
) -> Result<(String, Vec<Endpoint>), Box<dyn std::error::Error + Send + Sync>> {
    let key_path = data_dir.join("oi.key");
    let key = keys::load_or_generate(&key_path)?;
    let spki = keys::spki_der(&key);
    let fingerprint = keys::fingerprint(&spki);

    tracing::info!("OI SPKI fingerprint: {fingerprint}");
    state.spki_fingerprint.set(fingerprint.clone()).ok();

    {
        let db = state.db.lock();
        super::auth::load_from_db(&db, &state.trusted_keys)
            .map_err(|e| format!("loading authorized keys: {e}"))?;
        super::auth::import_bootstrap_file(data_dir, &db, &state.trusted_keys)
            .map_err(|e| format!("reading bootstrap file: {e}"))?;
    }

    if state.trusted_keys.read().is_empty() {
        tracing::warn!(
            "no authorized client keys — add fingerprints to {}/authorized_keys and restart",
            data_dir.display()
        );
    }

    let tls_config = build_tls_config(&key, spki, Arc::clone(&state.trusted_keys))?;
    let quic_config = quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)?;
    let mut transport = quinn::TransportConfig::default();
    // Send PING frames every 10 s so idle connections do not trip the idle
    // timeout.  As long as seedling-ctl is alive and the network is healthy,
    // PINGs reset the idle timer and the connection stays open indefinitely.
    transport.keep_alive_interval(Some(Duration::from_secs(10)));
    // 30 s: long enough that a single missed PING does not drop the
    // connection, short enough that a genuinely dead connection is detected
    // and cleaned up promptly.
    transport.max_idle_timeout(Some(
        quinn::VarInt::from_u32(30_000).into(), // 30 s in ms
    ));
    // Enable QUIC datagrams for UDP port-forward relay.
    transport.datagram_receive_buffer_size(Some(65536));
    let transport = Arc::new(transport);
    // Build one server config and clone it per endpoint (cheap: all heavy
    // state is behind Arcs inside quinn::ServerConfig).
    let mut base_server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_config));
    base_server_config.transport_config(Arc::clone(&transport));

    if std::env::var_os("SSLKEYLOGFILE").is_some() {
        tracing::warn!("SSLKEYLOGFILE is set — TLS session keys are being logged to disk");
    }

    let stream_semaphore = Arc::new(Semaphore::new(max_streams));
    tracing::info!(max_streams, "OI concurrency limit configured");

    let mut endpoints = Vec::with_capacity(addrs.len());
    for addr in addrs {
        let endpoint = Endpoint::server(base_server_config.clone(), *addr)?;
        tracing::info!("OI listening on {}", endpoint.local_addr()?);
        let ep_clone = endpoint.clone();
        tokio::spawn(accept_loop(
            endpoint,
            Arc::clone(&state),
            Arc::clone(&stream_semaphore),
        ));
        endpoints.push(ep_clone);
    }

    Ok((fingerprint, endpoints))
}

async fn accept_loop(endpoint: Endpoint, state: Arc<OiState>, stream_semaphore: Arc<Semaphore>) {
    while let Some(incoming) = endpoint.accept().await {
        let state = Arc::clone(&state);
        let stream_semaphore = Arc::clone(&stream_semaphore);
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    handle_connection(conn, state, stream_semaphore).await;
                }
                Err(e) => tracing::warn!("incoming connection failed: {e}"),
            }
        });
    }
}

// i[stream.control]
// i[stream.dispatch]
// i[stream.concurrency-limit]
// i[datagram.forward]
async fn handle_connection(
    conn: quinn::Connection,
    state: Arc<OiState>,
    stream_semaphore: Arc<Semaphore>,
) {
    let conn_id = conn.stable_id();
    let peer = conn.remote_address();
    let client_fp = extract_client_fp(&conn);

    let client: String = client_fp
        .as_deref()
        .and_then(|fp| {
            let db = state.db.lock();
            super::auth::get_label(&db, fp).ok().flatten()
        })
        .or_else(|| client_fp.clone())
        .unwrap_or_else(|| "unauthenticated".to_owned());

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

    if maybe_method.as_deref() == Some("/shells/start") {
        drop(stream_permit);
        open_shell_session(conn, send, recv, leftover, line, state).await;
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
        .unwrap_or_else(|| synthesise_actor(&state, client_fp.as_deref()));
    let ctx = RequestCtx { actor };

    let rest = recv.read_to_end(MAX_REQUEST_SIZE).await.unwrap_or_default();
    let buf = [line, rest].concat();

    let response = tokio::task::block_in_place(|| dispatch(&state, &buf, &ctx));
    if let Err(e) = send.write_all(&response).await {
        tracing::warn!("stream write error: {e}");
    }
    let _ = send.finish();
}

/// Reads bytes from `recv` until a `\n` is found or `max_len` is reached.
///
/// Returns `(line_including_newline, leftover_bytes_after_newline)`.
/// On stream close before any data, returns `([], [])`.
async fn read_json_line(
    recv: &mut quinn::RecvStream,
    max_len: usize,
) -> Result<(Vec<u8>, Vec<u8>), quinn::ReadError> {
    let mut buf = vec![0u8; max_len.min(65536)];
    let mut total = 0usize;
    loop {
        let chunk_size = (max_len - total).min(1024);
        if chunk_size == 0 {
            break;
        }
        match recv.read(&mut buf[total..total + chunk_size]).await? {
            Some(0) | None => break,
            Some(n) => {
                total += n;
                if let Some(pos) = buf[..total].iter().position(|&b| b == b'\n') {
                    let line = buf[..=pos].to_vec();
                    let leftover = buf[pos + 1..total].to_vec();
                    return Ok((line, leftover));
                }
            }
        }
    }
    Ok((buf[..total].to_vec(), Vec::new()))
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
