use std::{collections::HashMap, io::Write, net::SocketAddr, path::PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use clap::{Parser, Subcommand};
use lloggs::LoggingArgs;
use seedling::oi::{
    client::{ClientAuth, ClientError, OiClient},
    keys::ClientIdentity,
};

mod known_hosts {
    use std::{
        collections::HashMap,
        io,
        path::{Path, PathBuf},
    };

    pub(super) enum Status {
        Match,
        Unknown,
        Mismatch { expected: String },
    }

    pub(super) struct KnownHosts {
        path: PathBuf,
        entries: HashMap<String, String>,
    }

    impl KnownHosts {
        pub(super) fn default_path() -> PathBuf {
            dirs::state_dir()
                .or_else(dirs::data_local_dir)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("seedling")
                .join("known_hosts")
        }

        pub(super) fn load(path: &Path) -> io::Result<Self> {
            let mut entries = HashMap::new();
            if path.exists() {
                for line in std::fs::read_to_string(path)?.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let mut parts = line.splitn(2, ' ');
                    if let (Some(ep), Some(fp)) = (parts.next(), parts.next()) {
                        entries.insert(ep.to_owned(), fp.to_owned());
                    }
                }
            }
            Ok(Self {
                path: path.to_owned(),
                entries,
            })
        }

        pub(super) fn empty(path: PathBuf) -> Self {
            Self {
                path,
                entries: HashMap::new(),
            }
        }

        pub(super) fn check(&self, endpoint: &str, fingerprint: &str) -> Status {
            match self.entries.get(endpoint) {
                Some(saved) if saved == fingerprint => Status::Match,
                Some(saved) => Status::Mismatch {
                    expected: saved.clone(),
                },
                None => Status::Unknown,
            }
        }

        pub(super) fn add(&mut self, endpoint: &str, fingerprint: &str) {
            self.entries
                .insert(endpoint.to_owned(), fingerprint.to_owned());
        }

        pub(super) fn save(&self) -> io::Result<()> {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out =
                String::from("# seedling-ctl known hosts\n# endpoint sha256-fingerprint\n");
            let mut pairs: Vec<_> = self.entries.iter().collect();
            pairs.sort_by_key(|(ep, _)| ep.as_str());
            for (ep, fp) in pairs {
                out.push_str(ep);
                out.push(' ');
                out.push_str(fp);
                out.push('\n');
            }
            std::fs::write(&self.path, out)
        }
    }
}

#[derive(Parser)]
#[command(name = "seedling-ctl", about = "Seedling operator interface CLI")]
struct Cli {
    /// OI server address
    #[arg(long, default_value = "[::1]:7891")]
    endpoint: SocketAddr,

    /// SHA-256 SPKI fingerprint (hex) to pin
    #[arg(long, conflicts_with = "trust_any")]
    fingerprint: Option<String>,

    /// Skip server key verification (development only)
    #[arg(long, conflicts_with = "fingerprint")]
    trust_any: bool,

    #[command(flatten)]
    logging: LoggingArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show instance status
    Status,
    /// List registered apps
    ListApps,
    /// Describe an app
    DescribeApp { name: String },
    /// Register an app from a script file
    RegisterApp { name: String, script_file: PathBuf },
    /// Deregister an app
    DeregisterApp { name: String },
    /// Uninstall an app (stop all resources). The app can be deregistered once done.
    UninstallApp { name: String },
    /// Update an app's script
    UpdateApp { name: String, script_file: PathBuf },
    /// Set a param value
    SetParam {
        app: String,
        name: String,
        value: String,
    },
    /// Unset a param value
    UnsetParam { app: String, name: String },
    /// Invoke a lifecycle action
    InvokeAction { app: String, name: String },
    /// Invoke the install action
    InvokeInstall {
        app: String,
        /// Requirements as key=value
        #[arg(long = "req")]
        requirements: Vec<String>,
    },
    /// List faults
    ListFaults {
        #[arg(long)]
        app: Option<String>,
    },
    /// List open shell sessions
    ListShells {
        #[arg(long)]
        app: Option<String>,
    },
    /// Stop a shell session
    StopShell { session_id: String },
    /// List port forwards
    ListForwards {
        #[arg(long)]
        app: Option<String>,
    },
    /// Subscribe to event feed (streams JSON to stdout)
    Subscribe,
    /// Open an interactive shell session
    OpenShell { app: String, name: String },
    /// Forward a local port to a service
    ForwardPort {
        app: String,
        service: String,
        port: u16,
        #[arg(long)]
        proto: String,
        #[arg(long)]
        local_port: Option<u16>,
    },
    /// Print this client's key fingerprint (no server connection needed)
    PrintFingerprint,
    /// List authorized client keys on the server
    ListKeys,
    /// Authorize a client key on the server
    AuthorizeKey {
        /// Fingerprint to authorize
        fingerprint: String,
        /// Human-readable label for this key
        #[arg(long)]
        label: String,
    },
    /// Revoke an authorized client key on the server
    RevokeKey {
        /// Fingerprint to revoke
        fingerprint: String,
    },
}

