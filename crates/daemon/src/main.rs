use std::{
    panic::AssertUnwindSafe,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::Parser;
use futures_util::FutureExt;
use lloggs::LoggingArgs;
use parking_lot::RwLock;
use seedling_core::{
    oi::{self, server::DEFAULT_MAX_STREAMS, state::OiState},
    runtime::{
        AppRegistry, InstanceRegistry, Scheduler, audit,
        db::{Db, DbHandle},
        gc::{GcConfig, InstanceGcConfig},
        registry::DbInstanceRegistry,
    },
    system::{
        System,
        nat64::should_activate_nat64,
        netinfo::{detect_ipv4_egress, detect_ipv6_egress},
        node_prefix_from_machine_id,
        reconcile::Reconciler,
        resolver::{resolver_addr, resolver_gateway_addr, spawn_dns_forwarder},
    },
};
use seedling_protocol::names::AppName;
use tokio::sync::Notify;

/// Report a fatal startup error and exit the process.
///
/// Writes the message to stderr **synchronously** in addition to `tracing`:
/// the fatal path calls `std::process::exit`, which does not run the logging
/// guard's `Drop`, so the non-blocking `tracing` writer never flushes and a
/// `tracing`-only message would be lost — leaving operators with the dreaded
/// "exited 1, no output". Takes the same argument syntax as `format!`.
macro_rules! fatal {
    ($($arg:tt)*) => {{
        tracing::error!($($arg)*);
        eprintln!("fatal: {}", format_args!($($arg)*));
        std::process::exit(1);
    }};
}

#[derive(Parser)]
#[command(name = "seedling", version)]
struct Args {
    /// Directory to store persistent state
    #[arg(long, default_value = ".")]
    data_dir: PathBuf,

    #[command(flatten)]
    logging: LoggingArgs,

    #[command(flatten)]
    script_limits: ScriptLimitArgs,

    /// Maximum number of concurrently active bidirectional streams across all
    /// connections. Limits overall OI concurrency.
    #[arg(long, default_value_t = DEFAULT_MAX_STREAMS)]
    max_streams: usize,

    /// Path to the audit log file.
    #[arg(long, default_value = "/var/log/seedling/audit.log")]
    audit_log: PathBuf,

    #[command(flatten)]
    gc: GcArgs,

    /// NAT64 mode: auto (default), enabled, or disabled
    #[arg(long, default_value = "auto")]
    nat64: seedling_core::system::nat64::Nat64Mode,

    /// Run without BTRFS support; use plain directories for named volumes
    #[arg(long)]
    without_btrfs: bool,

    /// Boot with in-memory fakes for the host-system backends (container
    /// runtime, systemd, nftables, Caddy). The OI, DB, scheduler, and event
    /// stream all run normally; only host-system effects are faked. Skips
    /// NAT64 setup, the DNS forwarder, and IPv4/IPv6 egress probes — egress
    /// is reported as `false` for both. Intended for end-to-end UI tests
    /// (Playwright) and for integration tests of individual subsystems.
    #[arg(long)]
    stub_backends: bool,

    // spec: TRN#listen-addresses
    /// Network interface(s) to bind the OI on (comma-separated names).
    /// All IPv4 and IPv6 addresses of each interface are used.
    /// Failure to resolve a named interface is fatal.
    #[arg(long, value_delimiter = ',')]
    interface: Vec<String>,

    /// Explicit OI listen address(es). May be repeated.
    #[arg(long)]
    listen: Vec<std::net::SocketAddr>,

    /// OI listen port, used with --interface. Conflicts with --listen.
    #[arg(long, default_value_t = oi::DEFAULT_PORT, conflicts_with = "listen")]
    port: u16,

    /// Upstream DNS servers for the in-pod CoreDNS resolver
    /// (comma-separated `host:port` / `[host]:port`). When set, CoreDNS
    /// forwards directly to these servers and the built-in forwarder is
    /// skipped. When unset, CoreDNS forwards to seedling's in-process
    /// forwarder, which proxies to systemd-resolved's extra stub at
    /// `127.0.0.54:53` (preserving split DNS, search domains, etc.).
    // spec: DNS#upstreams
    #[arg(long, value_delimiter = ',')]
    dns_upstreams: Vec<std::net::SocketAddr>,
}

#[derive(clap::Args)]
struct GcArgs {
    /// Garbage collection interval in seconds.
    #[arg(long, default_value_t = 3600)]
    gc_interval_secs: u64,

    /// How long to retain completed action log entries, in seconds.
    #[arg(long, default_value_t = 86400)]
    gc_retain_action_log_secs: u64,

    /// How long to retain cleared fault records, in seconds.
    #[arg(long, default_value_t = 604800)]
    gc_retain_cleared_faults_secs: u64,

    /// How long to retain completed autonomous operation records, in seconds.
    #[arg(long, default_value_t = 604800)]
    gc_retain_completed_operations_secs: u64,

    /// How long to retain unscheduled resource instances, in seconds.
    #[arg(long, default_value_t = 600)]
    gc_retain_unscheduled_instances_secs: u64,
}

impl From<GcArgs> for GcConfig {
    fn from(a: GcArgs) -> Self {
        Self {
            interval: Duration::from_secs(a.gc_interval_secs),
            retain_action_log: Duration::from_secs(a.gc_retain_action_log_secs),
            retain_cleared_faults: Duration::from_secs(a.gc_retain_cleared_faults_secs),
            retain_completed_operations: Duration::from_secs(a.gc_retain_completed_operations_secs),
        }
    }
}

impl From<&GcArgs> for InstanceGcConfig {
    fn from(a: &GcArgs) -> Self {
        Self {
            interval: Duration::from_secs(a.gc_interval_secs),
            retain: Duration::from_secs(a.gc_retain_unscheduled_instances_secs),
        }
    }
}

#[derive(clap::Args)]
struct ScriptLimitArgs {
    /// Maximum operations per script evaluation (0 = unlimited)
    #[arg(long, default_value_t = 100_000)]
    script_max_operations: u64,

    /// Maximum function call nesting depth (0 = unlimited)
    #[arg(long, default_value_t = 64)]
    script_max_call_depth: usize,

    /// Maximum expression nesting depth (0 = unlimited)
    #[arg(long, default_value_t = 64)]
    script_max_expr_depth: usize,

    /// Maximum string size in bytes (0 = unlimited)
    #[arg(long, default_value_t = 1_048_576)]
    script_max_string_size: usize,

    /// Maximum array size in elements (0 = unlimited)
    #[arg(long, default_value_t = 10_000)]
    script_max_array_size: usize,

    /// Maximum object map size in entries (0 = unlimited)
    #[arg(long, default_value_t = 10_000)]
    script_max_map_size: usize,
}

impl From<ScriptLimitArgs> for seedling_core::ScriptLimits {
    fn from(a: ScriptLimitArgs) -> Self {
        Self {
            max_operations: a.script_max_operations,
            max_call_levels: a.script_max_call_depth,
            max_expr_depth: a.script_max_expr_depth,
            max_string_size: a.script_max_string_size,
            max_array_size: a.script_max_array_size,
            max_map_size: a.script_max_map_size,
        }
    }
}

#[tokio::main]
async fn main() {
    let mut _guard = lloggs::PreArgs::parse_with_env("SEEDLING_LOG")
        .setup()
        .unwrap_or_else(|e| {
            tracing::warn!("logging setup: {e}");
            None
        });

    let args = Args::parse();

    if _guard.is_none() {
        _guard = args
            .logging
            .setup(|v| match v {
                0 => "seedling=info,warn,netlink_packet_route::link::buffer_tool=off",
                1 => "seedling=debug,warn,netlink_packet_route::link::buffer_tool=off",
                2 => "info",
                3 => "seedling=debug,info",
                4 => "debug",
                5 => "seedling=trace,debug",
                _ => "trace",
            })
            .map(Some)
            .unwrap_or_else(|e| {
                tracing::warn!("logging setup: {e}");
                None
            });
    }

    #[cfg(debug_assertions)]
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let deadlocks = parking_lot::deadlock::check_deadlock();
            if !deadlocks.is_empty() {
                tracing::error!(count = deadlocks.len(), "deadlocks detected");
                for (i, threads) in deadlocks.iter().enumerate() {
                    for t in threads {
                        tracing::error!(
                            deadlock = i,
                            thread_id = ?t.thread_id(),
                            backtrace = ?t.backtrace(),
                            "deadlocked thread"
                        );
                    }
                }
            }
        }
    });

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("ring crypto provider already installed");

    let script_limits: seedling_core::ScriptLimits = args.script_limits.into();

    let data_dir = args.data_dir;

    std::fs::create_dir_all(&data_dir)
        .unwrap_or_else(|e| fatal!("cannot create data directory {}: {e}", data_dir.display()));

    let data_dir = std::fs::canonicalize(&data_dir).unwrap_or_else(|e| {
        fatal!(
            "cannot canonicalize data directory {}: {e}",
            data_dir.display()
        )
    });

    // spec: ACTU#btrfs-check
    let use_btrfs = match seedling_core::system::volume_store::is_btrfs(&data_dir) {
        Ok(true) => {
            tracing::info!("data directory is on BTRFS; using subvolumes for named volumes");
            true
        }
        Ok(false) if args.without_btrfs => {
            tracing::info!(
                "data directory is not on BTRFS; using plain directories (--without-btrfs)"
            );
            false
        }
        Ok(false) => fatal!(
            "data directory {} is not on a BTRFS filesystem; \
             pass --without-btrfs to use plain directories instead",
            data_dir.display()
        ),
        Err(e) => {
            fatal!(
                "cannot determine filesystem type for {}: {e}",
                data_dir.display()
            )
        }
    };

    let db_path = data_dir.join("seedling.db");
    let db = Db::open(&db_path)
        .unwrap_or_else(|e| fatal!("cannot open database {}: {e}", db_path.display()));

    let cipher = {
        let key_path = data_dir.join("seedling.db.key");
        let c = seedling_core::runtime::secrets::Cipher::load_or_create(&key_path).unwrap_or_else(
            |e| {
                fatal!(
                    "cannot load or create secret key {}: {e}",
                    key_path.display()
                )
            },
        );
        std::sync::Arc::new(c)
    };

    // ---------------------------------------------------------------------------
    // System backends
    // ---------------------------------------------------------------------------

    let node_prefix = node_prefix_from_machine_id()
        .unwrap_or_else(|e| fatal!("cannot derive node prefix from machine-id: {e}"));

    let tick_notify = Arc::new(Notify::new());

    // spec: NAT64#mode
    // spec: NAT64#detection
    let nat64_active = if args.stub_backends {
        false
    } else {
        should_activate_nat64(args.nat64).await
    };
    tracing::info!(
        nat64_mode = %args.nat64,
        nat64_active,
        stub_backends = args.stub_backends,
        "NAT64 decision",
    );

    // spec: NAT64#translator
    if !args.stub_backends
        && nat64_active
        && let Err(e) = seedling_core::system::jool::setup_nat64().await
    {
        // spec: NAT64#translator-lifecycle
        fatal!("failed to set up NAT64 translator: {e}");
    }

    // spec: NAT64#ipv6-egress
    // spec: CONST#host-constants
    let (ipv4_egress, ipv6_egress) = if args.stub_backends {
        (false, false)
    } else {
        (detect_ipv4_egress().await, detect_ipv6_egress().await)
    };

    // spec: CONST#host-constants
    seedling_core::sysconst::set(seedling_core::sysconst::SystemFacts {
        ipv4_egress,
        ipv6_egress,
        nat64_active,
        has_snapshots: use_btrfs,
        node_name: whoami::devicename().unwrap_or_default(),
        timezone: seedling_core::sysconst::detect_timezone(),
    });

    // spec: NAT64#dns64
    // Force DNS64 to translate all names only when seedling is itself
    // providing NAT64 and the host cannot route native IPv6. If an external
    // NAT64+DNS64 setup is in play, trust its own policy.
    let force_dns64_translation = nat64_active && !ipv6_egress;
    tracing::info!(
        nat64_active,
        force_dns64_translation,
        "DNS64 translate-all decision"
    );

    let dns_servers: Vec<std::net::Ipv6Addr> = vec![resolver_addr(&node_prefix)];

    // DNS upstream resolution. By default CoreDNS forwards to an
    // in-process proxy on the resolver-bridge gateway which in turn
    // forwards to systemd-resolved's extra stub (127.0.0.54:53), so
    // containers inherit the host's split DNS / MagicDNS / search
    // domains. `--dns-upstreams` overrides this and points CoreDNS
    // straight at the operator-supplied servers.
    let (dns_upstreams, forwarder_handle) = if args.stub_backends {
        // The DNS forwarder binds a socket on the resolver-bridge gateway
        // address, which only exists when the proxy network is real. With
        // stub backends there's no bridge to bind to, so skip the forwarder
        // entirely. The resolver itself isn't started either; this value is
        // only consumed for log output and CoreDNS config that no stub
        // container ever reads.
        (Vec::new(), None)
    } else if args.dns_upstreams.is_empty() {
        let gw = resolver_gateway_addr(&node_prefix);
        let forwarder_listen = std::net::SocketAddr::new(std::net::IpAddr::V6(gw), 53);
        let host_stub: std::net::SocketAddr =
            "127.0.0.54:53".parse().expect("static socket addr parses");
        tracing::info!(
            %forwarder_listen,
            upstream = %host_stub,
            "starting DNS forwarder (no --dns-upstreams given)"
        );
        let handle = spawn_dns_forwarder(forwarder_listen, host_stub);
        (vec![forwarder_listen], Some(handle))
    } else {
        tracing::info!(
            upstreams = ?args.dns_upstreams,
            "using explicit DNS upstreams; skipping in-process forwarder"
        );
        (args.dns_upstreams.clone(), None)
    };
    let _dns_forwarder_handle = forwarder_handle;

    let (driver, caddy_admin_client) = if args.stub_backends {
        tracing::info!("booting with stubbed system backends (no podman/systemd/nftables)");
        System::setup_stubbed(&data_dir, use_btrfs)
            .unwrap_or_else(|e| fatal!("stubbed system setup failed: {e}"))
    } else {
        System::setup(node_prefix, &data_dir, use_btrfs)
            .await
            .unwrap_or_else(|e| fatal!("system setup failed: {e}"))
    };

    // ---------------------------------------------------------------------------
    // App registry — load registered apps from DB
    // ---------------------------------------------------------------------------

    let registry = tokio::task::block_in_place(|| {
        AppRegistry::load_from_db(&db, &cipher, Arc::clone(&tick_notify), &script_limits).inspect(
            |registry| {
                // spec: APPS#persistence — a stored definition that no longer
                // evaluates is faulted, and one that evaluates again clears
                // its old fault.
                for entry in registry.iter() {
                    seedling_core::runtime::apps::sync_script_error_fault(&db, entry);
                }
            },
        )
    })
    .unwrap_or_else(|e| fatal!("failed to load registered apps: {e}"));

    let registry = Arc::new(RwLock::new(registry));
    let db = DbHandle::from_db(db);
    let scheduler = Arc::new(parking_lot::Mutex::new(Scheduler::new()));

    // Podman-level scan for orphaned anonymous volumes.
    // Any volume with the "seedling-anon-" prefix that isn't tracked in the DB
    // is an orphan from a previous run.
    {
        let driver_ref = Arc::clone(&driver);
        let orphan_vols = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                driver_ref
                    .container
                    .list_volumes_by_prefix("seedling-anon-")
                    .await
                    .unwrap_or_default()
            })
        });

        if !orphan_vols.is_empty() {
            tracing::warn!(
                count = orphan_vols.len(),
                "found orphaned seedling-anon- volumes in podman; removing"
            );
            let driver_ref = Arc::clone(&driver);
            for vol_name in &orphan_vols {
                tracing::info!(volume = %vol_name, "removing orphaned anonymous volume");
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let _ = driver_ref.container.remove_volume(vol_name).await;
                    })
                });
            }
        }
    }

    seedling_core::system::leftover_snapshots::sweep(&db, &driver.volume_store).await;

    // spec: ACTU#volume-storage
    // Reset the tmpfs-volume bind-mount root on startup. /run is tmpfs so
    // contents are already gone after a host reboot, but a daemon restart
    // alone leaves the directory tree intact — clear it so each fresh run
    // starts from an empty slate, matching the spec's "tmpfs contents do not
    // survive" guarantee. Skipped under --stub-backends since stub volumes
    // live under {data_dir}/stub-volumes, not /run.
    if !args.stub_backends {
        let tmpfs_root = std::path::Path::new(seedling_core::system::actuator::TMPFS_VOLUMES_DIR);
        if tmpfs_root.exists()
            && let Err(e) = std::fs::remove_dir_all(tmpfs_root)
        {
            tracing::warn!(
                path = %tmpfs_root.display(),
                "failed to clear tmpfs-volumes root on startup: {e}"
            );
        }
        if let Err(e) = std::fs::create_dir_all(tmpfs_root) {
            tracing::warn!(
                path = %tmpfs_root.display(),
                "failed to (re)create tmpfs-volumes root on startup: {e}"
            );
        }
    }

    // Podman-level scan for orphaned shell-session containers.
    //
    // Shell sessions (both regular shells labelled "seedling.session=shell"
    // and volume shells labelled "seedling.session=volume-shell") are tied to
    // a live PTY / QUIC stream in the daemon process. After a daemon restart
    // the PTY fds are dead and the session is unrecoverable, so any such
    // container is an orphan. Runs BEFORE the pod-network scan so the
    // newly-dead shell's network is picked up as an orphan immediately.
    {
        let driver_ref = Arc::clone(&driver);
        let orphan_shells: Vec<seedling_core::system::types::ContainerSummary> =
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    driver_ref
                        .container
                        .list(seedling_core::system::types::ContainerFilter {
                            label_key: Some(seedling_core::system::types::SESSION_LABEL),
                            ..Default::default()
                        })
                        .await
                        .unwrap_or_default()
                })
            });
        if !orphan_shells.is_empty() {
            tracing::warn!(
                count = orphan_shells.len(),
                "found orphaned shell-session containers from a previous run; removing"
            );
            let driver_ref = Arc::clone(&driver);
            for container in &orphan_shells {
                tracing::info!(
                    container = %container.name,
                    session = container.labels.get(seedling_core::system::types::SESSION_LABEL).map(|s| s.as_str()).unwrap_or("?"),
                    "removing orphaned shell container"
                );
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let _ = driver_ref
                            .container
                            .remove_container(&container.name, true)
                            .await;
                    })
                });
            }
        }
    }

    // Containers of apps that are no longer registered, and the pod networks
    // nothing live is using. The containers go first: a network is judged live
    // by the container behind it, and an orphan still exists until removed.
    {
        let known_apps: std::collections::HashSet<AppName> = {
            let reg = registry.read();
            reg.names().into_iter().collect()
        };

        let driver_ref = Arc::clone(&driver);
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(
                seedling_core::system::startup_cleanup::sweep_orphaned_workloads(
                    &*driver_ref.container,
                    &known_apps,
                ),
            )
        });
    }

    // ---------------------------------------------------------------------------
    // Global reconciler
    // ---------------------------------------------------------------------------

    let instance_registry: Arc<dyn InstanceRegistry> = Arc::new(DbInstanceRegistry::new(
        DbHandle::open(&db_path)
            .unwrap_or_else(|e| fatal!("cannot open instance registry db: {e}")),
    ));

    let obs_db =
        DbHandle::open(&db_path).unwrap_or_else(|e| fatal!("cannot open observations db: {e}"));

    let event_tx = seedling_protocol::events::new_event_channel();
    seedling_core::runtime::faults::init(event_tx.clone());

    // Audit log — subscribe before anything emits events.
    let _audit_handle = audit::spawn_audit_task(args.audit_log, event_tx.subscribe(), db.clone());

    // spec: HIST#autonomous-operations
    // Before the reconciler starts, so no record it makes can be caught. Fatal
    // like the other startup reads: once the reconciler is recording, the
    // previous run's records can no longer be told apart from its own.
    match db.call(seedling_core::runtime::history::interrupt_incomplete_autonomous_operations) {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            count = n,
            "marked autonomous operations from the previous run as interrupted"
        ),
        Err(e) => fatal!("cannot mark interrupted autonomous operations: {e}"),
    }

    // Periodic garbage collection of operational tables. The reconciler runs
    // the unscheduled-instance sweep, as only it knows the desired state.
    let instance_gc = InstanceGcConfig::from(&args.gc);
    let _gc_handle = seedling_core::runtime::gc::spawn_gc_task(db.clone(), args.gc.into());

    // spec: SITE#tailscale-discovery
    // Spawn the Tailscale discovery provider. It polls the host's
    // tailscaled local API on its own cadence and reconciles a single
    // discovered site ingress against the runtime DB. We always
    // construct the provider and let it skip cycles when the socket is
    // unreachable — operators without Tailscale see no faults; operators
    // with Tailscale get a discovered ingress as soon as the daemon can
    // reach tailscaled.
    let tailscale_provider = seedling_core::runtime::tailscale::TailscaleProvider::new(
        db.clone(),
        seedling_core::runtime::tailscale::TailscaleConfig::default(),
    );
    let _tailscale_handle = Arc::clone(&tailscale_provider).spawn();

    let shells = seedling_core::oi::shells::ShellRegistry::new();

    // spec: TLS#issuance
    // The TLS issuance coordinator owns every ACME-DNS flow: the
    // reconciler hands it the set of TLS-terminating hostnames each
    // tick, and the OI / CLI route operator-driven retries through it
    // too. The cert-serve endpoint stays a pure lookup, and asks it only
    // whether the proxy may go on to order a certificate of its own.
    let tls_coordinator =
        seedling_core::runtime::tls::issuance::Coordinator::new(db.clone(), Arc::clone(&cipher));

    // spec: TLS#serving
    // Bind the cert-serving HTTP endpoint on the seedling-proxy bridge
    // gateway so only Caddy + host processes can reach it. Token-in-path
    // protects against host-local enumeration; the URL we hand to Caddy
    // includes both. See runtime::tls::serve.
    // The cert serve endpoint exists to feed live certs to Caddy via the
    // proxy-bridge IPv6 gateway. Under --stub-backends Caddy isn't running,
    // and the fixed bind address would collide with any concurrent real
    // daemon on the same host, so skip the listener entirely.
    let (cert_endpoint_url, _cert_serve_handle) = if args.stub_backends {
        (None, tokio::spawn(async {}))
    } else {
        let token_path = data_dir.join("tls-cert-endpoint.token");
        let token = seedling_core::runtime::tls::serve::load_or_create_token(&token_path)
            .unwrap_or_else(|e| fatal!("cannot load/create cert endpoint token: {e}"));

        let gateway = seedling_core::system::proxy_bridge_gateway(&node_prefix);
        let bind_addr = std::net::SocketAddr::new(std::net::IpAddr::V6(gateway), 7892);

        let cipher_for_serve = Arc::clone(&cipher);
        let db_for_serve = db.clone();
        let token_for_serve = token.clone();
        let (resolved_addr, handle) = match seedling_core::runtime::tls::serve::spawn_server(
            db_for_serve,
            cipher_for_serve,
            Arc::clone(&tls_coordinator),
            token_for_serve,
            bind_addr,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(
                    bind = %bind_addr,
                    "cannot bind cert serve endpoint: {e}; runtime-managed certs will not be served"
                );
                (bind_addr, tokio::spawn(async {}))
            }
        };
        let url = seedling_core::runtime::tls::serve::caddy_url(resolved_addr, &token);
        tracing::info!(url = %url, "tls cert serve endpoint listening");
        (Some(url), handle)
    };

    // spec: TLS#renewal
    // Spawn the ACME-DNS renewal task. Each tick (default hourly) it
    // asks the issuance coordinator to ensure every hostname whose
    // unified state says "issue now" — same decision function the
    // reconciler and the OI use.
    {
        let db_for_renewal = db.clone();
        let coord_for_renewal = Arc::clone(&tls_coordinator);
        let _renewal_handle = seedling_core::runtime::tls::renewal::spawn(
            db_for_renewal,
            coord_for_renewal,
            seedling_core::runtime::tls::renewal::DEFAULT_TICK,
        );
        // spec: TLS#renewal
        let _ari_handle = seedling_core::runtime::tls::renewal::spawn_ari_refresh(
            db.clone(),
            Arc::clone(&cipher),
        );
    }

    // spec: TLS#serving
    // Keep the certificate SAN index in line with the certificates it indexes:
    // it narrows every certificate lookup, so a certificate missing from it is
    // one no lookup finds.
    let _san_index_handle = seedling_core::runtime::tls::san_index::spawn(db.clone());

    // spec: SITE#endpoint-addressing
    // Spawn the site-service DNS resolver alongside the reconciler so
    // operator-supplied DNS-named endpoints get periodically re-resolved
    // and the reconciler is kicked when their addresses change.
    let (site_resolver, _site_resolver_handle) =
        seedling_core::runtime::site_services::resolver::SiteServiceResolver::spawn(
            db.clone(),
            Arc::clone(&tick_notify),
        );

    let reconciler = Reconciler::new(
        Arc::clone(&driver),
        node_prefix,
        instance_registry,
        Arc::clone(&caddy_admin_client),
        data_dir.clone(),
        obs_db,
        Arc::clone(&registry),
        event_tx.clone(),
        dns_servers.clone(),
        dns_upstreams,
        nat64_active,
        force_dns64_translation,
        Arc::clone(&shells),
        cert_endpoint_url,
        Some(Arc::clone(&tls_coordinator)),
        Some(Arc::clone(&site_resolver)),
        instance_gc,
    );

    // The reconciler and schedule ticker are spawned below, after OiState is
    // constructed, so that scheduled-action fires can spawn lifecycle operations.
    let reconciler_handle = {
        let tick_notify = Arc::clone(&tick_notify);
        let schedule_db = db.clone();
        let schedule_scheduler = Arc::clone(&scheduler);
        let schedule_registry = Arc::clone(&registry);
        (
            reconciler,
            tick_notify,
            schedule_db,
            schedule_scheduler,
            schedule_registry,
        )
    };

    tracing::info!("started global reconciler");

    // ---------------------------------------------------------------------------
    // OI server
    // ---------------------------------------------------------------------------

    // spec: CNR#canopy-access — the database is the source of truth
    // across restarts; the in-memory mirror is what the relay's hot path reads.
    let canopy_state = Arc::new(seedling_core::oi::canopy::CanopyState::new());
    match db.call(seedling_core::runtime::canopy::get_settings) {
        Ok(settings) => canopy_state.set_enabled(settings.enabled),
        Err(e) => {
            // Default to enabled rather than off: an unreadable setting must not
            // silently stop a host reporting, which would look like the host
            // being down rather than like a local fault.
            tracing::warn!("cannot read the Canopy settings, assuming enabled: {e}");
        }
    }

    let oi_state = Arc::new(OiState {
        registry: Arc::clone(&registry),
        spki_fingerprint: std::sync::OnceLock::new(),
        start_time: Instant::now(),
        started_at: jiff::Timestamp::now(),
        db: db.clone(),
        scheduler: Arc::clone(&scheduler),
        tick_notify: Arc::clone(&tick_notify),
        db_path: db_path.clone(),
        trusted_keys: seedling_core::oi::auth::new_trusted_keys(),
        shells,
        forwards: seedling_core::oi::forwards::ForwardRegistry::new(),
        canopy: canopy_state,
        container_runtime: Arc::clone(&driver.container),
        driver: Arc::clone(&driver),
        node_prefix,
        event_tx: event_tx.clone(),
        script_limits,
        dns_servers,
        cipher,
        tls_coordinator: Arc::clone(&tls_coordinator),
        caddy_data_path: tokio::sync::OnceCell::new(),
        tailscale_provider: Some(Arc::clone(&tailscale_provider)),
        site_resolver: Some(Arc::clone(&site_resolver)),
        definition_registry: Arc::new(seedling_core::runtime::definition::fetch::OciRegistry::new()),
    });
    // The coordinator and the cert endpoint were built before the state they
    // read the facts of a strategy chain from, and have decided nothing until
    // now.
    tls_coordinator.set_environment(Arc::new(seedling_core::oi::state::OiEnvironment::new(
        &oi_state,
    )));

    // ---------------------------------------------------------------------------
    // Decide whether to replay an interrupted lifecycle operation, restore the
    // queue, and start each installed app. Runs after OiState is built but
    // before the reconciler starts ticking, so the install closure's
    // rt.start() calls are picked up by the first reconciliation tick. If the
    // persisted state cannot be read, nothing starts until a later iteration
    // reads it.
    // spec: OPS#initiating-events, SUSP#replay, OPS#unreadable-operation-state
    // ---------------------------------------------------------------------------

    resume_operations(&oi_state).await;

    // Pre-build the volume-shell image so it is warm before the first
    // operator opens a volume shell session. Building takes longer than a
    // pull; doing it eagerly at startup is preferable to making the first
    // operator wait when they're already trying to do something. Skipped
    // under --stub-backends since the build shells out to podman directly,
    // bypassing the ContainerRuntime trait.
    if !args.stub_backends {
        let cr = Arc::clone(&driver.container);
        let build_dir = data_dir.to_path_buf();
        tokio::spawn(async move {
            let image = seedling_core::oi::shells::VOLUME_SHELL_IMAGE;
            match cr.image_exists(image).await {
                Ok(false) => {
                    tracing::info!(%image, "pre-building volume shell image");
                    if let Err(e) =
                        seedling_core::oi::shells::build_volume_shell_image(&build_dir).await
                    {
                        tracing::warn!(%image, "volume shell image build failed: {e}");
                    }
                }
                Ok(true) => {}
                Err(e) => tracing::warn!(%image, "image_exists check failed: {e}"),
            }
        });
    }

    // spec: FLT#app-faults — the condition changes only when
    // a definition is replaced or the runtime starts as another version, so
    // it is converged for every app here and on each replacement.
    {
        let running = seedling_core::runtime::definition::version::running();
        let definitions: Vec<_> = registry
            .read()
            .iter()
            .map(|e| {
                // spec: CNR#applications — only a definition that evaluated
                // says what its type is; one that did not says nothing.
                let canopy_type = e
                    .script_error
                    .is_none()
                    .then(|| e.app.def.load().canopy_type.clone());
                (e.name.clone(), Arc::clone(&e.bundle), canopy_type)
            })
            .collect();
        db.call(move |db| {
            for (app, bundle, canopy_type) in definitions {
                seedling_core::runtime::definition::faults::sync_unsupported(
                    db, &app, &bundle, &running,
                );
                if let Some(canopy_type) = canopy_type
                    && let Err(e) = seedling_core::runtime::canopy::absent::sync_app_type(
                        db,
                        &app,
                        canopy_type.as_deref(),
                    )
                {
                    tracing::error!(%app, "cannot record the app's Canopy type: {e}");
                }
            }
        });
    }

    // spec: RCK#resolution
    let _definition_recheck_handle = seedling_core::runtime::definition::recheck::spawn(
        Arc::clone(&registry),
        db.clone(),
        Arc::clone(&oi_state.definition_registry),
    );

    // spec: CNR#report-schedule — reports go quiet on their own when no
    // client is offering to carry them, so this runs unconditionally.
    let _canopy_report_handle = seedling_core::runtime::canopy::spawn(Arc::clone(&oi_state));

    // Spawn the reconciler + schedule ticker now that OiState is available.
    {
        let (mut reconciler, tick_notify, schedule_db, schedule_scheduler, schedule_registry) =
            reconciler_handle;
        let oi_state_for_sched = Arc::clone(&oi_state);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut idle = false;
            // spec: SCHED#firing
            let mut schedule_ticker = seedling_core::runtime::schedules::ScheduleTicker::new();
            loop {
                // spec: OPS#unreadable-operation-state
                // Until the persisted operation state can be read nothing
                // starts, so each iteration tries again, and the loop keeps
                // iterating rather than idling.
                if oi_state_for_sched.scheduler.lock().is_state_unreadable() {
                    resume_operations(&oi_state_for_sched).await;
                    if oi_state_for_sched.scheduler.lock().is_state_unreadable() {
                        idle = false;
                    }
                }
                if idle {
                    tick_notify.notified().await;
                    idle = false;
                    interval.reset();
                } else {
                    tokio::select! {
                        _ = interval.tick() => {},
                        _ = tick_notify.notified() => {},
                    }
                }

                // spec: SCHED#firing
                // Snapshot generations before acquiring the DB to maintain
                // consistent lock order (registry → db) across the codebase.
                if let Some(now) = schedule_ticker.maybe_tick() {
                    // spec: SCHED#firing
                    // Only Installed apps' schedules may fire. Apps still
                    // installing, uninstalling, or never installed are
                    // excluded — otherwise a permanently-failing install
                    // whose schedule keeps re-acquiring the scheduler slot
                    // would block operator updates.
                    let app_generations: std::collections::HashMap<
                        AppName,
                        seedling_core::runtime::schedules::ScheduledApp,
                    > = {
                        let reg = schedule_registry.read();
                        reg.names()
                            .into_iter()
                            .filter_map(|name| {
                                let entry = reg.get(name.as_str())?;
                                let phase = entry.phase.lock().clone();
                                if matches!(phase, seedling_core::runtime::AppPhase::Installed) {
                                    // The handle travels on so the scheduler
                                    // can read the phase again under its own
                                    // lock: this reading is already stale by
                                    // the time a firing is admitted.
                                    let schemas: std::collections::BTreeMap<_, _> = entry
                                        .app
                                        .def
                                        .load()
                                        .actions
                                        .iter()
                                        .map(|(n, a)| (n.clone(), a.params.clone()))
                                        .collect();
                                    Some((
                                        name,
                                        (
                                            entry.current_generation,
                                            Arc::clone(&entry.phase),
                                            Arc::new(schemas),
                                        ),
                                    ))
                                } else {
                                    None
                                }
                            })
                            .collect()
                    };
                    let sched_arc = Arc::clone(&schedule_scheduler);
                    let accepted_fires = schedule_db.call(move |db| {
                        let mut sched = sched_arc.lock();
                        let fired = seedling_core::runtime::schedules::check_due_schedules(
                            db,
                            &mut sched,
                            now,
                            &|app_name| app_generations.get(app_name).cloned(),
                        );
                        drop(sched);
                        fired
                            .into_iter()
                            .filter(|f| f.operation_id.is_some())
                            .collect::<Vec<_>>()
                    });
                    for fire in accepted_fires {
                        if !fire.accepted {
                            // spec: OPS#concurrency
                            // A fire queued behind another app's operation is
                            // persisted like any queued request.
                            seedling_core::oi::handler::actions::lifecycle::persist_queued_fire(
                                &oi_state_for_sched,
                                &fire,
                                now,
                            );
                            continue;
                        }
                        if let Some(op_id) = fire.operation_id {
                            seedling_core::oi::handler::actions::lifecycle::spawn_accepted_operation(
                                Arc::clone(&oi_state_for_sched),
                                fire.app,
                                fire.action,
                                op_id,
                                serde_json::Map::new(),
                                fire.generation,
                                fire.generation,
                                "schedule".to_owned(),
                                None,
                            );
                        }
                    }
                }

                match AssertUnwindSafe(reconciler.tick()).catch_unwind().await {
                    Ok(active) => {
                        if !active {
                            idle = true;
                        }
                    }
                    Err(payload) => {
                        let msg = match payload.downcast_ref::<&str>() {
                            Some(s) => (*s).to_owned(),
                            None => match payload.downcast_ref::<String>() {
                                Some(s) => s.clone(),
                                None => "unknown panic".to_owned(),
                            },
                        };
                        tracing::error!(panic = %msg, "reconciler tick panicked; recovering");
                        reconciler.file_system_fault(
                            "reconciler_panic",
                            &format!("reconciler tick panicked: {msg}"),
                        );
                    }
                }
            }
        });
    }

    // spec: TRN#listen-addresses
    let oi_addrs = resolve_oi_addrs(&args.interface, &args.listen, args.port);
    let (_fingerprint, oi_endpoints) = oi::run(
        Arc::clone(&oi_state),
        &oi_addrs,
        &data_dir,
        args.max_streams,
    )
    .await
    .unwrap_or_else(|e| fatal!("OI server failed to start: {e}"));

    tracing::info!("seedling ready");

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = sigterm.recv() => {}
    }

    tracing::info!("shutting down");
    for ep in &oi_endpoints {
        ep.close(quinn::VarInt::from_u32(0), b"shutdown");
    }
    for ep in oi_endpoints {
        ep.wait_idle().await;
    }
}

