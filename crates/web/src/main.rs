use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use parking_lot::{Mutex, RwLock};

mod auth;
mod config;
mod daemon;
mod http;
mod interfaces;
mod proxy;
mod spa;
mod state;
mod wt;
mod wt_cert;

use config::Config;
use daemon::DaemonConn;
use interfaces::resolve_bind_addrs;
use seedling_protocol::client::ClientAuth;
use state::AppState;
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

    let daemon = DaemonConn::connect(args.daemon_addr, daemon_auth, &key_file)
        .await
        .unwrap_or_else(|e| {
            eprintln!("error: daemon connection failed: {e}");
            std::process::exit(1);
        });
    daemon.probe().await.unwrap_or_else(|e| {
        eprintln!("error: daemon rejected connection: {e}");
        eprintln!(
            "hint: authorise this key in seedlingd: seedling-ctl oi add-key {} seedling-web",
            daemon.fingerprint
        );
        std::process::exit(1);
    });
    let daemon = Arc::new(daemon);

    let cert_store = Arc::new(RwLock::new(CertStore::new()));

    let (rotation_tx, rotation_rx) = tokio::sync::watch::channel(());

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
        daemon,
    };

    // Spawn cert rotation background task.
    tokio::spawn(wt::run_cert_rotation(Arc::clone(&cert_store), rotation_tx));

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