#[tokio::main]
async fn main() {
    let mut _guard = lloggs::PreArgs::parse_with_env("SEEDLING_LOG")
        .setup()
        .unwrap_or_else(|e| {
            tracing::warn!("logging setup: {e}");
            None
        });

    let cli = Cli::parse();

    if _guard.is_none() {
        _guard = cli
            .logging
            .setup(|v| match v {
                0 => "seedling=info,seedling_ctl=info,warn",
                1 => "seedling=debug,seedling_ctl=debug,warn",
                2 => "info",
                3 => "seedling=debug,seedling_ctl=debug,info",
                4 => "debug",
                5 => "seedling=trace,seedling_ctl=trace,debug",
                _ => "trace",
            })
            .map(Some)
            .unwrap_or_else(|e| {
                tracing::warn!("logging setup: {e}");
                None
            });
    }

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("ring crypto provider already installed");

    if std::env::var_os("SSLKEYLOGFILE").is_some() {
        tracing::warn!("SSLKEYLOGFILE is set — TLS session keys are being logged to disk");
    }

    // Load (or generate) the client identity early; PrintFingerprint needs it
    // before any server connection is attempted.
    let key_path = ClientIdentity::default_path();
    let (identity, is_new) = ClientIdentity::load_or_generate(&key_path).unwrap_or_else(|e| {
        tracing::error!(
            "could not load/generate client key at {}: {e}",
            key_path.display()
        );
        std::process::exit(1);
    });
    if is_new {
        tracing::info!(
            path = %key_path.display(),
            fingerprint = %identity.fingerprint,
            "generated new client key"
        );
    }

    // Handle commands that don't need a server connection.
    if let Command::PrintFingerprint = &cli.command {
        println!("{}", identity.fingerprint);
        eprintln!("Client key: {}", key_path.display());
        eprintln!(
            "\nTo bootstrap a new server, add this line to $data_dir/authorized_keys:\n  {} my-label",
            identity.fingerprint
        );
        return;
    }

    let client;

    if cli.trust_any {
        client = OiClient::connect(cli.endpoint, ClientAuth::TrustAny, &identity)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });
    } else if let Some(fp) = cli.fingerprint {
        client = OiClient::connect(cli.endpoint, ClientAuth::Fingerprint(fp), &identity)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });
    } else {
        let kh_path = known_hosts::KnownHosts::default_path();
        let mut kh = known_hosts::KnownHosts::load(&kh_path).unwrap_or_else(|e| {
            tracing::warn!("could not read {}: {e}", kh_path.display());
            known_hosts::KnownHosts::empty(kh_path.clone())
        });

        let (c, fp) = OiClient::connect_pinning(cli.endpoint, &identity)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });

        let ep = cli.endpoint.to_string();
        match kh.check(&ep, &fp) {
            known_hosts::Status::Match => {}
            known_hosts::Status::Unknown => {
                let mut stderr = std::io::stderr();
                writeln!(
                    stderr,
                    "The authenticity of host '{ep}' can't be established."
                )
                .ok();
                writeln!(stderr, "Fingerprint: {fp}").ok();
                write!(stderr, "Continue connecting? (yes/no) ").ok();
                stderr.flush().ok();

                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                if line.trim() != "yes" {
                    eprintln!("Aborted.");
                    std::process::exit(1);
                }

                kh.add(&ep, &fp);
                match kh.save() {
                    Ok(()) => eprintln!(
                        "Permanently added '{ep}' to known hosts ({}).",
                        kh_path.display()
                    ),
                    Err(e) => tracing::warn!("could not save known hosts: {e}"),
                }
            }
            known_hosts::Status::Mismatch { expected } => {
                let bar = "@".repeat(60);
                eprintln!("{bar}");
                eprintln!("@ WARNING: REMOTE HOST FINGERPRINT HAS CHANGED!            @");
                eprintln!("{bar}");
                eprintln!("Someone could be eavesdropping on you right now!");
                eprintln!("Expected fingerprint for '{ep}':");
                eprintln!("  {expected}");
                eprintln!("Received:");
                eprintln!("  {fp}");
                eprintln!(
                    "Remove the stale entry from {} to proceed.",
                    kh_path.display()
                );
                std::process::exit(1);
            }
        }

        client = c;
    }

    dispatch(&client, cli.command).await;
}

