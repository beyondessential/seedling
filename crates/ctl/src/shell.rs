use std::time::Duration;

use seedling_protocol::client::OiClient;
use seedling_protocol::names::AppName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

/// SIGTERM source (Unix only). On other platforms `recv()` never resolves, so
/// the graceful-terminate select branch simply never fires.
struct TermSignal {
    #[cfg(unix)]
    inner: tokio::signal::unix::Signal,
}

impl TermSignal {
    #[cfg(unix)]
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            inner: signal(SignalKind::terminate())?,
        })
    }
    #[cfg(not(unix))]
    fn new() -> std::io::Result<Self> {
        Ok(Self {})
    }

    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            self.inner.recv().await;
        }
        #[cfg(not(unix))]
        {
            std::future::pending::<()>().await;
        }
    }
}

/// Terminal-resize notifier. On Unix it fires on SIGWINCH; elsewhere there is
/// no resize signal — and the raw stdin relay rules out crossterm's event
/// stream (it would consume the input bytes) — so it polls the terminal size on
/// a fixed interval. Either way the caller only sends a resize when the size
/// actually changed.
struct ResizeWatcher {
    #[cfg(unix)]
    sig: tokio::signal::unix::Signal,
    #[cfg(not(unix))]
    tick: tokio::time::Interval,
}

impl ResizeWatcher {
    #[cfg(unix)]
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            sig: signal(SignalKind::window_change())?,
        })
    }
    #[cfg(not(unix))]
    fn new() -> std::io::Result<Self> {
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Ok(Self { tick })
    }

    async fn changed(&mut self) {
        #[cfg(unix)]
        {
            self.sig.recv().await;
        }
        #[cfg(not(unix))]
        {
            self.tick.tick().await;
        }
    }
}

/// Drop guard that restores the terminal from raw mode.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Read a single newline-terminated line from a Quinn RecvStream.
pub async fn read_shell_line(recv: &mut quinn::RecvStream) -> Result<Vec<u8>, String> {
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
            return Err("server response line too long".into());
        }
    }
}

pub async fn open_shell(
    client: &OiClient,
    app: AppName,
    name: String,
    params: serde_json::Map<String, serde_json::Value>,
) -> i32 {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut shell_params = serde_json::json!({
        "app": app, "name": name, "rows": rows, "cols": cols,
    });
    if !params.is_empty() {
        shell_params["params"] = serde_json::Value::Object(params);
    }
    run_shell(client, "/shells/start", shell_params).await
}

/// Open a volume shell session against `/volumes/shell`. `read_only` forces
/// every mount into the container as read-only regardless of the underlying
/// volume kind.
// i[impl volumes.shell]
pub async fn open_volume_shell(
    client: &OiClient,
    volumes: Vec<serde_json::Value>,
    read_only: bool,
) -> i32 {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let params = serde_json::json!({
        "volumes": volumes,
        "rows": rows,
        "cols": cols,
        "read_only": read_only,
    });
    run_shell(client, "/volumes/shell", params).await
}

