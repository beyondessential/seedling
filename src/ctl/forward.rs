use seedling::oi::client::OiClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

    // Send the ForwardPort request (newline-terminated). Do NOT call finish on
    // ctrl_send — the open stream is how the server detects the forward is alive.
    {
        let mut req = serde_json::to_vec(&serde_json::json!({
            "method": "ForwardPort",
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
            eprintln!("send ForwardPort: {e}");
            std::process::exit(1);
        }
    }

    // Read the newline-terminated JSON response.
    let resp_bytes = super::shell::read_shell_line(&mut ctrl_recv)
        .await
        .unwrap_or_else(|e| {
            eprintln!("read ForwardPort response: {e}");
            std::process::exit(1);
        });
    let resp: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap_or_else(|e| {
        eprintln!("parse ForwardPort response: {e}");
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
                            let fwd_id = forward_id.clone();
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
                                                    if fwd_send.write_all(&tbuf[..n]).await.is_err() {
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                let _ = fwd_send.finish();
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
                                socket.send_to(&data[2..], addr).await.ok();
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
    } else {
        eprintln!("unsupported proto: {proto}; expected tcp or udp");
        std::process::exit(1);
    }

    // Close the control stream to signal forward teardown to the server.
    let _ = ctrl_send.finish();
}
