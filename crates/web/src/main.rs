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

const DEFAULT_HTTP_PORT: u16 = 7894;
const DEFAULT_WT_PORT: u16 = 7893;

#[derive(Parser)]
#[command(name = "seedling-web", version)]
struct Args {
    #[command(flatten)]
    logging: lloggs::LoggingArgs,

    /// Path to the web config file (TOML).
    #[arg(long)]
    config: Option<PathBuf>,

    // spec: WEB#listen-addresses
    /// Network interface(s) to bind on (comma-separated names).
    #[arg(long, value_delimiter = ',')]
    interface: Vec<String>,

    /// Explicit listen address(es). May be repeated.
    #[arg(long)]
    listen: Vec<SocketAddr>,

    // spec: WEB#listen-addresses
    /// Interface(s) to bind the WebTransport listener on, overriding
    /// `--interface`/`--listen` for WebTransport only (comma-separated names).
    /// WebTransport cannot be carried by an HTTP reverse proxy, so this exposes
    /// it directly to clients (e.g. on the tailnet) while the HTTP listener
    /// stays on loopback behind `tailscale serve`.
    #[arg(long, value_delimiter = ',')]
    wt_interface: Vec<String>,

    // spec: WEB#listen-addresses
    /// Explicit WebTransport listen address(es), overriding
    /// `--interface`/`--listen` for WebTransport only. May be repeated.
    #[arg(long)]
    wt_listen: Vec<SocketAddr>,

    /// HTTP listener port (used with --interface). Conflicts with --listen.
    #[arg(long, default_value_t = DEFAULT_HTTP_PORT, conflicts_with = "listen")]
    http_port: u16,

    /// WebTransport listener port (used with --interface). The web interface
    /// does not start when it is given with an address the WebTransport
    /// listener binds explicitly.
    #[arg(long)]
    wt_port: Option<u16>,

    // spec: WEB#tailscale-identity
    /// Trust Tailscale identity headers for authentication.
    #[arg(long)]
    trust_tailscale_headers: bool,

    // spec: WEB#development-bypass
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

    // spec: WEB#daemon-connection
    /// Path to a file containing the daemon's SHA-256 SPKI fingerprint (hex) to
    /// pin. Re-read on each connection attempt, so the daemon may publish it
    /// after this process starts (see the daemon's `oi.fingerprint` file).
    #[arg(long, conflicts_with = "daemon_fingerprint")]
    #[cfg_attr(debug_assertions, arg(conflicts_with = "daemon_trust_any"))]
    daemon_fingerprint_file: Option<std::path::PathBuf>,

    /// Skip daemon key verification (development only).
    #[cfg(debug_assertions)]
    #[arg(long, conflicts_with = "daemon_fingerprint")]
    daemon_trust_any: bool,

    /// Path to the web binary's persistent client key file.
    #[arg(long)]
    key_file: Option<std::path::PathBuf>,
}

/// How the daemon's identity is pinned for each connection attempt.
enum DaemonPin {
    /// A fixed auth mode resolved once at startup (explicit fingerprint,
    /// or trust-any in debug builds).
    Fixed(ClientAuth),
    /// A file the daemon publishes its fingerprint to, re-read per attempt.
    File(std::path::PathBuf),
}

impl DaemonPin {
    /// Resolve the auth for one connection attempt. Returns `None` when a
    /// fingerprint file is configured but not yet readable or empty, so the
    /// caller retries instead of failing.
    fn resolve(&self) -> Option<ClientAuth> {
        match self {
            DaemonPin::Fixed(auth) => Some(auth.clone()),
            DaemonPin::File(path) => {
                let contents = std::fs::read_to_string(path).ok()?;
                let fingerprint = contents.trim();
                (!fingerprint.is_empty()).then(|| ClientAuth::Fingerprint(fingerprint.to_owned()))
            }
        }
    }
}

