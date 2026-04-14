use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use seedling::oi::client::OiClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// l[impl ctl.forward.stats]
struct ForwardStats {
    bytes_to_service: AtomicU64,
    bytes_from_service: AtomicU64,
    connections_opened: AtomicU64,
    connections_active: AtomicU64,
    datagrams_to_service: AtomicU64,
    datagrams_from_service: AtomicU64,
}

impl ForwardStats {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            bytes_to_service: AtomicU64::new(0),
            bytes_from_service: AtomicU64::new(0),
            connections_opened: AtomicU64::new(0),
            connections_active: AtomicU64::new(0),
            datagrams_to_service: AtomicU64::new(0),
            datagrams_from_service: AtomicU64::new(0),
        })
    }

    fn print_tcp_summary(&self) {
        let to_svc = self.bytes_to_service.load(Ordering::Relaxed);
        let from_svc = self.bytes_from_service.load(Ordering::Relaxed);
        let opened = self.connections_opened.load(Ordering::Relaxed);
        eprintln!("\n--- Forward stats ---");
        eprintln!("Connections opened: {opened}");
        eprintln!("Bytes to service:   {}", format_bytes(to_svc));
        eprintln!("Bytes from service: {}", format_bytes(from_svc));
    }

    fn print_udp_summary(&self) {
        let to_svc = self.bytes_to_service.load(Ordering::Relaxed);
        let from_svc = self.bytes_from_service.load(Ordering::Relaxed);
        let dg_to = self.datagrams_to_service.load(Ordering::Relaxed);
        let dg_from = self.datagrams_from_service.load(Ordering::Relaxed);
        eprintln!("\n--- Forward stats ---");
        eprintln!("Datagrams to service:   {dg_to}");
        eprintln!("Datagrams from service: {dg_from}");
        eprintln!("Bytes to service:       {}", format_bytes(to_svc));
        eprintln!("Bytes from service:     {}", format_bytes(from_svc));
    }
}

fn format_bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    if n < 1024 * 1024 {
        return format!("{:.1} KiB", n as f64 / 1024.0);
    }
    if n < 1024 * 1024 * 1024 {
        return format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0));
    }
    format!("{:.1} GiB", n as f64 / (1024.0 * 1024.0 * 1024.0))
}