// spec: OPS#initiating-events, SUSP#replay
// spec: ACI#install-parameters
/// Resume an interrupted lifecycle operation. Called after OiState is built
/// and the cipher is available, before the reconciler starts ticking.
///
/// For install operations whose params were persisted encrypted, decrypt and
/// spawn_accepted_operation with the restored params. Non-install operations
/// have no params to restore — they re-run with an empty params map, relying
/// on the action-log replay to reproduce their rt.start/stop sequence
/// deterministically.
///
/// If the operation cannot be replayed (missing ciphertext for an install,
/// app no longer registered, decrypt failure), the function clears the
/// current_operation row, reverts any Installing phase to NotInstalled, and
/// files an install_interrupted fault so the operator can see why.
/// Tear `records` down in the world and forget them.
///
/// Stops and resets the unit, removes the pod network, force-removes the
/// container that may have outlived it, clears the faults a previous run filed
/// against the instance, and deletes the row.
///
/// Only records actually torn down are deleted. One that could not be read is
/// never in this list, and one whose teardown failed is left out of it, so
/// both stay on the books rather than being written off as a teardown that did
/// not happen. The next start finds them again.
// spec: SUSP#abandoned-replay
// spec: FLT#workload-faults
fn tear_down_dynamic_resources(
    driver: &Arc<seedling_core::system::System>,
    db: &DbHandle,
    records: &[seedling_core::runtime::desired::DynamicResourceRecord],
) {
    let mut handled: Vec<(AppName, String)> = Vec::new();
    let mut retained = 0usize;
    for record in records {
        tracing::info!(
            instance = %record.display_name,
            kind = %record.kind,
            operation_id = %record.operation_id,
            "tearing down dynamic resource"
        );
        // The reconciler will not do this for us: these instances are absent
        // from the desired state, so it ignores them rather than stopping
        // them. A unit still loaded is reset so it does not linger.
        let unit = format!("seedling-{}.service", record.display_name);
        let net_name = format!("seedling-{}", record.display_name);
        let container_name = record.display_name.clone();
        let driver_ref = Arc::clone(driver);
        // spec: SUSP#abandoned-replay
        // Whether the world actually gave the resource up. A teardown that
        // failed, or one whose outcome could not be established, is not a
        // teardown: deleting the row on either would leave a live container
        // with no record, no fault and no reconciler covering it, since the
        // reconciler ignores instances absent from the desired state and the
        // next start's orphan scan has nothing left to find it by.
        let torn_down = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut torn_down = true;
                match driver_ref.process.unit_state(&unit).await {
                    Ok(Some(_)) => {
                        if let Err(e) = driver_ref.process.stop_unit(&unit).await {
                            tracing::warn!(unit = %unit, "could not stop the unit: {e}");
                            torn_down = false;
                        } else if let Err(e) = driver_ref.process.reset_failed_unit(&unit).await {
                            tracing::warn!(unit = %unit, "could not reset the unit: {e}");
                            torn_down = false;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(
                            unit = %unit,
                            "could not tell whether the unit is still loaded: {e}"
                        );
                        torn_down = false;
                    }
                }
                match driver_ref.container.network_exists(&net_name).await {
                    Ok(true) => {
                        if let Err(e) = driver_ref.container.remove_network(&net_name).await {
                            tracing::warn!(network = %net_name, "could not remove the network: {e}");
                            torn_down = false;
                        }
                    }
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(
                            network = %net_name,
                            "could not tell whether the network is still there: {e}"
                        );
                        torn_down = false;
                    }
                }
                // Asked for first, rather than removing and discarding the
                // error: the runtime reports a container that is not there as
                // an error, and a dynamic resource's container has routinely
                // already gone, removed on exit. Discarding it also discards
                // the failure to remove one that is still there, against a
                // runtime answering perfectly well.
                match driver_ref.container.inspect(&container_name).await {
                    Ok(None) => {}
                    Ok(Some(_)) => {
                        if let Err(e) =
                            driver_ref.container.remove_container(&container_name, true).await
                        {
                            tracing::warn!(
                                container = %container_name,
                                "could not remove the container: {e}"
                            );
                            torn_down = false;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            container = %container_name,
                            "could not tell whether the container is still there: {e}"
                        );
                        torn_down = false;
                    }
                }
                torn_down
            })
        });
        if torn_down {
            handled.push((record.app.clone(), record.instance_id.clone()));
        } else {
            tracing::warn!(
                instance = %record.display_name,
                operation_id = %record.operation_id,
                "dynamic resource not torn down; keeping its record so the next start finds it"
            );
            retained += 1;
        }
    }

    if retained > 0 {
        tracing::warn!(
            retained,
            "some dynamic resources could not be torn down and stay on the books"
        );
    }
    db.call(move |db| {
        for (app, instance_id) in &handled {
            if let Err(e) =
                seedling_core::runtime::faults::clear_faults_for_instance(db, app, instance_id)
            {
                tracing::warn!(
                    app = %app,
                    instance_id = %instance_id,
                    "failed to clear per-instance faults while tearing down dynamic resources: {e}"
                );
            }
            if let Err(e) = db.conn.execute(
                "DELETE FROM dynamic_resources WHERE instance_id = ?1",
                [instance_id],
            ) {
                tracing::warn!(
                    instance_id = %instance_id,
                    "failed to delete dynamic resource record: {e}"
                );
            }
        }
    });
}