async fn dispatch(client: &OiClient, cmd: Command) {
    match cmd {
        Command::PrintFingerprint => unreachable!("handled before connect"),
        Command::ListKeys => {
            print_result(client.request("ListKeys", serde_json::json!({})).await);
        }
        Command::AuthorizeKey { fingerprint, label } => {
            print_result(
                client
                    .request(
                        "AuthorizeKey",
                        serde_json::json!({ "fingerprint": fingerprint, "label": label }),
                    )
                    .await,
            );
        }
        Command::RevokeKey { fingerprint } => {
            print_result(
                client
                    .request(
                        "RevokeKey",
                        serde_json::json!({ "fingerprint": fingerprint }),
                    )
                    .await,
            );
        }
        Command::Status => {
            print_result(client.request("GetStatus", serde_json::json!({})).await);
        }
        Command::ListApps => {
            print_result(client.request("ListApps", serde_json::json!({})).await);
        }
        Command::DescribeApp { name } => {
            print_result(
                client
                    .request("DescribeApp", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        Command::RegisterApp { name, script_file } => {
            let script = read_script_file(&script_file);
            print_result(
                client
                    .request(
                        "RegisterApp",
                        serde_json::json!({ "name": name, "script": script }),
                    )
                    .await,
            );
        }
        Command::DeregisterApp { name } => {
            print_result(
                client
                    .request("DeregisterApp", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        Command::UninstallApp { name } => {
            print_result(
                client
                    .request("UninstallApp", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        Command::UpdateApp { name, script_file } => {
            let script = read_script_file(&script_file);
            print_result(
                client
                    .request(
                        "UpdateApp",
                        serde_json::json!({ "name": name, "script": script }),
                    )
                    .await,
            );
        }
        Command::SetParam { app, name, value } => {
            print_result(
                client
                    .request(
                        "SetParam",
                        serde_json::json!({ "app": app, "name": name, "value": value }),
                    )
                    .await,
            );
        }
        Command::UnsetParam { app, name } => {
            print_result(
                client
                    .request(
                        "UnsetParam",
                        serde_json::json!({ "app": app, "name": name }),
                    )
                    .await,
            );
        }
        Command::InvokeAction { app, name } => {
            print_result(
                client
                    .request(
                        "InvokeAction",
                        serde_json::json!({ "app": app, "name": name }),
                    )
                    .await,
            );
        }
        Command::InvokeInstall { app, requirements } => {
            let reqs: HashMap<String, String> = requirements
                .iter()
                .filter_map(|r| {
                    let mut parts = r.splitn(2, '=');
                    Some((parts.next()?.to_owned(), parts.next()?.to_owned()))
                })
                .collect();
            print_result(
                client
                    .request(
                        "InvokeInstall",
                        serde_json::json!({ "app": app, "requirements": reqs }),
                    )
                    .await,
            );
        }
        Command::ListFaults { app } => {
            print_result(
                client
                    .request("ListFaults", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        Command::ListShells { app } => {
            print_result(
                client
                    .request("ListShells", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        Command::StopShell { session_id } => {
            print_result(
                client
                    .request("StopShell", serde_json::json!({ "session_id": session_id }))
                    .await,
            );
        }
        Command::ListForwards { app } => {
            print_result(
                client
                    .request("ListForwards", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        Command::Subscribe => {
            subscribe(client).await;
        }
        Command::OpenShell { app, name } => {
            let code = open_shell(client, app, name).await;
            std::process::exit(code);
        }
        Command::ForwardPort {
            app,
            service,
            port,
            proto,
            local_port,
        } => {
            forward_port(client, app, service, port, proto, local_port).await;
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
            return Err("server response line too long".into());
        }
    }
}

async fn open_shell(client: &OiClient, app: String, name: String) -> i32 {
    // 1. Current terminal dimensions.
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));

    // 2. Open the session bidi stream (kept open for stdin after the handshake).
    let (mut session_send, mut session_recv) = client.open_bi().await.unwrap_or_else(|e| {
        eprintln!("error opening shell stream: {e}");
        std::process::exit(1);
    });

    // 3. Send the OpenShell request (newline-terminated JSON).
    {
        let mut req = serde_json::to_vec(&serde_json::json!({
            "method": "OpenShell",
            "params": { "app": app, "name": name, "rows": rows, "cols": cols },
        }))
        .expect("serialisation never fails");
        req.push(b'\n');
        if let Err(e) = session_send.write_all(&req).await {
            eprintln!("error sending OpenShell: {e}");
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

    // 7. SIGWINCH handler for terminal resize.
    let mut sigwinch =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("could not install SIGWINCH handler: {e}");
                return 1;
            }
        };

    // 8. I/O relay loop.
    //    - local stdin  → session_send (raw bytes)
    //    - stdout_recv  → local stdout
    //    - stderr_recv  → local stderr
    //    - session_recv → exit frame accumulation
    //    - SIGWINCH     → ResizeShell control request
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
            // SIGWINCH: forward new terminal dimensions to the server
            _ = sigwinch.recv() => {
                let (new_cols, new_rows) = crossterm::terminal::size().unwrap_or((80, 24));
                client
                    .request(
                        "ResizeShell",
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
    };

    // Restore the terminal explicitly before process::exit bypasses drop glue.
    drop(_raw);
    exit_code
}

async fn subscribe(client: &OiClient) {
    // Send the Subscribe request on a normal bidi stream.
    let req_bytes = serde_json::to_vec(&serde_json::json!({
        "method": "Subscribe",
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

async fn forward_port(
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
    let resp_bytes = read_shell_line(&mut ctrl_recv).await.unwrap_or_else(|e| {
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

fn print_result(result: Result<serde_json::Value, ClientError>) {
    match result {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => {
            tracing::error!("{e}");
            std::process::exit(1);
        }
    }
}

fn read_script_file(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        tracing::error!("cannot read {}: {e}", path.display());
        std::process::exit(1);
    })
}
