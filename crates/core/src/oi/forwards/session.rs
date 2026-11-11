use std::sync::Arc;

use seedling_protocol::{
    names::{AppName, ForwardId},
    relay::{abort_forward_stream, relay_forward_stream},
};
use tokio::io::AsyncWriteExt;

use super::{
    registry::{ForwardEntry, ForwardProto, ForwardStatus, plural},
    target::declares_port,
};
use crate::{
    defs::resource::ResourceKind,
    oi::state::OiState,
    runtime::{
        AppPhase,
        registry::{DbInstanceRegistry, InstanceRegistry},
    },
    system::translate::proxy::instance_ipv6,
};

// FWD#requesting-a-forward
// FWD#udp-tunnelling
pub(crate) async fn forward_port_session(
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    initial_line: Vec<u8>,
    state: Arc<OiState>,
) {
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(default)]
        actor: Option<seedling_protocol::actor::Actor>,
        #[serde(default)]
        params: serde_json::Value,
    }
    #[derive(serde::Deserialize)]
    struct Params {
        app: AppName,
        service: String,
        port: u16,
        proto: String,
    }

    async fn write_err(send: &mut quinn::SendStream, code: &str, msg: &str) {
        let resp = serde_json::to_vec(&serde_json::json!({
            "error": { "code": code, "message": msg }
        }))
        .unwrap_or_default();
        let _ = send.write_all(&resp).await;
        let _ = send.finish();
    }

    let req: Request = match serde_json::from_slice(&initial_line) {
        Ok(r) => r,
        Err(e) => {
            write_err(&mut send, "not_found", &format!("invalid request: {e}")).await;
            return;
        }
    };
    let req_actor = req.actor;
    let params: Params = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => {
            write_err(
                &mut send,
                "requirements_invalid",
                &format!("invalid params: {e}"),
            )
            .await;
            return;
        }
    };

    let proto = match params.proto.as_str() {
        "tcp" => ForwardProto::Tcp,
        "udp" => ForwardProto::Udp,
        other => {
            write_err(
                &mut send,
                "requirements_invalid",
                &format!("unknown proto: {other}; expected tcp or udp"),
            )
            .await;
            return;
        }
    };

    let lookup: Result<(), (&'static str, String)> = (|| {
        let reg = state.registry.read();
        let Some(entry) = reg.get(params.app.as_str()) else {
            return Err(("not_found", format!("app not found: {}", params.app)));
        };
        if !matches!(*entry.phase.lock(), AppPhase::Installed) {
            return Err((
                "not_installed",
                format!("app not installed: {}", params.app),
            ));
        }
        let def = entry.app.def.load();
        let found = def.resources.keys().any(|rid| {
            rid.kind == ResourceKind::Service && rid.name.as_str() == params.service.as_str()
        });
        if !found {
            return Err((
                "not_found",
                format!("service not found: {}", params.service),
            ));
        }
        if !declares_port(&def, &params.service, params.port, &proto) {
            return Err((
                "not_found",
                undeclared_port(&params.service, &params.proto, params.port),
            ));
        }
        Ok(())
    })();
    if let Err((code, msg)) = lookup {
        write_err(&mut send, code, &msg).await;
        return;
    }

    let target_addr = {
        let registry = DbInstanceRegistry::new(state.db.clone());
        let instance = match registry.get_or_create_singleton(
            &params.app,
            ResourceKind::Service,
            Some(&params.service),
        ) {
            Ok(i) => i,
            Err(e) => {
                write_err(&mut send, "internal", &format!("registry error: {e}")).await;
                return;
            }
        };
        instance_ipv6(&state.node_prefix, &instance)
    };

    let conn_id = conn.stable_id();
    let forward_id = ForwardId::generate();
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);

    // FWD#udp-tunnelling
    let max_udp_payload: Option<u64> = match proto {
        ForwardProto::Udp => conn.max_datagram_size().map(|s| s.saturating_sub(2) as u64),
        ForwardProto::Tcp => None,
    };

    // spec: FWD#requesting-a-forward — a UDP forward whose relay cannot reach the
    // service is refused here, rather than reported as started with every
    // datagram then vanishing.
    let udp_socket = match proto {
        ForwardProto::Udp => match open_udp_relay(target_addr, params.port).await {
            Ok(socket) => Some(socket),
            Err(e) => {
                write_err(
                    &mut send,
                    "internal",
                    &format!("UDP relay to the service: {e}"),
                )
                .await;
                return;
            }
        },
        ForwardProto::Tcp => None,
    };

    let (status_tx, mut status_rx) = tokio::sync::mpsc::channel::<ForwardStatus>(16);

    let (udp_tx, udp_rx) = match proto {
        ForwardProto::Udp => {
            let (tx, rx) = tokio::sync::mpsc::channel(64);
            (Some(tx), Some(rx))
        }
        ForwardProto::Tcp => (None, None),
    };

    let registered_proto = proto.clone();
    let registered = state
        .forwards
        .lock()
        .register(conn_id, |forward_key| ForwardEntry {
            forward_id,
            forward_key,
            conn_id,
            app: params.app.clone(),
            service: params.service.clone(),
            port: params.port,
            proto,
            target_addr,
            opened_at: jiff::Timestamp::now(),
            actor: req_actor,
            stop_tx,
            udp_tx,
            status_tx: status_tx.clone(),
            udp_dropped: 0,
        });
    let forward_key = match registered {
        Ok(k) => k,
        Err(e) => {
            write_err(
                &mut send,
                "internal",
                &format!("forward key allocation: {e}"),
            )
            .await;
            return;
        }
    };

    // spec: FWD#requesting-a-forward — the app's definition may have been replaced
    // between the check above and this registration, and that update's
    // teardown pass could not see a forward that was not yet registered.
    // Checked again now that it is: an update landing after this point finds
    // the forward and tears it down itself.
    let still_declared = state
        .registry
        .read()
        .get(params.app.as_str())
        .is_some_and(|entry| {
            declares_port(
                &entry.app.def.load(),
                &params.service,
                params.port,
                &registered_proto,
            )
        });
    if !still_declared {
        if let Some(entry) = state.forwards.lock().remove(&forward_id) {
            let _ = entry.stop_tx.send(true);
        }
        write_err(
            &mut send,
            "not_found",
            &undeclared_port(&params.service, &params.proto, params.port),
        )
        .await;
        return;
    }

    if let (Some(rx), Some(socket)) = (udp_rx, udp_socket) {
        let conn_clone = conn.clone();
        let status_tx = status_tx.clone();
        tokio::spawn(async move {
            udp_relay_task(rx, conn_clone, forward_key, socket, status_tx).await;
        });
    }

    let resp = serde_json::to_vec(&serde_json::json!({
        "result": {
            "forward_id": forward_id.to_string(),
            "forward_key": forward_key,
            "max_udp_payload": max_udp_payload,
        }
    }))
    .unwrap_or_default();
    let mut resp_line = resp;
    resp_line.push(b'\n');
    if let Err(e) = send.write_all(&resp_line).await {
        tracing::warn!(fwd = %forward_id, "write forward response: {e}");
        if let Some(entry) = state.forwards.lock().remove(&forward_id) {
            let _ = entry.stop_tx.send(true);
        }
        return;
    }

    tracing::info!(
        app = %params.app, service = %params.service, port = params.port,
        fwd = %forward_id, "forward started"
    );
    // spec: FWD#forward-events
    state
        .event_tx
        .forward_started(forward_id, &params.app, &params.service, params.port);

    let mut ctrl_buf = [0u8; 1];
    loop {
        tokio::select! {
            n = recv.read(&mut ctrl_buf) => {
                match n {
                    Ok(None) | Ok(Some(0)) | Err(_) => break,
                    Ok(Some(_)) => {}
                }
            }
            result = stop_rx.changed() => {
                if result.is_err() || *stop_rx.borrow() {
                    break;
                }
            }
            msg = status_rx.recv() => {
                if let Some(msg) = msg {
                    // FWD#status-messages
                    let line = serde_json::to_vec(&serde_json::json!({
                        "status": { "level": msg.level, "message": msg.message }
                    })).unwrap_or_default();
                    let mut buf = line;
                    buf.push(b'\n');
                    if send.write_all(&buf).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    if let Some(entry) = state.forwards.lock().remove(&forward_id) {
        let _ = entry.stop_tx.send(true);
    }
    // spec: FWD#forward-events
    state.event_tx.forward_stopped(forward_id);

    let _ = send.finish();
    tracing::info!(
        app = %params.app, service = %params.service, fwd = %forward_id,
        "forward ended"
    );
}

// WIRE#port-forward-data
// FWD#tcp-tunnelling
pub(crate) async fn handle_forward_stream(
    (mut send, mut recv): (quinn::SendStream, quinn::RecvStream),
    forward_id_str: String,
    leftover: Vec<u8>,
    state: Arc<OiState>,
) {
    let forward_id: ForwardId = match forward_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            tracing::warn!(fwd = %forward_id_str, "invalid forward_id in stream header");
            return;
        }
    };

    let Some((target_addr, port, status_tx)) = state.forwards.lock().get_target(&forward_id) else {
        tracing::warn!(fwd = %forward_id, "TCP forward stream for unknown forward");
        return;
    };

    let target = format!("[{target_addr}]:{port}");
    tracing::debug!(fwd = %forward_id, %target, "TCP relay: connecting");
    let mut tcp = match tokio::net::TcpStream::connect(&target).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(fwd = %forward_id, "TCP connect to {target} failed: {e}");
            // spec: FWD#status-messages
            let _ = status_tx.try_send(ForwardStatus::warn(format!(
                "connection to the service failed: {e}"
            )));
            abort_forward_stream(&mut send, &mut recv);
            return;
        }
    };
    tracing::debug!(fwd = %forward_id, %target, "TCP relay: connected");

    let (mut tcp_recv, mut tcp_send) = tcp.split();

    if !leftover.is_empty()
        && let Err(e) = tcp_send.write_all(&leftover).await
    {
        tracing::debug!(fwd = %forward_id, "TCP relay failed: {e}");
        abort_forward_stream(&mut send, &mut recv);
        return;
    }

    match relay_forward_stream(
        (&mut send, &mut recv),
        (&mut tcp_recv, &mut tcp_send),
        |_| {},
        |_| {},
    )
    .await
    {
        Ok(()) => tracing::debug!(fwd = %forward_id, "TCP relay: closed"),
        Err(e) => tracing::debug!(fwd = %forward_id, "TCP relay failed: {e}"),
    }
}