/// Give back the dynamic resources startup spared for a replay that did not
/// start.
///
/// Startup cleanup skips the rows carrying this `operation_id` on the promise
/// that the replay adopts them. Nothing else ever will: the rows name an
/// operation no live operation claims, and the next start reads the same
/// persisted row and spares them again on the same reasoning.
// spec: SUSP#abandoned-replay
fn abandon_spared_resources(
    driver: &Arc<seedling_core::system::System>,
    db: &DbHandle,
    operation_id: &str,
) {
    let wanted = operation_id.to_owned();
    let records = db.call(move |db| {
        seedling_core::runtime::desired::list_dynamic_resources_for_operation(db, &wanted)
    });
    match records {
        Ok(records) if !records.is_empty() => {
            tracing::warn!(
                operation_id = %operation_id,
                count = records.len(),
                "replay did not start; tearing down the resources startup spared for it"
            );
            tear_down_dynamic_resources(driver, db, &records);
        }
        Ok(_) => {}
        Err(e) => {
            // The resources stay recorded, so the next start spares them
            // again and arrives back here. Say so rather than leaving a
            // silent leak.
            tracing::error!(
                operation_id = %operation_id,
                "could not list the resources spared for an abandoned replay; \
                 they remain recorded and will not be reclaimed: {e}"
            );
        }
    }
}

