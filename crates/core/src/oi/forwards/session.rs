use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    defs::resource::ResourceKind,
    oi::state::OiState,
    runtime::{
        AppPhase,
        registry::{DbInstanceRegistry, InstanceRegistry},
    },
    system::translate::proxy::instance_ipv6,
};

use super::registry::{ForwardEntry, ForwardProto};

struct StatusMsg {
    level: &'static str,
    message: String,
}

// i[forward.request]
// i[forward.mtu]
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
        params: serde_json::Value,
    }
    #[derive(serde::Deserialize)]
    struct Params {
        app: String,
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
        let Some(entry) = reg.get(&params.app) else {
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
        Ok(())
    })();
    if let Err((code, msg)) = lookup {
        write_err(&mut send, code, &msg).await;
        return;
    }

    let target_addr = {
        let registry = DbInstanceRegistry::new(Arc::clone(&state.db));
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
    let forward_id = Uuid::new_v4();
    let forward_key_result = state.forwards.lock().alloc_key(conn_id);
    let forward_key = match forward_key_result {
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

    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);

    // i[forward.mtu]
    let max_udp_payload: Option<u64> = match proto {
        ForwardProto::Udp => conn.max_datagram_size().map(|s| s.saturating_sub(2) as u64),
        ForwardProto::Tcp => None,
    };

    let (status_tx, status_rx) = tokio::sync::mpsc::channel::<StatusMsg>(16);
    let mut status_rx: Option<tokio::sync::mpsc::Receiver<StatusMsg>> =
        if matches!(proto, ForwardProto::Udp) {
            Some(status_rx)
        } else {
            None
        };

    let udp_tx: Option<tokio::sync::mpsc::Sender<Vec<u8>>> = if matches!(proto, ForwardProto::Udp) {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let conn_clone = conn.clone();
        let fkey = forward_key;
        let taddr = target_addr;
        let tport = params.port;
        let status_tx = status_tx.clone();
        tokio::spawn(async move {
            udp_relay_task(rx, conn_clone, fkey, taddr, tport, status_tx).await;
        });
        Some(tx)
    } else {
        None
    };

    state.forwards.lock().insert(ForwardEntry {
        forward_id,
        forward_key,
        conn_id,
        app: params.app.clone(),
        service: params.service.clone(),
        port: params.port,
        proto,
        target_addr,
        opened_at: jiff::Timestamp::now(),
        stop_tx,
        udp_tx,
    });

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
    // i[impl forward.start]
    state.event_tx.forward_started(
        &forward_id.to_string(),
        &params.app,
        &params.service,
        params.port,
    );

    let mut ctrl_buf = [0u8; 1];
    loop {
        let status_fut = async {
            match status_rx.as_mut() {
                Some(rx) => rx.recv().await,
                None => std::future::pending().await,
            }
        };
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
            msg = status_fut => {
                if let Some(msg) = msg {
                    // i[forward.status]
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
    // i[impl forward.start]
    state.event_tx.forward_stopped(&forward_id.to_string());

    let _ = send.finish();
    tracing::info!(
        app = %params.app, service = %params.service, fwd = %forward_id,
        "forward ended"
    );
}

// i[stream.forward]
// i[forward.tunnel.tcp]
pub(crate) async fn handle_forward_stream(
    (mut send, mut recv): (quinn::SendStream, quinn::RecvStream),
    forward_id_str: String,
    leftover: Vec<u8>,
    state: Arc<OiState>,
) {
    let forward_id = match uuid::Uuid::parse_str(&forward_id_str) {
        Ok(id) => id,
        Err(_) => {
            tracing::warn!(fwd = %forward_id_str, "invalid forward_id in stream header");
            return;
        }
    };

    let Some((target_addr, port)) = state.forwards.lock().get_target(&forward_id) else {
        tracing::warn!(fwd = %forward_id, "TCP forward stream for unknown forward");
        return;
    };

    let target = format!("[{target_addr}]:{port}");
    tracing::debug!(fwd = %forward_id, %target, "TCP relay: connecting");
    let mut tcp = match tokio::net::TcpStream::connect(&target).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(fwd = %forward_id, "TCP connect to {target} failed: {e}");
            return;
        }
    };
    tracing::debug!(fwd = %forward_id, %target, "TCP relay: connected");

    let (mut tcp_recv, mut tcp_send) = tcp.split();

    if !leftover.is_empty() && tcp_send.write_all(&leftover).await.is_err() {
        let _ = send.finish();
        return;
    }

    let mut qbuf = vec![0u8; 8192];
    let mut tbuf = vec![0u8; 8192];

    loop {
        tokio::select! {
            n = recv.read(&mut qbuf) => {
                match n {
                    Ok(Some(n)) if n > 0 => {
                        if tcp_send.write_all(&qbuf[..n]).await.is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            n = tcp_recv.read(&mut tbuf) => {
                match n {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if send.write_all(&tbuf[..n]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }

    let _ = send.finish();
    tracing::debug!(fwd = %forward_id, "TCP relay: closed");
}

// i[forward.tunnel.udp]
// i[datagram.forward]
async fn udp_relay_task(
    mut udp_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    conn: quinn::Connection,
    forward_key: u16,
    target_addr: std::net::Ipv6Addr,
    port: u16,
    status_tx: tokio::sync::mpsc::Sender<StatusMsg>,
) {
    let target = std::net::SocketAddr::V6(std::net::SocketAddrV6::new(target_addr, port, 0, 0));
    let socket = match tokio::net::UdpSocket::bind("[::]:0").await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(key = forward_key, "UDP relay bind failed: {e}");
            return;
        }
    };
    if let Err(e) = socket.connect(target).await {
        tracing::warn!(key = forward_key, "UDP relay connect failed: {e}");
        return;
    }

    let key_bytes = forward_key.to_be_bytes();
    let mut buf = vec![0u8; 65535];

    loop {
        tokio::select! {
            payload = udp_rx.recv() => {
                match payload {
                    Some(data) => {
                        if let Err(e) = socket.send(&data).await {
                            tracing::warn!(key = forward_key, "UDP send failed: {e}");
                        }
                    }
                    None => break,
                }
            }
            n = socket.recv(&mut buf) => {
                match n {
                    Ok(n) if n > 0 => {
                        let max_size = conn.max_datagram_size().unwrap_or(0);
                        if n + 2 > max_size {
                            tracing::warn!(
                                key = forward_key, size = n + 2, max = max_size,
                                "UDP response too large, dropping"
                            );
                            let _ = status_tx.try_send(StatusMsg {
                                level: "warn",
                                message: format!("UDP response too large ({} bytes, max {}), dropping", n + 2, max_size),
                            });
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
                                let _ = status_tx.try_send(StatusMsg {
                                    level: "warn",
                                    message: format!("send_datagram: {e}"),
                                });
                            }
                        }
                    }
                    _ => break,
                }
            }
        }
    }
}
