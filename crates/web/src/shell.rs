use serde_json::json;
use tokio::io::AsyncWriteExt as _;

use crate::proxy::PeekedRequest;
use crate::state::AppState;

/// Read a single newline-terminated line from a Quinn RecvStream.
async fn read_shell_line(recv: &mut quinn::RecvStream) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        recv.read_exact(&mut byte)
            .await
            .map_err(|e| e.to_string())?;
        buf.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(buf);
        }
        if buf.len() > 64 * 1024 {
            return Err("handshake line too long".into());
        }
    }
}

/// Open a WT uni stream to the browser and write the 8-byte BE stream ID prefix.
///
/// The browser's `UniRouter` reads this prefix to route the remaining bytes
/// to the correct shell session handler.
async fn open_prefixed_wt_uni(
    wt_conn: &wtransport::Connection,
    stream_id: u64,
) -> Result<wtransport::SendStream, Box<dyn std::error::Error + Send + Sync>> {
    let mut send = wt_conn.open_uni().await?.await?;
    send.write_all(&stream_id.to_be_bytes()).await?;
    Ok(send)
}

/// RAII guard: fires `/shells/stop` when dropped if the session did not exit cleanly.
struct ShellSessionGuard {
    session_id: String,
    state: AppState,
    exited: bool,
}

impl ShellSessionGuard {
    fn new(session_id: String, state: AppState) -> Self {
        Self {
            session_id,
            state,
            exited: false,
        }
    }

    fn mark_exited(&mut self) {
        self.exited = true;
    }
}

impl Drop for ShellSessionGuard {
    fn drop(&mut self) {
        if !self.exited {
            let session_id = std::mem::take(&mut self.session_id);
            let state = self.state.clone();
            // Spawn a best-effort stop; errors are logged but not fatal.
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = state
                        .daemon
                        .request("/shells/stop", json!({"session_id": session_id}))
                        .await;
                });
            }
        }
    }
}