pub async fn forward_port(
    client: &OiClient,
    app: String,
    service: String,
    port: u16,
    proto: String,
    local_port: Option<u16>,
) {
    let (mut ctrl_send, mut ctrl_recv) = client.open_bi().await.unwrap_or_else(|e| {
        eprintln!("open control stream: {e}");
        std::process::exit(1);
    });

    // Send the /forwards/start request (newline-terminated). Do NOT call finish on
    // ctrl_send — the open stream is how the server detects the forward is alive.
    {
        let mut req = serde_json::to_vec(&serde_json::json!({
            "method": "/forwards/start",
            "params": {
                "app": app,
                "service": service,
                "port": port,
                "proto": proto,
            },
        }))
        .expect("serialisation never fails");
        req.push(b'\n');
        if let Err(e) = ctrl_send.write_all(&req).await {
            eprintln!("send /forwards/start: {e}");
            std::process::exit(1);
        }
    }

    // Read the newline-terminated JSON response.
    let resp_bytes = super::shell::read_shell_line(&mut ctrl_recv)
        .await
        .unwrap_or_else(|e| {
            eprintln!("read /forwards/start response: {e}");
            std::process::exit(1);
        });
    let resp: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap_or_else(|e| {
        eprintln!("parse /forwards/start response: {e}");
        std::process::exit(1);
    });
    if let Some(err) = resp.get("error") {
        let code = err.get("code").and_then(|c| c.as_str()).unwrap_or("error");
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
        eprintln!("[{code}] {msg}");
        std::process::exit(1);
    }
    let result = &resp["result"];
    let forward_id = result["forward_id"].as_str().unwrap_or("").to_owned();
    let forward_key = result["forward_key"].as_u64().unwrap_or(0) as u16;

    if proto == "tcp" {
        let listener = tokio::net::TcpListener::bind(format!("[::1]:{}", local_port.unwrap_or(0)))
            .await
            .unwrap_or_else(|e| {
                eprintln!("bind TCP listener: {e}");
                std::process::exit(1);
            });
        let bound = listener.local_addr().unwrap();
        eprintln!("Forwarding tcp://{app}/{service}:{port} -> {bound}");
        eprintln!("forward_id: {forward_id}");

        let stats = ForwardStats::new();
        let mut ctrl_buf = [0u8; 1];
        loop {
            tokio::select! {
                accept = listener.accept() => {
                    match accept {
                        Ok((tcp_conn, _peer)) => {
                            let (mut fwd_send, mut fwd_recv) = match client.open_bi().await {
                                Ok(s) => s,
                                Err(e) => {
                                    eprintln!("open relay stream: {e}");
                                    continue;
                                }
                            };
                            stats.connections_opened.fetch_add(1, Ordering::Relaxed);
                            stats.connections_active.fetch_add(1, Ordering::Relaxed);
                            let fwd_id = forward_id.clone();
                            let task_stats = Arc::clone(&stats);
                            tokio::spawn(async move {
                                // Write the forward data-stream header.
                                let mut hdr = serde_json::to_vec(
                                    &serde_json::json!({ "forward": fwd_id })
                                )
                                .unwrap_or_default();
                                hdr.push(b'\n');
                                if fwd_send.write_all(&hdr).await.is_err() {
                                    return;
                                }
                                let (mut tcp_read, mut tcp_write) = tcp_conn.into_split();
                                let mut qbuf = vec![0u8; 8192];
                                let mut tbuf = vec![0u8; 8192];
                                loop {
                                    tokio::select! {
                                        n = fwd_recv.read(&mut qbuf) => {
                                            match n {
                                                Ok(Some(n)) if n > 0 => {
                                                    task_stats.bytes_from_service.fetch_add(n as u64, Ordering::Relaxed);
                                                    if tcp_write.write_all(&qbuf[..n]).await.is_err() {
                                                        break;
                                                    }
                                                }
                                                _ => break,
                                            }
                                        }
                                        n = tcp_read.read(&mut tbuf) => {
                                            match n {
                                                Ok(0) | Err(_) => break,
                                                Ok(n) => {
                                                    task_stats.bytes_to_service.fetch_add(n as u64, Ordering::Relaxed);
                                                    if fwd_send.write_all(&tbuf[..n]).await.is_err() {
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                let _ = fwd_send.finish();
                                task_stats.connections_active.fetch_sub(1, Ordering::Relaxed);
                            });
                        }
                        Err(e) => {
                            eprintln!("TCP accept error: {e}");
                            break;
                        }
                    }
                }
                n = ctrl_recv.read(&mut ctrl_buf) => {
                    match n {
                        Ok(Some(_)) => {} // ignore any bytes on the control stream
                        _ => {
                            eprintln!("Control stream closed by server");
                            break;
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => break,
            }
        }
        stats.print_tcp_summary();
    } else if proto == "udp" {
        let socket = tokio::net::UdpSocket::bind(format!("[::1]:{}", local_port.unwrap_or(0)))
            .await
            .unwrap_or_else(|e| {
                eprintln!("bind UDP socket: {e}");
                std::process::exit(1);
            });
        let bound = socket.local_addr().unwrap();
        eprintln!("Forwarding udp://{app}/{service}:{port} -> {bound}");
        eprintln!("forward_id: {forward_id}  forward_key: {forward_key}");

        let stats = ForwardStats::new();
        let key_bytes = forward_key.to_be_bytes();
        let mut buf = vec![0u8; 65535];
        let mut last_client: Option<std::net::SocketAddr> = None;
        let mut ctrl_buf = [0u8; 1];

        loop {
            tokio::select! {
                // Local UDP datagram -> QUIC (prepend forward_key prefix)
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((n, addr)) => {
                            last_client = Some(addr);
                            stats.datagrams_to_service.fetch_add(1, Ordering::Relaxed);
                            stats.bytes_to_service.fetch_add(n as u64, Ordering::Relaxed);
                            let mut pkt = Vec::with_capacity(2 + n);
                            pkt.extend_from_slice(&key_bytes);
                            pkt.extend_from_slice(&buf[..n]);
                            if client.send_datagram(pkt).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("UDP recv error: {e}");
                            break;
                        }
                    }
                }
                // QUIC datagram -> local UDP (strip forward_key prefix)
                result = client.read_datagram() => {
                    match result {
                        Ok(data) if data.len() >= 2 => {
                            let dgram_key = u16::from_be_bytes([data[0], data[1]]);
                            if dgram_key == forward_key && let Some(addr) = last_client {
                                let payload = &data[2..];
                                stats.datagrams_from_service.fetch_add(1, Ordering::Relaxed);
                                stats.bytes_from_service.fetch_add(payload.len() as u64, Ordering::Relaxed);
                                socket.send_to(payload, addr).await.ok();
                            }
                        }
                        Err(_) => break,
                        _ => {}
                    }
                }
                n = ctrl_recv.read(&mut ctrl_buf) => {
                    match n {
                        Ok(Some(_)) => {}
                        _ => {
                            eprintln!("Control stream closed by server");
                            break;
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => break,
            }
        }
        stats.print_udp_summary();
    } else {
        eprintln!("unsupported proto: {proto}; expected tcp or udp");
        std::process::exit(1);
    }

    // Close the control stream to signal forward teardown to the server.
    let _ = ctrl_send.finish();
}
