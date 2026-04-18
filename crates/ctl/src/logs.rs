use seedling_protocol::client::OiClient;

/// Run a log streaming session against an already-connected client.
///
/// `params` is the JSON object sent as the `params` field of the
/// `/logs/stream` request. `json_mode` controls whether entries are
/// printed as raw JSON lines or as human-readable text. `follow`
/// determines whether we listen for Ctrl-C (follow mode) or simply
/// drain the stream until the server closes it.
// i[ctl.logs.display]
pub(super) async fn stream_logs(
    client: &OiClient,
    params: serde_json::Value,
    json_mode: bool,
    follow: bool,
) {
    match run_log_session(client, params, json_mode, follow).await {
        Ok(()) => {}
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

async fn run_log_session(
    client: &OiClient,
    params: serde_json::Value,
    json_mode: bool,
    follow: bool,
) -> Result<(), String> {
    let req_bytes = serde_json::to_vec(&serde_json::json!({
        "method": "/logs/stream",
        "params": params,
    }))
    .expect("serialisation");

    let (mut send, mut recv) = client
        .open_bi()
        .await
        .map_err(|e| format!("open_bi: {e}"))?;

    send.write_all(&req_bytes)
        .await
        .map_err(|e| format!("write: {e}"))?;
    let _ = send.finish();

    let resp = recv
        .read_to_end(64 * 1024)
        .await
        .map_err(|e| format!("read response: {e}"))?;

    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&resp)
        && let Some(err) = v.get("error")
    {
        let code = err
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap_or("unknown");
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Err(format!("[{code}] {msg}"));
    }

    let mut log_stream = client
        .accept_uni()
        .await
        .map_err(|e| format!("accept_uni: {e}"))?;

    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];

    // i[ctl.logs.follow-interrupt]
    loop {
        let result = if follow {
            tokio::select! {
                r = log_stream.read(&mut tmp) => r,
                _ = tokio::signal::ctrl_c() => return Ok(()),
            }
        } else {
            log_stream.read(&mut tmp).await
        };

        match result {
            Ok(Some(n)) => {
                buf.extend_from_slice(&tmp[..n]);
                while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    let line = &buf[..pos];
                    if !line.is_empty() {
                        print_entry(line, json_mode);
                    }
                    buf.drain(..=pos);
                }
            }
            Ok(None) => break,
            Err(e) => return Err(format!("log stream: {e}")),
        }
    }

    // Flush any remaining partial line.
    if !buf.is_empty() {
        print_entry(&buf, json_mode);
    }

    Ok(())
}

fn print_entry(line: &[u8], json_mode: bool) {
    if json_mode {
        println!("{}", String::from_utf8_lossy(line));
        return;
    }

    let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
        println!("{}", String::from_utf8_lossy(line));
        return;
    };

    let timestamp = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
    let message = v.get("message").and_then(|m| m.as_str()).unwrap_or("");

    // Pick the most specific display name available.
    let display = v
        .get("instance")
        .or_else(|| v.get("resource"))
        .or_else(|| v.get("app"))
        .or_else(|| v.get("infra"))
        .and_then(|s| s.as_str())
        .unwrap_or("");

    println!("{timestamp} {display} {message}");
}
