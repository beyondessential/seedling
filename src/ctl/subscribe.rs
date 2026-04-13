use seedling::oi::client::OiClient;

pub async fn subscribe(client: &OiClient) {
    // Send the /events/subscribe request on a normal bidi stream.
    let req_bytes = serde_json::to_vec(&serde_json::json!({
        "method": "/events/subscribe",
        "params": {},
    }))
    .expect("serialisation");

    let (mut send, mut recv) = client.open_bi().await.unwrap_or_else(|e| {
        tracing::error!("open_bi: {e}");
        std::process::exit(1);
    });

    send.write_all(&req_bytes).await.unwrap_or_else(|e| {
        tracing::error!("write: {e}");
        std::process::exit(1);
    });
    let _ = send.finish();

    // Read the response to confirm success.
    let resp = recv.read_to_end(64 * 1024).await.unwrap_or_else(|e| {
        tracing::error!("read response: {e}");
        std::process::exit(1);
    });

    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&resp)
        && v.get("error").is_some()
    {
        eprintln!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        std::process::exit(1);
    }

    // Accept the server-initiated unidirectional event stream.
    let mut event_stream = client.accept_uni().await.unwrap_or_else(|e| {
        tracing::error!("accept_uni: {e}");
        std::process::exit(1);
    });

    // Read newline-delimited JSON events and print them.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match event_stream.read(&mut tmp).await {
            Ok(Some(n)) => {
                buf.extend_from_slice(&tmp[..n]);
                // Process complete lines.
                while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    let line = &buf[..pos];
                    if !line.is_empty() {
                        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) {
                            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                        } else {
                            println!("{}", String::from_utf8_lossy(line));
                        }
                    }
                    buf.drain(..=pos);
                }
            }
            Ok(None) => {
                break;
            }
            Err(e) => {
                tracing::error!("event stream error: {e}");
                break;
            }
        }
    }
}