/// Drives the shared shell wire protocol: open a bidi, send the start request,
/// parse the handshake, accept the two uni streams, switch the local terminal
/// into raw mode, and pump bytes until the server emits its exit frame.
async fn run_shell(client: &OiClient, method: &str, request_params: serde_json::Value) -> i32 {
    // 1. Open the session bidi stream (kept open for stdin after the handshake).
    let (mut session_send, mut session_recv) = client.open_bi().await.unwrap_or_else(|e| {
        eprintln!("error opening shell stream: {e}");
        std::process::exit(1);
    });

    // 2. Send the start request (newline-terminated JSON).
    {
        let mut req = serde_json::to_vec(&serde_json::json!({
            "method": method,
            "params": request_params,
        }))
        .expect("serialisation never fails");
        req.push(b'\n');
        if let Err(e) = session_send.write_all(&req).await {
            eprintln!("error sending {method}: {e}");
            return 1;
        }
    }

    // 4. Read the handshake response line.
    let handshake_bytes = match read_shell_line(&mut session_recv).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error reading handshake: {e}");
            return 1;
        }
    };
    let handshake: serde_json::Value = match serde_json::from_slice(&handshake_bytes) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("invalid handshake: {e}");
            return 1;
        }
    };
    if let Some(err) = handshake.get("error") {
        let code = err.get("code").and_then(|c| c.as_str()).unwrap_or("error");
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
        eprintln!("[{code}] {msg}");
        return 1;
    }
    let result = &handshake["result"];
    let session_id = result["session_id"].as_str().unwrap_or("").to_owned();
    let stdout_stream_id = result["stdout_stream_id"].as_u64().unwrap_or(0);
    let stderr_stream_id = result["stderr_stream_id"].as_u64().unwrap_or(0);

    // 5. Accept the two server-initiated uni streams (stdout and stderr).
    //    The server opens them before writing the handshake, so they should
    //    already be available.
    let accept_a = client.accept_uni().await;
    let accept_b = client.accept_uni().await;
    let (s_a, s_b) = match (accept_a, accept_b) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("error accepting shell streams: {e}");
            return 1;
        }
    };
    let (mut stdout_recv, mut stderr_recv) = if s_a.id().index() == stdout_stream_id {
        (s_a, s_b)
    } else if s_b.id().index() == stdout_stream_id {
        (s_b, s_a)
    } else {
        // Fallback: treat first as stdout, second as stderr.
        (s_a, s_b)
    };
    let _ = stderr_stream_id; // identified above; stderr is empty in PTY mode

    // 6. Enter raw mode; the guard restores it on any early return or panic.
    if let Err(e) = crossterm::terminal::enable_raw_mode() {
        eprintln!("could not enable raw mode: {e}");
        return 1;
    }
    let _raw = RawModeGuard;

    // 7. Resize notifier (SIGWINCH on Unix, size polling elsewhere) and the
    //    Unix graceful-terminate signal.
    let mut resize = match ResizeWatcher::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("could not install resize watcher: {e}");
            return 1;
        }
    };
    let mut sigterm = match TermSignal::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not install SIGTERM handler: {e}");
            return 1;
        }
    };
    let mut last_size = crossterm::terminal::size().unwrap_or((80, 24));

    // 8. I/O relay loop.
    //    - local stdin  → session_send (raw bytes)
    //    - stdout_recv  → local stdout
    //    - stderr_recv  → local stderr
    //    - session_recv → exit frame accumulation
    //    - SIGWINCH     → /shells/resize control request
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();

    let mut stdin_buf = vec![0u8; 4096];
    let mut stdout_buf = vec![0u8; 4096];
    let mut stderr_buf = vec![0u8; 4096];
    let mut exit_byte = [0u8; 1];
    let mut exit_buf = Vec::<u8>::new();

    let mut stdin_done = false;
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut shutdown_deadline: Option<Instant> = None;

    // i[impl ctl.graceful-shutdown]
    let exit_code = loop {
        tokio::select! {
            // stdin: local terminal → container
            n = stdin.read(&mut stdin_buf), if !stdin_done => {
                match n {
                    Ok(0) | Err(_) => {
                        stdin_done = true;
                        let _ = session_send.finish();
                    }
                    Ok(n) => {
                        if session_send.write_all(&stdin_buf[..n]).await.is_err() {
                            break -1;
                        }
                    }
                }
            }
            // stdout stream: container output → local terminal
            n = stdout_recv.read(&mut stdout_buf), if !stdout_done => {
                match n {
                    Ok(Some(n)) if n > 0 => {
                        stdout.write_all(&stdout_buf[..n]).await.ok();
                        stdout.flush().await.ok();
                    }
                    _ => stdout_done = true,
                }
            }
            // stderr stream: container stderr → local stderr (empty in PTY mode)
            n = stderr_recv.read(&mut stderr_buf), if !stderr_done => {
                match n {
                    Ok(Some(n)) if n > 0 => {
                        stderr.write_all(&stderr_buf[..n]).await.ok();
                        stderr.flush().await.ok();
                    }
                    _ => stderr_done = true,
                }
            }
            // session stream server→client: accumulate the exit frame
            n = session_recv.read(&mut exit_byte) => {
                match n {
                    Ok(Some(n)) if n > 0 => {
                        exit_buf.push(exit_byte[0]);
                        if exit_byte[0] == b'\n' {
                            if let Ok(v) =
                                serde_json::from_slice::<serde_json::Value>(&exit_buf)
                                && let Some(code) = v.get("exit_code").and_then(|c| c.as_i64())
                            {
                                break code as i32;
                            }
                            exit_buf.clear();
                        }
                    }
                    _ => break -1,
                }
            }
            // Terminal resize: forward new dimensions to the server, but only
            // when they actually changed (the non-Unix watcher polls on a timer).
            _ = resize.changed() => {
                let (new_cols, new_rows) = crossterm::terminal::size().unwrap_or((80, 24));
                if (new_cols, new_rows) != last_size {
                    last_size = (new_cols, new_rows);
                    client
                        .request(
                            "/shells/resize",
                            serde_json::json!({
                                "session_id": session_id,
                                "rows": new_rows,
                                "cols": new_cols,
                            }),
                        )
                        .await
                        .ok();
                }
            }
            // SIGTERM: send ETX to the remote shell, then drain with a timeout.
            _ = sigterm.recv(), if shutdown_deadline.is_none() => {
                let _ = session_send.write_all(b"\x03").await;
                shutdown_deadline = Some(Instant::now() + Duration::from_secs(5));
            }
            _ = tokio::time::sleep_until(shutdown_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(86400))), if shutdown_deadline.is_some() => {
                break 130;
            }
        }
    };

    // Restore the terminal explicitly before process::exit bypasses drop glue.
    drop(_raw);
    exit_code
}