/// Read the persisted operation state and act on it: spare or tear down
/// dynamic resources, replay an interrupted operation, recover stuck installs,
/// restore the queue and run each installed app's start action.
///
/// If the state cannot be read, files `operation_state_unreadable`, starts no
/// operation, and does none of the above, since each acts on an operation's
/// absence. Called again on each reconciliation iteration until it reads.
// spec: OPS#unreadable-operation-state
// spec: OPS#interrupted-operations
// spec: OPS#initiating-events
async fn resume_operations(oi_state: &Arc<OiState>) {
    use seedling_core::runtime::history::read_operation_state;
    use seedling_protocol::names::AppName;

    let system = AppName::new_unchecked("_system");
    let cipher = Arc::clone(&oi_state.cipher);
    let read = oi_state
        .db
        .call(move |db| read_operation_state(db, &cipher));
    let state = match read {
        Ok(state) => state,
        Err(why) => {
            tracing::error!(
                "cannot read the persisted operation state; starting no operation: {why}"
            );
            oi_state.scheduler.lock().set_state_unreadable(true);
            let description = format!("cannot read {why}");
            oi_state.db.call(move |db| {
                let key = seedling_core::runtime::faults::FaultKey::new(
                    &AppName::new_unchecked("_system"),
                    "operation_state_unreadable",
                    "",
                );
                let meta = seedling_core::runtime::faults::FaultMeta::default();
                if let Err(e) =
                    seedling_core::runtime::faults::file_once(db, &key, &meta, &description)
                {
                    tracing::warn!("failed to file operation_state_unreadable: {e}");
                }
            });
            return;
        }
    };
    if oi_state.scheduler.lock().is_state_unreadable() {
        oi_state.scheduler.lock().set_state_unreadable(false);
    }
    {
        let system = system.clone();
        oi_state.db.call(move |db| {
            let _ = seedling_core::runtime::faults::clear_faults_by_kind(
                db,
                &system,
                "operation_state_unreadable",
            );
        });
    }

    let persisted_operation = state.current;
    if let Some(op) = &persisted_operation {
        tracing::info!(
            app = %op.app,
            action = %op.action_name,
            operation_id = %op.operation_id.0,
            "found interrupted operation; will replay after startup"
        );
    }

    // Startup orphan cleanup — remove dynamic resources left by a previous run.
    // Resources owned by the operation we are about to replay are kept so the
    // replay picks up where it left off.
    let replay_op_id: Option<String> = persisted_operation
        .as_ref()
        .map(|op| op.operation_id.0.clone());
    {
        let records = oi_state
            .db
            .call(seedling_core::runtime::desired::list_dynamic_resources);
        let records: Result<Vec<_>, _> = records.map(|rs| {
            rs.into_iter()
                .filter(|r| Some(&r.operation_id) != replay_op_id.as_ref())
                .collect()
        });
        match records {
            Ok(records) if !records.is_empty() => {
                tracing::warn!(
                    count = records.len(),
                    "found orphaned dynamic resources from a previous run; cleaning up"
                );
                tear_down_dynamic_resources(&oi_state.driver, &oi_state.db, &records);
                tracing::info!("orphaned dynamic resource cleanup complete");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("failed to check for orphaned dynamic resources: {e}");
            }
        }
    }

    let replayed_app = match persisted_operation {
        Some(op) => {
            let app = op.app.clone();
            replay_interrupted_operation(Arc::clone(oi_state), op).then_some(app)
        }
        None => None,
    };

    let db = &oi_state.db;
    // spec: OPS#stuck-install-recovery
    // Recover apps left in inconsistent state by a previous run:
    //
    // 1. `Installing` phase with no matching current_operation. Causes: a
    //    previous run cancelled an op and cleared current_operation but its
    //    phase-revert persist was interrupted by crash/restart; or the daemon
    //    died between persisting the phase and persisting current_operation.
    //    Without this sweep, the operator can neither cancel nor uninstall
    //    such an app — both gates check phase.
    //
    // 2. `NotInstalled` phase but resource_instances still exist for the
    //    app. Causes: a previous recovery dropped Installing → NotInstalled
    //    directly (older code path), or DB-level surgery. The reconciler
    //    skips NotInstalled apps so the orphaned resources never get torn
    //    down through normal reconciliation.
    //
    // Both stuck apps transition to Uninstalling so the reconciler runs the
    // teardown loop; it converges to NotInstalled once teardown completes.
    {
        use seedling_core::runtime::apps::AppPhase;
        let stuck: Vec<seedling_protocol::names::AppName> = {
            let reg = oi_state.registry.read();
            reg.iter()
                .filter(|e| {
                    if replayed_app.as_ref() == Some(&e.name) {
                        return false;
                    }
                    let phase = e.phase.lock();
                    if matches!(*phase, AppPhase::Installing) {
                        return true;
                    }
                    if matches!(*phase, AppPhase::NotInstalled) {
                        let app_for_query = e.name.clone();
                        let count: i64 = db.call(move |db| {
                            seedling_core::runtime::history::app_resource_instance_count(
                                db,
                                &app_for_query,
                            )
                            .unwrap_or(0)
                        });
                        return count > 0;
                    }
                    false
                })
                .map(|e| e.name.clone())
                .collect()
        };
        for app_name in stuck {
            tracing::warn!(
                app = %app_name,
                "app in stuck state at startup; transitioning to Uninstalling for teardown"
            );
            seedling_core::oi::handler::actions::lifecycle::set_phase_and_persist(
                oi_state,
                &app_name,
                AppPhase::Uninstalling,
            );
        }
    }

    // spec: OPS#concurrency
    seedling_core::oi::handler::actions::lifecycle::restore_queue(oi_state, state.queued);

    // spec: OPS#initiating-events
    // spec: ACN#start-action
    // Each installed app with no operation being replayed or waiting has its
    // start action run, with the trigger `boot`; until it ends the app's
    // desired state is held, so the start action brings it up in its own
    // order.
    {
        use seedling_core::runtime::apps::AppPhase;
        let installed: Vec<AppName> = {
            let reg = oi_state.registry.read();
            reg.iter()
                .filter(|e| matches!(*e.phase.lock(), AppPhase::Installed))
                .filter(|e| replayed_app.as_ref() != Some(&e.name))
                .filter(|e| oi_state.scheduler.lock().blocker_for(&e.name).is_none())
                .map(|e| e.name.clone())
                .collect()
        };
        for app_name in installed {
            seedling_core::oi::handler::actions::lifecycle::initiate_start(
                oi_state, &app_name, "boot",
            );
        }
    }
}