// w[routes.shells]
// w[shells.wire]
/// Handle a `/shells/start` request from the browser.
///
/// Opens the shell session on the daemon using the shared connection, wires up
/// the two server-initiated uni streams (stdout, stderr) to WT uni streams on
/// the browser's session, and relays stdin and the exit frame over the bidi.
pub async fn handle_shell_start(
    state: AppState,
    wt_conn: wtransport::Connection,
    mut wt_send: wtransport::SendStream,
    peeked: PeekedRequest,
) {
    // 1. Open a bidi on the shared daemon connection.
    let (mut daemon_send, mut daemon_recv) = match state.daemon.open_bi().await {
        Ok(streams) => streams,
        Err(e) => {
            tracing::error!("shells/start: daemon stream open failed: {e}");
            let msg = json!({"error": {"code": "daemon_unavailable", "message": e.to_string()}});
            let _ = wt_send.write_all((msg.to_string() + "\n").as_bytes()).await;
            let _ = wt_send.shutdown().await;
            return;
        }
    };

    // 2. Write the actor-injected request to the daemon.
    if let Err(e) = daemon_send.write_all(&peeked.modified_line).await {
        tracing::error!("shells/start: write request failed: {e}");
        return;
    }
    if let Err(e) = daemon_send.write_all(b"\n").await {
        tracing::error!("shells/start: write newline failed: {e}");
        return;
    }

    // 3. Read the handshake response from the daemon.
    let handshake_line = match read_shell_line(&mut daemon_recv).await {
        Ok(line) => line,
        Err(e) => {
            tracing::error!("shells/start: reading handshake failed: {e}");
            let msg = json!({"error": {"code": "protocol_error", "message": e}});
            let _ = wt_send.write_all((msg.to_string() + "\n").as_bytes()).await;
            let _ = wt_send.shutdown().await;
            return;
        }
    };

    // 4. Parse the handshake.
    let handshake: serde_json::Value = match serde_json::from_slice(&handshake_line) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("shells/start: parse handshake failed: {e}");
            return;
        }
    };

    // 5. Forward the handshake to the browser. On error from the daemon, forward
    //    verbatim then shut down.
    let _ = wt_send.write_all(&handshake_line).await;
    if handshake.get("error").is_some() {
        let _ = wt_send.shutdown().await;
        return;
    }

    let result = &handshake["result"];
    let session_id = result["session_id"].as_str().unwrap_or("").to_owned();
    let stdout_stream_id = result["stdout_stream_id"].as_u64().unwrap_or(0);
    let stderr_stream_id = result["stderr_stream_id"].as_u64().unwrap_or(0);

    // 6. Register for the two daemon uni streams BEFORE the streams can arrive
    //    (registrations must precede delivery to avoid parking races).
    let stdout_rx = state.daemon.register_uni(stdout_stream_id).await;
    let stderr_rx = state.daemon.register_uni(stderr_stream_id).await;

    // 7. Concurrently: open WT uni streams to the browser and await daemon uni streams.
    let wt_conn2 = wt_conn.clone();
    let (wt_stdout, wt_stderr, daemon_stdout, daemon_stderr) = tokio::join!(
        open_prefixed_wt_uni(&wt_conn, stdout_stream_id),
        open_prefixed_wt_uni(&wt_conn2, stderr_stream_id),
        stdout_rx,
        stderr_rx,
    );

    let mut wt_stdout = match wt_stdout {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("shells/start: open WT stdout uni failed: {e}");
            return;
        }
    };
    let mut wt_stderr = match wt_stderr {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("shells/start: open WT stderr uni failed: {e}");
            return;
        }
    };
    let mut daemon_stdout = match daemon_stdout {
        Ok(s) => s,
        Err(_) => {
            tracing::error!("shells/start: dispatcher dropped stdout receiver");
            return;
        }
    };
    let mut daemon_stderr = match daemon_stderr {
        Ok(s) => s,
        Err(_) => {
            tracing::error!("shells/start: dispatcher dropped stderr receiver");
            return;
        }
    };

    // 8. Run the session with a cleanup guard.
    let mut guard = ShellSessionGuard::new(session_id, state);

    // Wrap wt_send in an Arc<Mutex> so the exit-relay task can mark the guard.
    // (guard lives in the exit task; other tasks don't need it)

    // Task A: browser bidi recv → daemon bidi send (raw stdin relay).
    let stdin_fwd = async {
        let mut browser_recv = peeked.remaining;
        let _ = tokio::io::copy(&mut browser_recv, &mut daemon_send).await;
        let _ = daemon_send.finish();
    };

    // Task B: daemon stdout uni → WT stdout uni (already prefixed).
    let stdout_fwd = async {
        let _ = tokio::io::copy(&mut daemon_stdout, &mut wt_stdout).await;
        let _ = wt_stdout.shutdown().await;
    };

    // Task C: daemon stderr uni → WT stderr uni (already prefixed).
    let stderr_fwd = async {
        let _ = tokio::io::copy(&mut daemon_stderr, &mut wt_stderr).await;
        let _ = wt_stderr.shutdown().await;
    };

    // Task D: daemon bidi recv → accumulate exit frame → forward to browser bidi.
    // w[impl shells.exit]
    let exit_relay = async {
        let mut exit_buf = Vec::with_capacity(64);
        let mut byte = [0u8; 1];
        while let Ok(Some(1)) = daemon_recv.read(&mut byte).await {
            exit_buf.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        if !exit_buf.is_empty() {
            let _ = wt_send.write_all(&exit_buf).await;
        }
        let _ = wt_send.shutdown().await;
        true // did exit
    };

    let (_, _, _, did_exit) = tokio::join!(stdin_fwd, stdout_fwd, stderr_fwd, exit_relay);

    if did_exit {
        guard.mark_exited();
    }
}