fn undeclared_port(service: &str, proto: &str, port: u16) -> String {
    format!("service {service} declares no {proto} port {port}")
}

/// Bind and connect the socket a UDP forward relays through.
async fn open_udp_relay(
    target_addr: std::net::Ipv6Addr,
    port: u16,
) -> std::io::Result<tokio::net::UdpSocket> {
    let target = std::net::SocketAddr::V6(std::net::SocketAddrV6::new(target_addr, port, 0, 0));
    let socket = tokio::net::UdpSocket::bind("[::]:0").await?;
    socket.connect(target).await?;
    Ok(socket)
}

// FWD#udp-tunnelling
// WIRE#port-forward-data
async fn udp_relay_task(
    mut udp_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    conn: quinn::Connection,
    forward_key: u16,
    socket: tokio::net::UdpSocket,
    status_tx: tokio::sync::mpsc::Sender<ForwardStatus>,
) {
    let key_bytes = forward_key.to_be_bytes();
    let mut send_failures: u64 = 0;
    let mut buf = vec![0u8; 65535];

    loop {
        tokio::select! {
            payload = udp_rx.recv() => {
                match payload {
                    Some(data) => match socket.send(&data).await {
                        // spec: FWD#status-messages
                        // Reported once an episode, as the registry reports
                        // dropped datagrams: a service that has gone away
                        // fails every send, and one message per datagram is
                        // all discarded once the status channel fills.
                        Ok(_) => {
                            if send_failures > 0 {
                                let _ = status_tx.try_send(ForwardStatus::warn(format!(
                                    "UDP relay recovered after {} to the service",
                                    plural(send_failures, "failed send")
                                )));
                                send_failures = 0;
                            }
                        }
                        Err(e) => {
                            if send_failures == 0 {
                                tracing::warn!(key = forward_key, "UDP send failed: {e}");
                                let _ = status_tx.try_send(ForwardStatus::warn(format!(
                                    "UDP send to the service failed: {e}"
                                )));
                            }
                            send_failures += 1;
                        }
                    },
                    None => break,
                }
            }
            n = socket.recv(&mut buf) => {
                match n {
                    // spec: FWD#relay-resilience
                    // A zero-length datagram is legal UDP, and an ICMP
                    // port-unreachable surfaces here as ECONNREFUSED on a
                    // connected socket. Both used to fall into the same
                    // `_ => break` arm as a fatal error, killing the relay
                    // forever while the forward stayed registered and listed
                    // as healthy.
                    Ok(0) => {
                        let mut pkt = Vec::with_capacity(2);
                        pkt.extend_from_slice(&key_bytes);
                        match conn.send_datagram(pkt.into()) {
                            Ok(()) => {}
                            Err(quinn::SendDatagramError::ConnectionLost(_)) => break,
                            Err(e) => {
                                tracing::warn!(key = forward_key, "send_datagram: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        // Transient: the peer may not be listening yet, or at
                        // all. Report it and keep relaying — the forward is
                        // not over because one datagram bounced.
                        tracing::debug!(key = forward_key, "UDP recv failed: {e}");
                        let _ = status_tx
                            .try_send(ForwardStatus::warn(format!("UDP receive failed: {e}")));
                    }
                    Ok(n) => {
                        let max_size = conn.max_datagram_size().unwrap_or(0);
                        if n + 2 > max_size {
                            tracing::warn!(
                                key = forward_key, size = n + 2, max = max_size,
                                "UDP response too large, dropping"
                            );
                            let _ = status_tx.try_send(ForwardStatus::warn(format!(
                                "UDP response too large ({} bytes, max {max_size}), dropping",
                                n + 2
                            )));
                            continue;
                        }
                        let mut pkt = Vec::with_capacity(2 + n);
                        pkt.extend_from_slice(&key_bytes);
                        pkt.extend_from_slice(&buf[..n]);
                        match conn.send_datagram(pkt.into()) {
                            Ok(()) => {}
                            Err(quinn::SendDatagramError::ConnectionLost(_)) => break,
                            Err(e) => {
                                tracing::warn!(key = forward_key, "send_datagram: {e}");
                                let _ = status_tx
                                    .try_send(ForwardStatus::warn(format!("send_datagram: {e}")));
                            }
                        }
                    }
                }
            }
        }
    }

    // spec: FWD#relay-resilience — nothing else observes this task, so
    // an unannounced exit left the forward in `/forwards/list` looking
    // healthy with no relay behind it.
    tracing::info!(key = forward_key, "UDP relay ended");
    let _ = status_tx.try_send(ForwardStatus::warn(
        "UDP relay ended; no further datagrams will be relayed",
    ));
}
