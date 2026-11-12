use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use quinn::Endpoint;
use rustls::ServerConfig as TlsServerConfig;
use serde_json::json;
use tokio::sync::Semaphore;
use tracing::Instrument;

use seedling_protocol::{
    OI_ALPN,
    actor::Actor,
    error::{ErrorCode, OiError},
    events::EventSenderWithActor,
    keys,
};

use super::{
    auth::{SeedlingClientVerifier, TrustedKeys},
    forwards::{
        Delivery,
        session::{forward_port_session, handle_forward_stream},
    },
    handler::{RequestCtx, dispatch},
    shells::{open_shell_session, open_volume_shell_session},
    state::OiState,
};

/// Default maximum number of concurrently active bidirectional streams.
pub const DEFAULT_MAX_STREAMS: usize = 64;

/// Maximum size of a request body read.
const MAX_REQUEST_SIZE: usize = seedling_protocol::REQUEST_LIMIT;

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
    // spec: TRN#connection
    tls_config.alpn_protocols = vec![OI_ALPN.to_vec()];

    Ok(tls_config)
}

fn extract_client_fp(conn: &quinn::Connection) -> Option<String> {
    let id = conn.peer_identity()?;
    let certs = id
        .downcast::<Vec<rustls_pki_types::CertificateDer<'static>>>()
        .ok()?;
    certs.first().map(|c| keys::fingerprint(c.as_ref()))
}

/// The actor a control request names, or `None` when it names none.
///
/// A malformed `actor` is refused rather than replaced: attributing the
/// request to someone the client did not name would record a fiction.
// spec: WIRE#actor-attribution
fn request_actor(request: &serde_json::Value) -> Result<Option<Actor>, OiError> {
    let Some(actor) = request.get("actor") else {
        return Ok(None);
    };
    let invalid = |why: String| OiError::new(ErrorCode::RequirementsInvalid, why);
    let members = actor
        .as_object()
        .ok_or_else(|| invalid("actor must be an object".to_owned()))?;
    let member = |name: &str| -> Result<Option<String>, OiError> {
        match members.get(name) {
            None => Ok(None),
            Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(invalid(format!("actor.{name} must be a string"))),
        }
    };
    Ok(Some(Actor {
        kind: member("kind")?,
        id: member("id")?,
        display: member("display")?,
        session: member("session")?,
    }))
}

/// Serialise an error response.
fn error_response(e: &OiError) -> Vec<u8> {
    serde_json::to_vec(&json!({ "error": { "code": e.code, "message": e.message } }))
        .expect("response serialisation never fails")
}

// spec: WIRE#actor-attribution
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

/// Write the server's SPKI fingerprint (hex, newline-terminated) to
/// `oi.fingerprint` in the data directory.
// spec: TRN#server-identity
fn publish_fingerprint(data_dir: &Path, fingerprint: &str) -> std::io::Result<()> {
    std::fs::write(data_dir.join("oi.fingerprint"), format!("{fingerprint}\n"))
}

