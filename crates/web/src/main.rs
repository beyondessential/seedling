use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use parking_lot::{Mutex, RwLock};

mod actor_activity;
mod auth;
mod config;
mod daemon;
mod event_broker;
mod http;
mod interfaces;
mod proxy;
mod shell;
mod spa;
mod state;
mod web_sessions;
mod wt;
mod wt_cert;

use actor_activity::ActorActivityRegistry;
use config::Config;
use daemon::DaemonConn;
use event_broker::{EventBroker, run_event_broker};
use interfaces::resolve_bind_addrs;
use seedling_protocol::client::ClientAuth;
use state::AppState;
use web_sessions::WebSessionRegistry;
use wt_cert::CertStore;

const DEFAULT_HTTP_PORT: u16 = 8080;
const DEFAULT_WT_PORT: u16 = 7893;

#[derive(Parser)]
#[command(name = "seedling-web")]
struct Args {
    #[command(flatten)]
    logging: lloggs::LoggingArgs,

    /// Path to the web config file (TOML).
    #[arg(long)]
    config: Option<PathBuf>,

    // w[bind]
    /// Network interface(s) to bind on (comma-separated names).
    #[arg(long, value_delimiter = ',')]
    interface: Vec<String>,

    /// Explicit listen address(es). May be repeated.
    #[arg(long)]
    listen: Vec<SocketAddr>,

    /// HTTP listener port (used with --interface). Conflicts with --listen.
    #[arg(long, default_value_t = DEFAULT_HTTP_PORT, conflicts_with = "listen")]
    http_port: u16,

    /// WebTransport listener port (used with --interface). Conflicts with --listen.
    #[arg(long, default_value_t = DEFAULT_WT_PORT, conflicts_with = "listen")]
    wt_port: u16,

    // w[auth.tailscale]
    /// Trust Tailscale identity headers for authentication.
    #[arg(long)]
    trust_tailscale_headers: bool,

    // w[auth.dev]
    /// Bypass all authentication. Only allowed on loopback addresses.
    #[arg(long)]
    dev_no_auth: bool,

    /// Proxy the SPA to a Vite dev server on this port instead of serving embedded assets.
    #[arg(long)]
    vite_port: Option<u16>,

    /// Address of the seedlingd OI endpoint to proxy.
    #[arg(long, default_value = "[::1]:7891")]
    daemon_addr: std::net::SocketAddr,

    /// SHA-256 SPKI fingerprint (hex) of the daemon to pin.
    #[arg(long)]
    #[cfg_attr(debug_assertions, arg(conflicts_with = "daemon_trust_any"))]
    daemon_fingerprint: Option<String>,

    /// Skip daemon key verification (development only).
    #[cfg(debug_assertions)]
    #[arg(long, conflicts_with = "daemon_fingerprint")]
    daemon_trust_any: bool,

    /// Path to the web binary's persistent client key file.
    #[arg(long)]
    key_file: Option<std::path::PathBuf>,
}