// spec: WEB#daemon-connection
async fn connect_daemon_with_retry(
    addr: std::net::SocketAddr,
    pin: &DaemonPin,
    key_file: &std::path::Path,
) -> DaemonConn {
    let mut backoff = Duration::from_secs(1);
    loop {
        // spec: WEB#daemon-connection — resolve the pinned fingerprint per attempt so a
        // daemon that has not yet published its fingerprint file does not make
        // startup fail; we retry until the file appears.
        match pin.resolve() {
            None => {
                tracing::warn!(
                    "daemon fingerprint not yet available — retrying in {}s",
                    backoff.as_secs()
                );
            }
            Some(auth) => match DaemonConn::connect(addr, auth, key_file).await {
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
            },
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

// spec: WEB#listen-addresses
/// Choose the bind sources for the WebTransport listener: its own
/// interfaces/addresses when either is configured, otherwise the shared HTTP
/// set. Lets a deployment bind WebTransport somewhere an HTTP reverse proxy
/// can't reach (e.g. the tailnet) while HTTP stays on loopback.
/// The port to advertise to clients, in `wt_url` and in the CSP's
/// `connect-src` origin.
///
/// Taken from the addresses actually bound rather than from `--wt-port`:
/// with explicit `--wt-listen` (or `--listen`) addresses that argument is not
/// the port anything listens on, and substituting the default made every
/// documented explicit-address combination advertise an unusable endpoint.
///
/// One port is advertised, so addresses that disagree on it are rejected
/// rather than having one of them silently picked.
// spec: WEB#authentication
fn advertised_wt_port(addrs: &[SocketAddr]) -> Result<u16, String> {
    let mut ports = addrs.iter().map(SocketAddr::port);
    let first = ports
        .next()
        .ok_or_else(|| "no WebTransport listen addresses were resolved".to_owned())?;
    if let Some(other) = ports.find(|p| *p != first) {
        return Err(format!(
            "WebTransport listen addresses disagree on the port ({first} and {other}); \
             a single port is advertised to clients, so they must match"
        ));
    }
    Ok(first)
}

/// How the daemon is verified: by the pinned fingerprint it was given, or,
/// only where asked and only in a debug build, not at all. Given neither, the
/// web interface does not start.
// spec: WEB#daemon-connection
fn choose_daemon_pin(
    file: Option<PathBuf>,
    fingerprint: Option<String>,
    trust_any: bool,
) -> Result<DaemonPin, String> {
    if let Some(path) = file {
        Ok(DaemonPin::File(path))
    } else if trust_any {
        tracing::warn!("--daemon-trust-any: skipping daemon key verification");
        Ok(DaemonPin::Fixed(ClientAuth::TrustAny))
    } else if let Some(fp) = fingerprint {
        Ok(DaemonPin::Fixed(ClientAuth::Fingerprint(fp)))
    } else {
        Err("--daemon-fingerprint or --daemon-fingerprint-file is required".to_owned())
    }
}

/// The port the WebTransport listener's interface-derived addresses take.
///
/// `--wt-port` is refused alongside an address the listener binds explicitly,
/// which carries its own port: it would be a port nothing is bound to.
// spec: WEB#listen-addresses
fn wt_bind_port(explicit_addrs: &[SocketAddr], wt_port: Option<u16>) -> Result<u16, String> {
    match (explicit_addrs.is_empty(), wt_port) {
        (false, Some(_)) => Err(
            "--wt-port cannot be given with an address the WebTransport listener binds \
             explicitly (--wt-listen, or --listen while it binds the shared addresses)"
                .to_owned(),
        ),
        (_, Some(port)) => Ok(port),
        (_, None) => Ok(DEFAULT_WT_PORT),
    }
}

fn wt_bind_sources<'a>(
    interface: &'a [String],
    listen: &'a [SocketAddr],
    wt_interface: &'a [String],
    wt_listen: &'a [SocketAddr],
) -> (&'a [String], &'a [SocketAddr]) {
    if !wt_interface.is_empty() || !wt_listen.is_empty() {
        (wt_interface, wt_listen)
    } else {
        (interface, listen)
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

    if std::env::var_os("SSLKEYLOGFILE").is_some() {
        tracing::warn!("SSLKEYLOGFILE is set — TLS session keys are being logged to disk");
    }

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

    // spec: WEB#listen-addresses — resolve WT bind addresses. WebTransport can't be carried
    // by an HTTP-terminating proxy, so it may need to bind a different set of
    // addresses than HTTP (e.g. HTTP on loopback behind `tailscale serve`, WT
    // directly on the tailnet). Use the WT-specific interfaces/addresses when
    // given, otherwise fall back to the shared HTTP set.
    let (wt_interface, wt_listen) = wt_bind_sources(
        &args.interface,
        &args.listen,
        &args.wt_interface,
        &args.wt_listen,
    );
    // Only interface-derived addresses take this port; explicit addresses
    // carry their own.
    let wt_bind_port = wt_bind_port(wt_listen, args.wt_port).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });
    let wt_addrs = resolve_bind_addrs(wt_interface, wt_listen, wt_bind_port).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });
    let wt_port = advertised_wt_port(&wt_addrs).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });

    // spec: WEB#development-bypass — dev-no-auth must not be used with non-loopback addresses.
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

    // spec: WEB#daemon-connection — a fingerprint file takes precedence and is resolved per
    // attempt; otherwise the pin is fixed at startup.
    let daemon_pin = choose_daemon_pin(
        args.daemon_fingerprint_file,
        args.daemon_fingerprint,
        trust_any,
    )
    .unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });

    let key_file = args.key_file.unwrap_or_else(DaemonConn::default_key_path);

    // spec: WEB#daemon-connection
    let daemon =
        Arc::new(connect_daemon_with_retry(args.daemon_addr, &daemon_pin, &key_file).await);

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

    // spec: WEB#session-tracking
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
        // spec: WEB#listen-addresses
        let endpoint = wt::bind_wt_endpoint(addr, &state).unwrap_or_else(|e| {
            eprintln!("error: WebTransport bind {addr}: {e}");
            std::process::exit(1);
        });
        let state2 = state.clone();
        let rx = rotation_rx.clone();
        tokio::spawn(async move {
            wt::run_wt_server(endpoint, addr, state2, rx).await;
        });
    }

    // Wait indefinitely.
    tokio::signal::ctrl_c().await.expect("ctrl-c handler");
    tracing::info!("shutting down");
}