// spec: SUSP#abandoned-replay
/// Replay an interrupted lifecycle operation, and where it does not start,
/// hand back the resources startup spared on its behalf.
///
/// The decision of whether to replay is [`start_replay`]'s alone; this is the
/// one place that acts on its answer, so no abort path can reach the world
/// without settling what it owes.
fn replay_interrupted_operation(
    state: Arc<OiState>,
    op: seedling_core::runtime::history::CurrentOperation,
) -> bool {
    let operation_id = op.operation_id.0.clone();
    let replayed = start_replay(Arc::clone(&state), op);
    if !replayed {
        abandon_spared_resources(&state.driver, &state.db, &operation_id);
    }
    replayed
}

/// Decide whether the interrupted operation can be replayed, and start it if
/// so. Returns whether the replay was handed to the lifecycle runtime.
fn start_replay(
    state: Arc<OiState>,
    op: seedling_core::runtime::history::CurrentOperation,
) -> bool {
    use seedling_core::runtime::{
        apps::AppPhase,
        history::{clear_current_operation, load_current_operation_params},
        scheduler::ScheduleResult,
    };

    let app_name = op.app.clone();
    let action_name = op.action_name.clone();
    let is_install = action_name == "install";

    // Check that the app is still registered. If the previous run
    // deregistered mid-operation (shouldn't happen, but defensive), drop
    // the row and move on.
    let phase_opt = {
        let reg = state.registry.read();
        reg.get(app_name.as_str()).map(|e| e.phase.lock().clone())
    };
    let Some(phase) = phase_opt else {
        tracing::warn!(
            app = %app_name,
            operation_id = %op.operation_id.0,
            "interrupted operation references an unregistered app; discarding"
        );
        state.db.call(|db| {
            let _ = clear_current_operation(db);
        });
        return false;
    };

    // spec: OPS#persistence
    // Restore the encrypted params that were persisted when the operation
    // began. Every persisted lifecycle operation has params (empty map for
    // actions that take none), so a None here is an install-specific
    // failure mode that pre-dates the generalised persistence.
    let params = {
        let cipher = Arc::clone(&state.cipher);
        let loaded = state
            .db
            .call(move |db| load_current_operation_params(db, &cipher));
        match loaded {
            Ok(Some(params)) => params,
            Ok(None) => {
                if is_install {
                    tracing::error!(
                        app = %app_name,
                        "interrupted install has no persisted params; reverting to NotInstalled"
                    );
                    seedling_core::oi::handler::actions::lifecycle::recover_unreplayable_install(
                        &state, &app_name,
                    );
                    return false;
                }
                tracing::warn!(
                    app = %app_name,
                    action = %action_name,
                    "interrupted operation has no persisted params (legacy row); replaying with empty map"
                );
                serde_json::Map::new()
            }
            Err(e) => {
                tracing::error!(
                    app = %app_name,
                    action = %action_name,
                    "failed to decrypt interrupted operation params: {e}"
                );
                if is_install {
                    seedling_core::oi::handler::actions::lifecycle::recover_unreplayable_install(
                        &state, &app_name,
                    );
                } else {
                    state.db.call(|db| {
                        let _ = clear_current_operation(db);
                    });
                }
                return false;
            }
        }
    };

    // Sanity-check phase vs action. An install must find its app in
    // Installing (or NotInstalled if the phase persistence raced with a
    // crash). A non-install must not find its app in Installing.
    if is_install && !matches!(phase, AppPhase::Installing | AppPhase::NotInstalled) {
        tracing::warn!(
            app = %app_name,
            ?phase,
            "interrupted install but phase is not Installing; clearing row"
        );
        state.db.call(|db| {
            let _ = clear_current_operation(db);
        });
        return false;
    }
    // spec: SCHED#firing
    // A non-install operation only makes sense on an Installed app. If a
    // crash interrupted a scheduled action that fired in a window before the
    // installed-phase gate landed, the persisted row would otherwise replay
    // forever, monopolising the scheduler slot and blocking script updates.
    if !is_install && !matches!(phase, AppPhase::Installed) {
        tracing::warn!(
            app = %app_name,
            ?phase,
            action = %action_name,
            "interrupted non-install operation but app is not Installed; clearing row"
        );
        state.db.call(|db| {
            let _ = clear_current_operation(db);
        });
        return false;
    }

    // Register with the scheduler so concurrency gates report "in progress"
    // for the duration of the replay.
    let params_for_sched = params.clone();
    let sched_result = state.scheduler.lock().request_with_id(
        &app_name,
        &action_name,
        params_for_sched,
        op.source_generation,
        op.target_generation,
        "replay",
        op.operation_id.clone(),
    );
    if !matches!(sched_result, ScheduleResult::Accepted) {
        tracing::error!(
            app = %app_name,
            "scheduler refused to accept replay slot: {sched_result:?}"
        );
        return false;
    }

    // spec: OPS#cancel-persistence
    // A cancel request issued before the crash has been persisted to
    // current_operation.cancel_requested; flip the newly-minted token so
    // the replay unwinds cleanly at the first barrier instead of re-doing
    // the whole operation the operator just cancelled.
    let cancel_was_requested = state
        .db
        .call(|db| seedling_core::runtime::history::load_cancel_requested(db).unwrap_or(false));
    if cancel_was_requested {
        let outcome = state.scheduler.lock().request_cancel(&app_name);
        tracing::info!(
            app = %app_name,
            operation_id = %op.operation_id.0,
            ?outcome,
            "resuming into pre-cancelled state",
        );
    }

    tracing::info!(
        app = %app_name,
        action = %action_name,
        operation_id = %op.operation_id.0,
        "replaying interrupted operation"
    );

    seedling_core::oi::handler::actions::lifecycle::spawn_accepted_operation(
        state,
        app_name,
        action_name,
        op.operation_id,
        params,
        op.source_generation,
        op.target_generation,
        "replay".to_owned(),
        None,
    );
    true
}