// w[daemon.connect-retry]
async fn connect_daemon_with_retry(
    addr: std::net::SocketAddr,
    auth: ClientAuth,
    key_file: &std::path::Path,
) -> DaemonConn {
    let mut backoff = Duration::from_secs(1);
    loop {
        match DaemonConn::connect(addr, auth.clone(), key_file).await {
            Err(e) => {
                tracing::warn!(
                    "daemon connection failed: {e} — retrying in {}s",
                    backoff.as_secs()
                );
            }
            Ok(daemon) => match daemon.probe().await {
                Ok(()) => return daemon,
                Err(e) => {
                    tracing::warn!(
                        fingerprint = %daemon.fingerprint,
                        "daemon probe failed: {e} — if this key is not yet authorised, run: seedling-ctl user add {} seedling-web — retrying in {}s",
                        daemon.fingerprint,
                        backoff.as_secs(),
                    );
                }
            },
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let mut _guard = lloggs::PreArgs::parse_with_env("SEEDLING_WEB_LOG")
        .setup()
        .unwrap_or_else(|e| {
            eprintln!("logging setup: {e}");
            None
        });
    if _guard.is_none() {
        _guard = args
            .logging
            .setup(|v| match v {
                0 => "info",
                1 => "info,seedling_web=debug",
                2 => "debug",
                _ => "trace",
            })
            .ok();
    }

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("ring crypto provider already installed");

    let cfg = args
        .config
        .as_deref()
        .map(|p| {
            Config::from_file(p).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            })
        })
        .unwrap_or_default();

    // Resolve HTTP bind addresses.
    let http_addrs = resolve_bind_addrs(&args.interface, &args.listen, args.http_port)
        .unwrap_or_else(|e| {
            eprintln!("error: {e}");
            std::process::exit(1);
        });

    // Resolve WT bind addresses (same interfaces, different port).
    let wt_port = if args.listen.is_empty() {
        args.wt_port
    } else {
        DEFAULT_WT_PORT
    };
    let wt_addrs = resolve_bind_addrs(&args.interface, &args.listen, wt_port).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });

    // w[auth.dev] — dev-no-auth must not be used with non-loopback addresses.
    if args.dev_no_auth {
        for addr in http_addrs.iter().chain(wt_addrs.iter()) {
            if !interfaces::is_loopback(addr) {
                eprintln!("error: --dev-no-auth is not allowed with non-loopback address {addr}");
                std::process::exit(1);
            }
        }
    }

    let session_lifetime = Duration::from_secs(cfg.auth.session_lifetime_secs);
    let password_hash = cfg.auth.password_hash;

    #[cfg(debug_assertions)]
    let trust_any = args.daemon_trust_any;
    #[cfg(not(debug_assertions))]
    let trust_any = false;

    let daemon_auth = if trust_any {
        tracing::warn!("--daemon-trust-any: skipping daemon key verification");
        ClientAuth::TrustAny
    } else if let Some(fp) = args.daemon_fingerprint {
        ClientAuth::Fingerprint(fp)
    } else if cfg!(debug_assertions) {
        tracing::warn!("no --daemon-fingerprint; trusting any daemon key (debug build)");
        ClientAuth::TrustAny
    } else {
        eprintln!("error: --daemon-fingerprint is required");
        std::process::exit(1);
    };

    let key_file = args.key_file.unwrap_or_else(DaemonConn::default_key_path);

    // w[daemon.connect-retry]
    let daemon =
        Arc::new(connect_daemon_with_retry(args.daemon_addr, daemon_auth, &key_file).await);

    let cert_store = Arc::new(RwLock::new(CertStore::new()));

    let (rotation_tx, rotation_rx) = tokio::sync::watch::channel(());

    let actor_activity = Arc::new(ActorActivityRegistry::new());
    let event_broker = EventBroker::new(Arc::clone(&actor_activity));

    let state = AppState {
        trust_tailscale: args.trust_tailscale_headers,
        dev_no_auth: args.dev_no_auth,
        cert_store: Arc::clone(&cert_store),
        sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        wt_tokens: Arc::new(Mutex::new(std::collections::HashMap::new())),
        session_lifetime,
        password_hash,
        wt_port,
        vite_port: args.vite_port,
        daemon: Arc::clone(&daemon),
        event_broker: Arc::clone(&event_broker),
        web_sessions: Arc::new(WebSessionRegistry::new()),
        actor_activity,
    };

    // Spawn cert rotation background task.
    tokio::spawn(wt::run_cert_rotation(Arc::clone(&cert_store), rotation_tx));

    // w[impl sessions.stale-cutoff]
    // Spawn the stale-session reaper.
    tokio::spawn(wt::run_session_reaper(
        Arc::clone(&state.web_sessions),
        Arc::clone(&state.event_broker),
    ));

    // Spawn the event broker — maintains a single daemon subscription and fans
    // out to all connected web clients.
    tokio::spawn(run_event_broker(event_broker, daemon));

    // Spawn HTTP servers.
    let router = http::router(state.clone());
    for addr in &http_addrs {
        let addr = *addr;
        let app = router.clone();
        tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("error: HTTP bind {addr}: {e}");
                    std::process::exit(1);
                });
            tracing::info!(%addr, "HTTP server listening");
            axum::serve(listener, app).await.expect("HTTP server");
        });
    }

    // Spawn WT servers.
    for addr in wt_addrs {
        let state2 = state.clone();
        let rx = rotation_rx.clone();
        tokio::spawn(async move {
            wt::run_wt_server(addr, state2, rx).await;
        });
    }

    // Wait indefinitely.
    tokio::signal::ctrl_c().await.expect("ctrl-c handler");
    tracing::info!("shutting down");
}