#[cfg(test)]
mod bind_tests {
    use super::*;

    // spec: WEB#listen-addresses
    #[test]
    fn wt_port_is_refused_with_an_explicit_address_and_defaults_without_one() {
        let explicit: Vec<SocketAddr> = vec!["127.0.0.1:1".parse().unwrap()];
        assert!(wt_bind_port(&explicit, Some(9000)).is_err());
        assert_eq!(wt_bind_port(&explicit, None), Ok(DEFAULT_WT_PORT));
        assert_eq!(wt_bind_port(&[], Some(9000)), Ok(9000));
        assert_eq!(wt_bind_port(&[], None), Ok(DEFAULT_WT_PORT));
    }

    // spec: WEB#listen-addresses
    // WebTransport falls back to the shared HTTP interfaces/addresses only when
    // no WT-specific ones are given; either WT override takes precedence so it
    // can be exposed where an HTTP proxy can't reach.
    #[test]
    fn wt_sources_fall_back_then_override() {
        let iface = vec!["lo".to_owned()];
        let listen: Vec<SocketAddr> = vec!["127.0.0.1:1".parse().unwrap()];
        let wt_iface = vec!["tailscale0".to_owned()];
        let wt_listen: Vec<SocketAddr> = vec!["[::]:2".parse().unwrap()];
        let empty_i: Vec<String> = vec![];
        let empty_a: Vec<SocketAddr> = vec![];

        // No WT-specific config → share the HTTP set.
        let (i, a) = wt_bind_sources(&iface, &listen, &empty_i, &empty_a);
        assert_eq!(i, iface.as_slice());
        assert_eq!(a, listen.as_slice());

        // WT interface given → use it (and the WT address set, here empty).
        let (i, a) = wt_bind_sources(&iface, &listen, &wt_iface, &empty_a);
        assert_eq!(i, wt_iface.as_slice());
        assert!(a.is_empty());

        // WT listen given → use it (and the WT interface set, here empty).
        let (i, a) = wt_bind_sources(&iface, &listen, &empty_i, &wt_listen);
        assert!(i.is_empty());
        assert_eq!(a, wt_listen.as_slice());
    }
}

