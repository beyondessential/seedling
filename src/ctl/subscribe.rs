use std::{net::SocketAddr, time::Duration};

use seedling::oi::{
    client::{ClientAuth, OiClient},
    keys::ClientIdentity,
};

const RECONNECT_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

// i[impl ctl.subscribe.reconnect]
pub async fn subscribe(endpoint: SocketAddr, fingerprint: String, identity: &ClientIdentity) {
    let mut deadline = tokio::time::Instant::now() + RECONNECT_TIMEOUT;
    let mut backoff = Duration::from_secs(1);

    loop {
        let client = match OiClient::connect(
            endpoint,
            ClientAuth::Fingerprint(fingerprint.clone()),
            identity,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    eprintln!("failed to reconnect within timeout: {e}");
                    std::process::exit(1);
                }
                eprintln!("connection failed, retrying in {backoff:?}: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        // Connection succeeded — reset backoff so the next retry starts fast.
        backoff = Duration::from_secs(1);

        match run_subscribe_session(&client).await {
            SessionOutcome::GracefulClose => return,
            SessionOutcome::Error(e) => {
                // Reset the deadline when we start reconnecting, not when the
                // session began — otherwise a long-lived session causes the
                // deadline to expire before the first retry.
                deadline = tokio::time::Instant::now() + RECONNECT_TIMEOUT;
                eprintln!("event stream lost, reconnecting in {backoff:?}: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
            SessionOutcome::Interrupted => return,
        }
    }
}

enum SessionOutcome {
    GracefulClose,
    Error(String),
    Interrupted,
}

// i[impl ctl.graceful-shutdown]
async fn run_subscribe_session(client: &OiClient) -> SessionOutcome {
    let req_bytes = serde_json::to_vec(&serde_json::json!({
        "method": "/events/subscribe",
        "params": {},
    }))
    .expect("serialisation");

    let (mut send, mut recv) = match client.open_bi().await {
        Ok(s) => s,
        Err(e) => return SessionOutcome::Error(format!("open_bi: {e}")),
    };

    if let Err(e) = send.write_all(&req_bytes).await {
        return SessionOutcome::Error(format!("write: {e}"));
    }
    let _ = send.finish();

    let resp = match recv.read_to_end(64 * 1024).await {
        Ok(r) => r,
        Err(e) => return SessionOutcome::Error(format!("read response: {e}")),
    };

    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&resp)
        && v.get("error").is_some()
    {
        eprintln!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return SessionOutcome::GracefulClose;
    }

    let mut event_stream = match client.accept_uni().await {
        Ok(s) => s,
        Err(e) => return SessionOutcome::Error(format!("accept_uni: {e}")),
    };

    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        tokio::select! {
            result = event_stream.read(&mut tmp) => {
                match result {
                    Ok(Some(n)) => {
                        buf.extend_from_slice(&tmp[..n]);
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
                    Ok(None) => return SessionOutcome::Error("event stream closed".into()),
                    Err(e) => return SessionOutcome::Error(format!("event stream: {e}")),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                return SessionOutcome::Interrupted;
            }
        }
    }
}