// spec: TRN#listen-addresses
fn resolve_oi_addrs(
    interfaces: &[String],
    explicit: &[std::net::SocketAddr],
    port: u16,
) -> Vec<std::net::SocketAddr> {
    let available = if interfaces.is_empty() {
        Vec::new()
    } else {
        if_addrs::get_if_addrs()
            .unwrap_or_else(|e| fatal!("failed to list network interfaces: {e}"))
    };
    select_oi_addrs(interfaces, explicit, port, &available)
        .unwrap_or_else(|e| fatal!("cannot resolve OI listen addresses: {e}"))
}

/// Decide which addresses the OI server binds, given the interfaces the host
/// currently reports.
///
/// Split from [`resolve_oi_addrs`] so the decision is testable without
/// touching the host's interface list or exiting the process.
// spec: TRN#listen-addresses
fn select_oi_addrs(
    interfaces: &[String],
    explicit: &[std::net::SocketAddr],
    port: u16,
    available: &[if_addrs::Interface],
) -> Result<Vec<std::net::SocketAddr>, String> {
    if interfaces.is_empty() && explicit.is_empty() {
        return Ok(vec![format!("[::1]:{port}").parse().unwrap()]);
    }

    let mut addrs: Vec<std::net::SocketAddr> = explicit.to_vec();

    for iface_name in interfaces {
        let iface_addrs: Vec<_> = available
            .iter()
            .filter(|i| &i.name == iface_name)
            .map(|i| std::net::SocketAddr::new(i.ip(), port))
            .collect();
        // An interface that is not up yet looks exactly like one that
        // resolved to nothing. Continuing here bound the subset that did
        // resolve — or nothing at all, when this was the only interface — and
        // the daemon then logged "seedling ready" with no management plane.
        if iface_addrs.is_empty() {
            return Err(format!(
                "interface {iface_name:?} not found or has no addresses"
            ));
        }
        addrs.extend(iface_addrs);
    }

    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use if_addrs::{IfAddr, IfOperStatus, Ifv4Addr, Interface};

    use super::select_oi_addrs;

    fn iface(name: &str, ip: [u8; 4]) -> Interface {
        Interface {
            name: name.to_owned(),
            addr: IfAddr::V4(Ifv4Addr {
                ip: Ipv4Addr::from(ip),
                netmask: Ipv4Addr::new(255, 255, 255, 0),
                prefixlen: 24,
                broadcast: None,
            }),
            index: None,
            oper_status: IfOperStatus::Up,
            is_p2p: false,
            #[cfg(windows)]
            adapter_name: String::new(),
        }
    }

    // spec: SUSP#abandoned-replay
    // Startup cleanup spares the replay's resources, and before this every
    // abort path in the replay returned without them: they survived carrying
    // an operation_id no live operation claims, and the next start read the
    // same persisted row and spared them again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_abandoned_replay_hands_back_the_resources_it_was_spared() {
        use seedling_core::runtime::db::DbHandle;
        use seedling_core::system::System;

        let data_dir = tempfile::tempdir().expect("temp data dir");
        let (driver, _caddy_admin) =
            System::setup_stubbed(data_dir.path(), false).expect("stub system");
        let db = DbHandle::open_in_memory().expect("in-memory db");

        // Two resources spared for the operation that will not replay, and
        // one belonging to a different operation that must be left alone.
        insert_dynamic_resources(
            &db,
            &[
                ("inst-a", "op-abandoned"),
                ("inst-b", "op-abandoned"),
                ("inst-c", "op-other"),
            ],
        );

        super::abandon_spared_resources(&driver, &db, "op-abandoned");

        assert_eq!(
            recorded_instances(&db),
            vec!["inst-c".to_owned()],
            "the abandoned operation's resources are forgotten, another operation's are not"
        );
    }

    // spec: SUSP#abandoned-replay
    // Every teardown call fails against an unreachable backend, so nothing was
    // actually torn down. Deleting the rows anyway would leave live containers
    // with no record, no fault and nothing left to find them by: the
    // reconciler ignores instances absent from the desired state, and the next
    // start's orphan scan reads this same table.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resources_that_could_not_be_torn_down_stay_on_the_books() {
        use seedling_core::runtime::db::DbHandle;
        use seedling_core::system::System;

        let data_dir = tempfile::tempdir().expect("temp data dir");
        let (driver, _caddy_admin) =
            System::setup_unavailable(data_dir.path(), false).expect("unavailable system");
        let db = DbHandle::open_in_memory().expect("in-memory db");

        insert_dynamic_resources(
            &db,
            &[("inst-a", "op-abandoned"), ("inst-b", "op-abandoned")],
        );

        super::abandon_spared_resources(&driver, &db, "op-abandoned");

        assert_eq!(
            recorded_instances(&db),
            vec!["inst-a".to_owned(), "inst-b".to_owned()],
            "a teardown that did not happen must not be recorded as one that did"
        );
    }

    fn insert_dynamic_resources(db: &seedling_core::runtime::db::DbHandle, rows: &[(&str, &str)]) {
        let rows: Vec<(String, String)> = rows
            .iter()
            .map(|(i, o)| ((*i).to_owned(), (*o).to_owned()))
            .collect();
        db.call(move |db| {
            for (instance, op) in &rows {
                db.conn
                    .execute(
                        &format!(
                            "INSERT INTO dynamic_resources
                                 (instance_id, app, operation_id, kind, display_name)
                             VALUES ('{instance}', 'demo', '{op}', 'Job', '{instance}')"
                        ),
                        [],
                    )
                    .expect("insert dynamic resource");
            }
        });
    }

    fn recorded_instances(db: &seedling_core::runtime::db::DbHandle) -> Vec<String> {
        db.call(|db| {
            let mut stmt = db
                .conn
                .prepare("SELECT instance_id FROM dynamic_resources ORDER BY instance_id")
                .expect("prepare");
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .expect("query");
            rows.collect::<Result<_, _>>().expect("collect")
        })
    }

    // spec: TRN#listen-addresses
    #[test]
    fn no_configuration_listens_on_loopback() {
        let addrs = select_oi_addrs(&[], &[], 7891, &[]).expect("default");
        assert_eq!(addrs, vec!["[::1]:7891".parse::<SocketAddr>().unwrap()]);
    }

    // spec: TRN#listen-addresses
    #[test]
    fn a_named_interface_contributes_its_addresses() {
        let available = [iface("eth0", [10, 0, 0, 5])];
        let addrs =
            select_oi_addrs(&["eth0".to_owned()], &[], 7891, &available).expect("resolve eth0");
        assert_eq!(addrs, vec!["10.0.0.5:7891".parse::<SocketAddr>().unwrap()]);
    }

    // spec: TRN#listen-addresses
    // An interface that has not come up yet is indistinguishable from one that
    // resolved to nothing, so it must not be silently skipped: doing so left
    // the daemon reporting ready with no OI listener at all.
    #[test]
    fn an_unresolvable_interface_is_an_error_rather_than_no_listener() {
        let err = select_oi_addrs(&["eth0".to_owned()], &[], 7891, &[])
            .expect_err("must not resolve to an empty listen set");
        assert!(
            err.contains("eth0"),
            "error should name the interface: {err}"
        );
    }

    // spec: TRN#listen-addresses
    // The same applies when another interface did resolve: binding the subset
    // silently drops whichever address the operator asked for.
    #[test]
    fn one_unresolvable_interface_fails_the_whole_set() {
        let available = [iface("eth0", [10, 0, 0, 5])];
        select_oi_addrs(
            &["eth0".to_owned(), "eth1".to_owned()],
            &[],
            7891,
            &available,
        )
        .expect_err("a partial resolution must not bind the subset");
    }

    // spec: TRN#listen-addresses
    #[test]
    fn explicit_addresses_are_kept_alongside_interface_addresses() {
        let available = [iface("eth0", [10, 0, 0, 5])];
        let explicit: Vec<SocketAddr> = vec!["127.0.0.1:9000".parse().unwrap()];
        let addrs = select_oi_addrs(&["eth0".to_owned()], &explicit, 7891, &available)
            .expect("both sources");
        assert_eq!(
            addrs,
            vec![
                "127.0.0.1:9000".parse::<SocketAddr>().unwrap(),
                "10.0.0.5:7891".parse::<SocketAddr>().unwrap(),
            ]
        );
    }
}