#[cfg(test)]
mod daemon_pin_tests {
    use super::*;

    // spec: WEB#daemon-connection
    // A fingerprint file is resolved per attempt: absent or empty means "not
    // yet available" (retry), and a real value is trimmed to the exact
    // fingerprint the daemon published.
    #[test]
    fn file_pin_trims_and_resolves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("oi.fingerprint");
        let pin = DaemonPin::File(path.clone());

        // Absent file → not yet available.
        assert!(pin.resolve().is_none());

        // Whitespace-only → still not available.
        std::fs::write(&path, "   \n").expect("write");
        assert!(pin.resolve().is_none());

        // Real fingerprint with trailing newline → trimmed exact match.
        std::fs::write(&path, "deadbeef\n").expect("write");
        match pin.resolve() {
            Some(ClientAuth::Fingerprint(fp)) => assert_eq!(fp, "deadbeef"),
            _ => panic!("expected a Fingerprint auth"),
        }
    }

    // spec: WEB#daemon-connection
    #[test]
    fn no_fingerprint_means_the_web_interface_does_not_start_even_in_a_debug_build() {
        assert!(choose_daemon_pin(None, None, false).is_err());
        assert!(choose_daemon_pin(None, Some("ab".into()), false).is_ok());
    }

    // spec: WEB#daemon-connection
    #[test]
    fn fixed_pin_returns_its_auth() {
        let pin = DaemonPin::Fixed(ClientAuth::TrustAny);
        assert!(matches!(pin.resolve(), Some(ClientAuth::TrustAny)));
    }

    // spec: WEB#authentication
    // With explicit listen addresses the `--wt-port` argument is not the port
    // anything is bound to; advertising the default in its place made every
    // documented explicit-address combination hand clients an unusable URL.
    #[test]
    fn the_advertised_port_comes_from_the_bound_address() {
        let addrs: Vec<SocketAddr> = vec!["10.0.0.5:9999".parse().unwrap()];
        assert_eq!(advertised_wt_port(&addrs), Ok(9999));
    }

    // spec: WEB#authentication
    #[test]
    fn matching_ports_across_several_addresses_are_advertised_once() {
        let addrs: Vec<SocketAddr> = vec![
            "10.0.0.5:9999".parse().unwrap(),
            "[::1]:9999".parse().unwrap(),
        ];
        assert_eq!(advertised_wt_port(&addrs), Ok(9999));
    }

    // spec: WEB#authentication
    // Only one port reaches the client, so picking one of several would leave
    // the rest advertised as something they are not.
    #[test]
    fn addresses_disagreeing_on_the_port_are_rejected() {
        let addrs: Vec<SocketAddr> = vec![
            "10.0.0.5:9999".parse().unwrap(),
            "[::1]:8888".parse().unwrap(),
        ];
        let err = advertised_wt_port(&addrs).expect_err("mixed ports must not resolve");
        assert!(err.contains("9999") && err.contains("8888"), "{err}");
    }

    // spec: WEB#authentication
    #[test]
    fn an_empty_address_set_has_no_port_to_advertise() {
        advertised_wt_port(&[]).expect_err("nothing bound means nothing to advertise");
    }
}