// spec: TRN#connection
// spec: TRN#server-identity
// spec: TRN#client-authentication
// spec: TRN#listen-addresses
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

    // Publish the fingerprint so a co-located process (e.g. seedling-web) can
    // pin it without a fingerprint probe. Non-fatal: the daemon still serves if
    // the write fails; only the file-based bootstrap of co-located clients breaks.
    if let Err(e) = publish_fingerprint(data_dir, &fingerprint) {
        tracing::warn!("failed to write OI fingerprint file: {e}");
    }

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

    if state.trusted_keys.is_empty() {
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

// spec: WIRE#control-requests
// spec: WIRE#stream-dispatch
// spec: WIRE#stream-limit
// spec: WIRE#port-forward-data
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
            let fp = fp.to_owned();
            state
                .db
                .call(move |db| super::auth::get_label(db, &fp).ok().flatten())
        })
        .or_else(|| client_fp.clone())
        .unwrap_or_else(|| "unauthenticated".to_owned());

    // spec: TRN#client-authentication — the handshake authorised this
    // connection once; the subscription is how it learns that the key it
    // was authorised with has since gone.
    let mut keys_changed = state.trusted_keys.subscribe();

    loop {
        tokio::select! {
            // spec: KEY#revoking-a-key
            Ok(()) = keys_changed.changed() => {
                // Asked afresh rather than taken from the signal, so a
                // connection that slept through several changes still sees
                // the one that matters.
                if let Some(fp) = client_fp.as_deref()
                    && !state.trusted_keys.contains(fp)
                {
                    tracing::info!(
                        fingerprint = %fp,
                        "closing a connection whose key was revoked"
                    );
                    conn.close(quinn::VarInt::from_u32(0), b"key revoked");
                    break;
                }
            }
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
                        match state.forwards.lock().deliver_datagram(conn_id, key, payload) {
                            Delivery::Delivered | Delivery::Dropped => {}
                            Delivery::RelayGone => {
                                tracing::debug!(key, "UDP relay channel closed, dropping datagram");
                            }
                            Delivery::UnknownKey => {
                                tracing::debug!(key, "datagram for unknown forward key");
                            }
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

    // spec: FWD#forward-lifetime — tear down all port forwards belonging to this connection.
    let entries = state.forwards.lock().remove_by_conn(conn_id);
    for entry in entries {
        let _ = entry.stop_tx.send(true);
    }

    // spec: RLY#offer-lifetime — an offer lives no longer than the connection
    // that made it. No actor on the event: nobody asked for this, the connection
    // simply went away.
    let ended = state.canopy.remove_by_conn(conn_id);
    for offer in &ended {
        tracing::info!(
            offer_id = %offer.offer_id,
            agent = %offer.agent,
            "a Canopy offer ended with its connection"
        );
        state.event_tx.canopy_withdrawn(
            offer.offer_id,
            crate::oi::canopy::WithdrawReason::Disconnected.as_str(),
            None,
        );
    }
    // spec: CNR#report-failure-fault — losing the last offer ends the expectation
    // to report, so a fault from when it held must not outlive it.
    if !ended.is_empty() {
        crate::runtime::canopy::absent::offer_was_live(&state);
        crate::runtime::canopy::clear_fault_if_not_expected(&state);
    }
}

async fn handle_bidi_stream(
    (mut send, mut recv): (quinn::SendStream, quinn::RecvStream),
    conn: quinn::Connection,
    state: Arc<OiState>,
    stream_semaphore: Arc<Semaphore>,
) {
    // spec: WIRE#stream-limit
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

    // spec: WIRE#port-forward-data — forward data stream, identified by the "forward" key.
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

    // spec: EVT#subscription
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

        // spec: WIRE#subscription-requests
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

    // spec: LOG#log-stream-request
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

        let rx =
            match tokio::task::block_in_place(|| crate::system::journal::spawn_log_reader(opts)) {
                Ok(rx) => rx,
                Err(e) => {
                    tracing::error!("failed to open journal: {e}");
                    // spec: LOG#log-stream-request
                    let resp = error_response(&OiError::new(
                        ErrorCode::Internal,
                        format!("cannot open the host's log store: {e}"),
                    ));
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

        // spec: WIRE#subscription-requests
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

    // spec: SHL#concurrency
    if maybe_method.as_deref() == Some("/shells/start") {
        drop(stream_permit);
        open_shell_session(conn, send, recv, leftover, line, state).await;
        return;
    }

    // spec: SHL#volume-shells
    if maybe_method.as_deref() == Some("/volumes/shell") {
        drop(stream_permit);
        open_volume_shell_session(conn, send, recv, leftover, line, state).await;
        return;
    }

    // spec: STS#infrastructure-status
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

    // spec: FWD#requesting-a-forward — /forwards/start keeps the control stream open for the
    // duration of the forward; it must be handled outside the normal req/resp path.
    if maybe_method.as_deref() == Some("/forwards/start") {
        drop(stream_permit);
        forward_port_session(conn, send, recv, line, state).await;
        return;
    }

    // spec: WIRE#control-requests
    // The opening line was read up to its first line feed or a cap, so the
    // request is only whole once the rest of the stream is read.
    let rest = match recv
        .read_to_end(MAX_REQUEST_SIZE.saturating_sub(line.len()))
        .await
    {
        Ok(rest) => rest,
        Err(e) => {
            let refusal = OiError::new(
                ErrorCode::RequirementsInvalid,
                format!("request cannot be read whole, or exceeds {MAX_REQUEST_SIZE} bytes: {e}"),
            );
            let _ = send.write_all(&error_response(&refusal)).await;
            let _ = send.finish();
            return;
        }
    };
    let buf = [line, rest].concat();

    // spec: WIRE#actor-attribution — extract actor from request or synthesise from client identity.
    let request = serde_json::from_slice::<serde_json::Value>(&buf).unwrap_or_default();
    let actor = match request_actor(&request) {
        Ok(Some(actor)) => Arc::new(actor),
        Ok(None) => synthesise_actor(&state, extract_client_fp(&conn).as_deref()),
        Err(refusal) => {
            let _ = send.write_all(&error_response(&refusal)).await;
            let _ = send.finish();
            return;
        }
    };
    let ctx = RequestCtx {
        events: EventSenderWithActor::new(state.event_tx.clone(), actor),
    };

    // spec: RLY#offers — registering an offer needs the connection it arrived on,
    // so these are dispatched here rather than through the shared table. Done
    // after the actor is resolved so their events carry it like every other
    // request's do.
    if let Some(response) = super::handler::dispatch_connection_bound(&state, &conn, &buf, &ctx) {
        if let Err(e) = send.write_all(&response).await {
            tracing::warn!("stream write error: {e}");
        }
        let _ = send.finish();
        return;
    }

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

// spec: WIRE#subscription-requests
// spec: EVT#ordering
async fn event_stream_task(
    mut send: quinn::SendStream,
    rx: tokio::sync::broadcast::Receiver<seedling_protocol::events::OiEvent>,
) {
    pump_events(&mut send, rx).await;
    let _ = send.finish();
}

/// Write events from `rx` to `send` as newline-delimited JSON until the bus
/// closes or the write fails.
async fn pump_events(
    send: &mut (impl tokio::io::AsyncWrite + Unpin),
    mut rx: tokio::sync::broadcast::Receiver<seedling_protocol::events::OiEvent>,
) {
    use tokio::io::AsyncWriteExt as _;

    loop {
        let event = match rx.recv().await {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("event subscriber lagged, dropped {n} events");
                // spec: EVT#gaps
                seedling_protocol::events::OiEvent::events_dropped(Some(n))
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
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
}

#[cfg(test)]
mod event_stream_tests {
    use seedling_protocol::events::OiEvent;

    use super::*;

    // spec: EVT#gaps
    #[tokio::test]
    async fn lagged_subscriber_receives_gap_marker() {
        let (tx, rx) = tokio::sync::broadcast::channel(2);
        for i in 0..5 {
            tx.send(OiEvent::ServerBusy {
                timestamp: jiff::Timestamp::now(),
                reason: format!("busy-{i}"),
                actor: None,
            })
            .unwrap();
        }
        drop(tx);

        let mut out = Vec::new();
        pump_events(&mut out, rx).await;
        let lines: Vec<serde_json::Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["type"], "EventsDropped");
        assert_eq!(lines[0]["skipped"], 3);
        assert_eq!(lines[1]["reason"], "busy-3");
        assert_eq!(lines[2]["reason"], "busy-4");
    }
}

#[cfg(test)]
mod connection_tests {
    //! What a real connection carries, and what ends it.
    //!
    //! Registering an offer needs the connection it arrived on, and an offer's
    //! lifetime is that connection's, so neither the accept side nor the
    //! teardown hook can be exercised without a connection actually existing.
    //! Revocation is the same: it is a property of a connection the handshake
    //! already let through.

    use std::{net::SocketAddr, sync::Arc, time::Duration};

    use ed25519_dalek::SigningKey;
    use quinn::Endpoint;
    use rand::rngs::ThreadRng;
    use seedling_protocol::{
        actor::Actor,
        canopy::OfferResult,
        client::{ClientAuth, OiClient},
        keys::{self, ClientIdentity},
    };
    use serde_json::json;

    use super::{DEFAULT_MAX_STREAMS, MAX_REQUEST_SIZE, build_tls_config, handle_connection};
    use crate::oi::{state::OiState, test_support::TestOi};

    /// Run the real connection handler against a real endpoint.
    fn spawn_server(state: Arc<OiState>) -> SocketAddr {
        let key = SigningKey::generate(&mut ThreadRng::default());
        let spki = keys::spki_der(&key);
        let tls = build_tls_config(&key, spki, Arc::clone(&state.trusted_keys))
            .expect("build tls config");
        let quic = quinn::crypto::rustls::QuicServerConfig::try_from(tls).expect("quic config");
        let endpoint = Endpoint::server(
            quinn::ServerConfig::with_crypto(Arc::new(quic)),
            "127.0.0.1:0".parse().unwrap(),
        )
        .expect("endpoint");
        let addr = endpoint.local_addr().expect("local addr");

        tokio::spawn(async move {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(DEFAULT_MAX_STREAMS));
            while let Some(incoming) = endpoint.accept().await {
                let state = Arc::clone(&state);
                let semaphore = Arc::clone(&semaphore);
                tokio::spawn(async move {
                    if let Ok(conn) = incoming.await {
                        handle_connection(conn, state, semaphore).await;
                    }
                });
            }
        });
        addr
    }

    /// Wait for the offer count to reach `want`, or give up.
    ///
    /// Teardown happens when the handler notices the connection is gone, which
    /// is not synchronous with the client dropping it.
    /// Wait for the active faults to reach `want`.
    ///
    /// Losing a connection removes its offers first and clears the report
    /// fault several steps later, with an event sent per offer in between.
    /// Waiting on the offer count and then asserting on the fault reads
    /// that gap as a failure whenever the machine is busy enough to widen
    /// it past a poll.
    async fn await_faults(state: &Arc<OiState>, want: usize) -> usize {
        let count = |state: &Arc<OiState>| {
            state.db.call(|db| {
                crate::runtime::faults::list_active_faults(db, None)
                    .unwrap_or_default()
                    .len()
            })
        };
        for _ in 0..100 {
            if count(state) == want {
                return want;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        count(state)
    }

    async fn await_offers(state: &Arc<OiState>, want: usize) -> usize {
        for _ in 0..100 {
            let count = state.canopy.count();
            if count == want {
                return count;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        state.canopy.count()
    }

    // spec: RLY#offers
    // spec: RLY#offer-lifetime
    #[test]
    fn an_offer_is_registered_over_a_connection_and_dies_with_it() {
        let oi = TestOi::new();
        oi.block_on(async {
            let _ = rustls::crypto::ring::default_provider().install_default();

            let identity = ClientIdentity::ephemeral();
            oi.state.trusted_keys.insert(identity.fingerprint.clone());
            let addr = spawn_server(Arc::clone(&oi.state));

            let client = OiClient::connect(addr, ClientAuth::TrustAny, &identity, Actor::default())
                .await
                .expect("connect");

            let result = client
                .request(
                    "/canopy/offer",
                    json!({ "agent": "test 1.0", "endpoint": "https://example.invalid" }),
                )
                .await
                .expect("the offer is accepted");
            let offer: OfferResult = serde_json::from_value(result).expect("an offer id");

            assert_eq!(oi.state.canopy.count(), 1);
            let live = oi.state.canopy.current().expect("a live offer");
            assert_eq!(live.offer_id, offer.offer_id);
            assert_eq!(live.agent, "test 1.0");

            // Dropping the client closes the connection, which is what ends the
            // offer: nothing has to ask for it.
            drop(client);
            assert_eq!(
                await_offers(&oi.state, 0).await,
                0,
                "an offer must not outlive the connection that made it"
            );
        });
    }

    /// Send `request` raw on a fresh control stream and return the response.
    async fn raw_request(client: &OiClient, request: &[u8]) -> serde_json::Value {
        let (mut send, mut recv) = client.open_bi().await.expect("open stream");
        send.write_all(request).await.expect("write");
        let _ = send.finish();
        let response = recv.read_to_end(1 << 20).await.expect("read response");
        serde_json::from_slice(&response).expect("a JSON response")
    }

    // spec: WIRE#actor-attribution
    // spec: WIRE#control-requests
    #[test]
    fn a_malformed_actor_or_an_oversized_request_is_requirements_invalid() {
        let oi = TestOi::new();
        oi.block_on(async {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let identity = ClientIdentity::ephemeral();
            oi.state.trusted_keys.insert(identity.fingerprint.clone());
            let addr = spawn_server(Arc::clone(&oi.state));
            let client = OiClient::connect(addr, ClientAuth::TrustAny, &identity, Actor::default())
                .await
                .expect("connect");

            for actor in [json!("someone"), json!({ "kind": 7 }), json!(null)] {
                let request =
                    json!({ "method": "/server/ping", "actor": actor, "params": {} }).to_string();
                let response = raw_request(&client, format!("{request}\n").as_bytes()).await;
                assert_eq!(
                    response["error"]["code"], "requirements_invalid",
                    "{actor}: {response}"
                );
            }

            let padding = "x".repeat(MAX_REQUEST_SIZE);
            let request = json!({ "method": "/server/ping", "params": { "pad": padding } });
            let response = raw_request(&client, request.to_string().as_bytes()).await;
            assert_eq!(
                response["error"]["code"], "requirements_invalid",
                "{response}"
            );

            let ok = raw_request(&client, b"{\"method\":\"/server/ping\"}\n").await;
            assert!(ok.get("result").is_some(), "{ok}");
        });
    }

    // spec: CNR#report-failure-fault
    #[test]
    fn losing_the_last_offer_clears_a_report_fault() {
        let oi = TestOi::new();
        oi.block_on(async {
            let _ = rustls::crypto::ring::default_provider().install_default();

            let identity = ClientIdentity::ephemeral();
            oi.state.trusted_keys.insert(identity.fingerprint.clone());
            let addr = spawn_server(Arc::clone(&oi.state));

            let client = OiClient::connect(addr, ClientAuth::TrustAny, &identity, Actor::default())
                .await
                .expect("connect");
            client
                .request(
                    "/canopy/offer",
                    json!({ "agent": "test", "endpoint": "https://example.invalid" }),
                )
                .await
                .expect("the offer is accepted");

            // As a failing report would have left it.
            oi.state.db.call(|db| {
                crate::runtime::faults::file_fault(
                    db,
                    &seedling_protocol::names::AppName::new_unchecked("seedling"),
                    None,
                    None,
                    None,
                    "canopy_report_failed",
                    "canopy said no",
                )
                .expect("file a fault");
            });

            drop(client);
            assert_eq!(await_offers(&oi.state, 0).await, 0);

            let remaining = await_faults(&oi.state, 0).await;
            assert_eq!(
                remaining, 0,
                "nothing is expected of an instance with no provider, so the \
                 fault must go with the connection rather than linger until the \
                 next scheduled tick"
            );
        });
    }

    // spec: RLY#offer-lifetime
    #[test]
    fn a_client_can_withdraw_its_own_offer_and_keep_the_connection() {
        let oi = TestOi::new();
        oi.block_on(async {
            let _ = rustls::crypto::ring::default_provider().install_default();

            let identity = ClientIdentity::ephemeral();
            oi.state.trusted_keys.insert(identity.fingerprint.clone());
            let addr = spawn_server(Arc::clone(&oi.state));

            let client = OiClient::connect(addr, ClientAuth::TrustAny, &identity, Actor::default())
                .await
                .expect("connect");

            let result = client
                .request(
                    "/canopy/offer",
                    json!({ "agent": "test", "endpoint": "https://example.invalid" }),
                )
                .await
                .expect("the offer is accepted");
            let offer: OfferResult = serde_json::from_value(result).expect("an offer id");

            client
                .request("/canopy/withdraw", json!({ "offer_id": offer.offer_id }))
                .await
                .expect("withdrawing our own offer");
            assert_eq!(oi.state.canopy.count(), 0);

            // The connection is still usable: withdrawing stops carrying
            // requests, it does not hang up.
            client
                .request("/server/ping", json!({}))
                .await
                .expect("the connection outlives the offer");
        });
    }

    // spec: RLY#disabled-access
    #[test]
    fn a_disabled_instance_refuses_an_offer() {
        let oi = TestOi::new();
        oi.state.canopy.set_enabled(false);
        oi.block_on(async {
            let _ = rustls::crypto::ring::default_provider().install_default();

            let identity = ClientIdentity::ephemeral();
            oi.state.trusted_keys.insert(identity.fingerprint.clone());
            let addr = spawn_server(Arc::clone(&oi.state));

            let client = OiClient::connect(addr, ClientAuth::TrustAny, &identity, Actor::default())
                .await
                .expect("connect");

            let err = client
                .request(
                    "/canopy/offer",
                    json!({ "agent": "test", "endpoint": "https://example.invalid" }),
                )
                .await
                .expect_err("a disabled instance accepts no offers");
            assert!(
                format!("{err}").contains("canopy_disabled"),
                "the client must be able to tell a refusal from a breakage: {err}"
            );
            assert_eq!(oi.state.canopy.count(), 0);
        });
    }

    /// Wait for `conn` to close, or give up: the handler notices the key
    /// went on its own schedule, not the test's.
    async fn await_closed(client: &OiClient) -> bool {
        tokio::time::timeout(Duration::from_secs(5), client.connection().closed())
            .await
            .is_ok()
    }

    // spec: KEY#revoking-a-key
    // spec: TRN#client-authentication
    #[test]
    fn revoking_a_key_closes_the_connection_it_authenticated() {
        let oi = TestOi::new();
        oi.block_on(async {
            let _ = rustls::crypto::ring::default_provider().install_default();

            let identity = ClientIdentity::ephemeral();
            let trusted = Arc::clone(&oi.state.trusted_keys);
            oi.state.db.call({
                let fingerprint = identity.fingerprint.clone();
                let trusted = Arc::clone(&trusted);
                move |db| {
                    crate::oi::auth::authorize_key(db, &trusted, &fingerprint, "laptop")
                        .expect("authorise the key")
                }
            });
            let addr = spawn_server(Arc::clone(&oi.state));

            let client = OiClient::connect(addr, ClientAuth::TrustAny, &identity, Actor::default())
                .await
                .expect("connect");
            client
                .request(
                    "/canopy/offer",
                    json!({ "agent": "test", "endpoint": "https://example.invalid" }),
                )
                .await
                .expect("the connection is usable while the key is authorised");

            oi.state.db.call({
                let fingerprint = identity.fingerprint.clone();
                move |db| {
                    assert!(
                        crate::oi::auth::revoke_key(db, &trusted, &fingerprint)
                            .expect("revoke the key"),
                        "the key was there to revoke"
                    );
                }
            });

            assert!(
                await_closed(&client).await,
                "a revoked key must not keep the connection it opened"
            );
            assert!(
                client.request("/status", json!({})).await.is_err(),
                "and nothing more can be asked over it"
            );
        });
    }

    // spec: KEY#revoking-a-key
    #[test]
    fn revoking_one_key_leaves_another_clients_connection_alone() {
        let oi = TestOi::new();
        oi.block_on(async {
            let _ = rustls::crypto::ring::default_provider().install_default();

            let staying = ClientIdentity::ephemeral();
            let going = ClientIdentity::ephemeral();
            oi.state.trusted_keys.insert(staying.fingerprint.clone());
            oi.state.trusted_keys.insert(going.fingerprint.clone());
            let addr = spawn_server(Arc::clone(&oi.state));

            let client = OiClient::connect(addr, ClientAuth::TrustAny, &staying, Actor::default())
                .await
                .expect("connect");

            assert!(oi.state.trusted_keys.remove(&going.fingerprint));

            assert!(
                !await_closed(&client).await,
                "revoking a key is not a reason to drop everyone else"
            );
        });
    }
}

#[cfg(test)]
mod auth_tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use ed25519_dalek::SigningKey;
    use quinn::Endpoint;
    use rand::rngs::ThreadRng;

    use seedling_protocol::{
        actor::Actor,
        client::{ClientAuth, OiClient},
        keys::{self, ClientIdentity},
    };

    use super::{Arc, build_tls_config, publish_fingerprint};
    use crate::oi::auth::{TrustedKeys, new_trusted_keys};

    // spec: TRN#server-identity
    // The published file must carry exactly the fingerprint clients pin, so a
    // reader that trims whitespace recovers the fingerprint verbatim.
    #[test]
    fn publishes_fingerprint_to_data_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fingerprint = "0123456789abcdef";
        publish_fingerprint(dir.path(), fingerprint).expect("write fingerprint");

        let contents =
            std::fs::read_to_string(dir.path().join("oi.fingerprint")).expect("read back");
        assert!(
            contents.ends_with('\n'),
            "file should be newline-terminated"
        );
        assert_eq!(contents.trim(), fingerprint);
    }

    /// Stand up a real OI QUIC endpoint using the production TLS wiring
    /// ([`build_tls_config`]) and accept connections, holding any that
    /// complete the handshake. Returns the bound address.
    fn spawn_oi_server(trusted: TrustedKeys) -> SocketAddr {
        let key = SigningKey::generate(&mut ThreadRng::default());
        let spki = keys::spki_der(&key);
        let tls = build_tls_config(&key, spki, trusted).expect("build tls config");
        let quic = quinn::crypto::rustls::QuicServerConfig::try_from(tls).expect("quic config");
        let server_config = quinn::ServerConfig::with_crypto(Arc::new(quic));
        let endpoint =
            Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).expect("endpoint");
        let addr = endpoint.local_addr().expect("local addr");
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                tokio::spawn(async move {
                    // The handshake includes mTLS client-cert verification; an
                    // untrusted peer fails here and never yields a connection.
                    if let Ok(conn) = incoming.await {
                        let _ = conn.closed().await;
                    }
                });
            }
        });
        addr
    }

    /// Connect with `identity` and report whether the peer ends up with a
    /// usable connection. In QUIC/TLS 1.3 the client completes the handshake
    /// optimistically before the server verifies its certificate, so a
    /// rejected peer's `connect` may still return `Ok`; the rejection then
    /// surfaces as the server closing the connection. We treat "still open
    /// after a short grace period" as admitted, and "connect failed or
    /// connection closed by the server" as rejected.
    async fn connection_admitted(addr: SocketAddr, identity: &ClientIdentity) -> bool {
        match OiClient::connect(addr, ClientAuth::TrustAny, identity, Actor::default()).await {
            Err(_) => false,
            Ok(client) => {
                let closed = client.connection().closed();
                // If the server accepted us it holds the connection open and
                // this times out; if it rejected us the close arrives promptly.
                tokio::time::timeout(Duration::from_secs(2), closed)
                    .await
                    .is_err()
            }
        }
    }

    // spec: TRN#client-authentication
    // A hostile client presenting a key the daemon does not trust must not
    // obtain a usable connection, while a trusted key is admitted.
    #[tokio::test]
    async fn rejects_untrusted_client_key_and_admits_trusted() {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let authorized = ClientIdentity::ephemeral();
        let stranger = ClientIdentity::ephemeral();

        let trusted = new_trusted_keys();
        trusted.insert(authorized.fingerprint.clone());

        let addr = spawn_oi_server(trusted);

        assert!(
            connection_admitted(addr, &authorized).await,
            "trusted client should obtain an open connection"
        );
        assert!(
            !connection_admitted(addr, &stranger).await,
            "untrusted client must be rejected at the auth boundary"
        );
    }
}
