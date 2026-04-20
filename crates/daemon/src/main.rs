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
        gc::GcConfig,
        registry::DbInstanceRegistry,
    },
    system::{
        System,
        nat64::should_activate_nat64,
        node_prefix_from_machine_id,
        reconcile::Reconciler,
        resolver::{resolver_addr, resolver_gateway_addr, spawn_dns_forwarder},
    },
};
use tokio::sync::Notify;

#[derive(Parser)]
#[command(name = "seedling")]
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

    // i[transport.listen]
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
    // r[impl infra.resolver.upstreams]
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
            retain_unscheduled_instances: Duration::from_secs(
                a.gc_retain_unscheduled_instances_secs,
            ),
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

    std::fs::create_dir_all(&data_dir).unwrap_or_else(|e| {
        tracing::error!("cannot create data directory {}: {e}", data_dir.display());
        std::process::exit(1);
    });

    let data_dir = std::fs::canonicalize(&data_dir).unwrap_or_else(|e| {
        tracing::error!(
            "cannot canonicalize data directory {}: {e}",
            data_dir.display()
        );
        std::process::exit(1);
    });

    // r[impl startup.btrfs]
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
        Ok(false) => {
            tracing::error!(
                "data directory {} is not on a BTRFS filesystem; \
                 pass --without-btrfs to use plain directories instead",
                data_dir.display()
            );
            std::process::exit(1);
        }
        Err(e) => {
            tracing::error!(
                "cannot determine filesystem type for {}: {e}",
                data_dir.display()
            );
            std::process::exit(1);
        }
    };

    let db_path = data_dir.join("seedling.db");
    let db = Db::open(&db_path).unwrap_or_else(|e| {
        tracing::error!("cannot open database {}: {e}", db_path.display());
        std::process::exit(1);
    });

    let cipher = {
        let key_path = data_dir.join("seedling.db.key");
        let c = seedling_core::runtime::secrets::Cipher::load_or_create(&key_path).unwrap_or_else(
            |e| {
                tracing::error!(
                    "cannot load or create secret key {}: {e}",
                    key_path.display()
                );
                std::process::exit(1);
            },
        );
        std::sync::Arc::new(c)
    };

    // ---------------------------------------------------------------------------
    // System backends
    // ---------------------------------------------------------------------------

    let node_prefix = node_prefix_from_machine_id().unwrap_or_else(|e| {
        tracing::error!("cannot derive node prefix from machine-id: {e}");
        std::process::exit(1);
    });

    let tick_notify = Arc::new(Notify::new());

    // r[impl infra.nat64.mode]
    // r[impl infra.nat64.detection]
    let nat64_active = should_activate_nat64(args.nat64).await;
    tracing::info!(nat64_mode = %args.nat64, nat64_active, "NAT64 decision");

    // r[impl infra.nat64.translator]
    if nat64_active && let Err(e) = seedling_core::system::jool::setup_nat64().await {
        // r[impl infra.nat64.translator.lifecycle]
        tracing::error!(error = %e, "failed to set up NAT64 translator; exiting");
        std::process::exit(1);
    }

    let dns_servers: Vec<std::net::Ipv6Addr> = vec![resolver_addr(&node_prefix)];

    // DNS upstream resolution. By default CoreDNS forwards to an
    // in-process proxy on the resolver-bridge gateway which in turn
    // forwards to systemd-resolved's extra stub (127.0.0.54:53), so
    // containers inherit the host's split DNS / MagicDNS / search
    // domains. `--dns-upstreams` overrides this and points CoreDNS
    // straight at the operator-supplied servers.
    let (dns_upstreams, forwarder_handle) = if args.dns_upstreams.is_empty() {
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

    let (driver, caddy_admin_client) = System::setup(node_prefix, &data_dir, use_btrfs)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("system setup failed: {e}");
            std::process::exit(1);
        });

    // ---------------------------------------------------------------------------
    // App registry — load registered apps from DB
    // ---------------------------------------------------------------------------

    let registry = tokio::task::block_in_place(|| {
        AppRegistry::load_from_db(&db, &cipher, Arc::clone(&tick_notify), &script_limits)
    })
    .unwrap_or_else(|e| {
        tracing::error!("failed to load registered apps: {e}");
        std::process::exit(1);
    });

    let registry = Arc::new(RwLock::new(registry));
    let db = DbHandle::from_db(db);
    let scheduler = Arc::new(parking_lot::Mutex::new(Scheduler::new()));

    // ---------------------------------------------------------------------------
    // Load the persisted in-flight operation (if any) so we can both skip its
    // resources during orphan cleanup below and replay it after the reconciler
    // comes online.
    // r[impl operation.lifecycle.events] r[impl barrier.replay]
    // ---------------------------------------------------------------------------

    let persisted_operation = db
        .call(seedling_core::runtime::history::load_current_operation)
        .unwrap_or_else(|e| {
            tracing::warn!("failed to load current_operation on startup: {e}");
            None
        });
    if let Some(op) = &persisted_operation {
        tracing::info!(
            app = %op.app,
            action = %op.action_name,
            operation_id = %op.operation_id.0,
            "found interrupted operation; will replay after startup"
        );
    }

    // ---------------------------------------------------------------------------
    // Startup orphan cleanup — remove dynamic resources left by a previous run.
    // Resources owned by the operation we are about to replay are kept so the
    // replay picks up where it left off.
    // ---------------------------------------------------------------------------

    let replay_op_id: Option<String> = persisted_operation
        .as_ref()
        .map(|op| op.operation_id.0.clone());
    {
        let records = db.call(seedling_core::runtime::desired::list_dynamic_resources);
        // Keep resources that belong to the operation we're about to replay;
        // the replay will take ownership of them. Everything else from a
        // prior run gets cleaned up here.
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
                for record in &records {
                    // Stop any lingering systemd unit for this instance.
                    let unit = format!("seedling-{}.service", record.display_name);
                    tracing::info!(
                        instance = %record.display_name,
                        kind = %record.kind,
                        operation_id = %record.operation_id,
                        "cleaning up orphaned dynamic resource"
                    );
                    // The actual container/unit stop will be handled by the
                    // reconciler on the first tick — it won't see these in the
                    // desired state and will ignore them. For systemd units that
                    // are still loaded, reset them so they don't linger.
                    let driver_ref = Arc::clone(&driver);
                    let unit_name = unit.clone();
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            if let Ok(Some(_)) = driver_ref.process.unit_state(&unit_name).await {
                                let _ = driver_ref.process.stop_unit(&unit_name).await;
                                let _ = driver_ref.process.reset_failed_unit(&unit_name).await;
                            }
                        });
                    });

                    // Remove the pod network if it exists.
                    let net_name = format!("seedling-{}", record.display_name);
                    let driver_ref = Arc::clone(&driver);
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            if driver_ref
                                .container
                                .network_exists(&net_name)
                                .await
                                .unwrap_or(false)
                            {
                                let _ = driver_ref.container.remove_network(&net_name).await;
                            }
                        });
                    });

                    // Force-remove the container if it outlived the unit.
                    let display = record.display_name.clone();
                    let driver_ref = Arc::clone(&driver);
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            let _ = driver_ref.container.remove_container(&display, true).await;
                        });
                    });
                }

                // Clear per-instance faults for every orphan we just tore
                // down, so image_pull_failed / container_start_failed
                // filed in a previous run don't linger after restart.
                // r[fault.image-pull] r[fault.container-start]
                let faulted_instances: Vec<(String, String)> = records
                    .iter()
                    .filter(|r| Some(&r.operation_id) != replay_op_id.as_ref())
                    .map(|r| (r.app.clone(), r.instance_id.clone()))
                    .collect();
                db.call(move |db| {
                    for (app, instance_id) in &faulted_instances {
                        if let Err(e) = seedling_core::runtime::faults::clear_faults_for_instance(
                            db,
                            app,
                            instance_id,
                        ) {
                            tracing::warn!(
                                app = %app,
                                instance_id = %instance_id,
                                "failed to clear per-instance faults during orphan cleanup: {e}"
                            );
                        }
                    }
                });

                // Clear cleaned records but keep rows owned by the operation
                // we are about to replay.
                let preserve_op_id = replay_op_id.clone();
                db.call(move |db| {
                    let result = match &preserve_op_id {
                        Some(op_id) => db.conn.execute(
                            "DELETE FROM dynamic_resources WHERE operation_id != ?1",
                            [op_id],
                        ),
                        None => db.conn.execute("DELETE FROM dynamic_resources", []),
                    };
                    if let Err(e) = result {
                        tracing::warn!("failed to clear orphaned dynamic resource records: {e}");
                    }
                });
                tracing::info!("orphaned dynamic resource cleanup complete");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("failed to check for orphaned dynamic resources: {e}");
            }
        }
    }

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

    // Podman-level scan for orphaned seedling containers.
    // Any container with a "seedling.app" label that doesn't belong to a
    // currently registered app is an orphan.
    {
        let known_apps: std::collections::HashSet<String> = {
            let reg = registry.read();
            reg.list().into_iter().map(|(name, _)| name).collect()
        };

        let driver_ref = Arc::clone(&driver);
        let all_seedling_containers: Vec<seedling_core::system::types::ContainerSummary> =
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    driver_ref
                        .container
                        .list(seedling_core::system::types::ContainerFilter {
                            label_key: Some("seedling.app"),
                            ..Default::default()
                        })
                        .await
                        .unwrap_or_default()
                })
            });

        let orphans: Vec<_> = all_seedling_containers
            .iter()
            .filter(|c| {
                c.labels
                    .get("seedling.app")
                    .is_none_or(|app| !known_apps.contains(app))
            })
            .collect();

        if !orphans.is_empty() {
            tracing::warn!(
                count = orphans.len(),
                "found orphaned seedling containers; removing"
            );
            let driver_ref = Arc::clone(&driver);
            for container in &orphans {
                tracing::info!(
                    container = %container.name,
                    app = container.labels.get("seedling.app").map(|s| s.as_str()).unwrap_or("?"),
                    "removing orphaned container"
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

    // ---------------------------------------------------------------------------
    // Global reconciler
    // ---------------------------------------------------------------------------

    let instance_registry: Arc<dyn InstanceRegistry> = Arc::new(DbInstanceRegistry::new(
        DbHandle::open(&db_path).unwrap_or_else(|e| {
            tracing::error!("cannot open instance registry db: {e}");
            std::process::exit(1);
        }),
    ));

    let obs_db = DbHandle::open(&db_path).unwrap_or_else(|e| {
        tracing::error!("cannot open observations db: {e}");
        std::process::exit(1);
    });

    let event_tx = seedling_protocol::events::new_event_channel();
    seedling_core::runtime::faults::init(event_tx.clone());

    // Audit log — subscribe before anything emits events.
    let _audit_handle = audit::spawn_audit_task(args.audit_log, event_tx.subscribe(), db.clone());

    // Periodic garbage collection of operational tables.
    let _gc_handle = seedling_core::runtime::gc::spawn_gc_task(db.clone(), args.gc.into());

    let shells = seedling_core::oi::shells::ShellRegistry::new();

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
        Arc::clone(&shells),
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

    let oi_state = Arc::new(OiState {
        registry: Arc::clone(&registry),
        spki_fingerprint: std::sync::OnceLock::new(),
        start_time: Instant::now(),
        db: db.clone(),
        scheduler: Arc::clone(&scheduler),
        tick_notify: Arc::clone(&tick_notify),
        db_path: db_path.clone(),
        trusted_keys: seedling_core::oi::auth::new_trusted_keys(),
        shells,
        forwards: seedling_core::oi::forwards::ForwardRegistry::new(),
        container_runtime: Arc::clone(&driver.container),
        driver: Arc::clone(&driver),
        node_prefix,
        event_tx: event_tx.clone(),
        script_limits,
        dns_servers,
        cipher,
    });

    // ---------------------------------------------------------------------------
    // Replay any interrupted lifecycle operation. Runs after OiState is built
    // but before the reconciler starts ticking, so the install closure's
    // rt.start() calls are picked up by the first reconciliation tick.
    // r[impl operation.lifecycle.events] r[impl barrier.replay]
    // ---------------------------------------------------------------------------

    if let Some(op) = persisted_operation {
        replay_interrupted_operation(Arc::clone(&oi_state), op);
    }

    // Pre-pull the ubuntu image used by volume shells so it is warm before
    // the first operator opens a volume shell session.
    {
        let cr = Arc::clone(&driver.container);
        tokio::spawn(async move {
            let image = "ubuntu:latest";
            match cr.image_exists(image).await {
                Ok(false) => {
                    tracing::info!(%image, "pre-pulling volume shell image");
                    if let Err(e) = cr.pull_image(image).await {
                        tracing::warn!(%image, "volume shell image pre-pull failed: {e}");
                    }
                }
                Ok(true) => {}
                Err(e) => tracing::warn!(%image, "image_exists check failed: {e}"),
            }
        });
    }

    // Spawn the reconciler + schedule ticker now that OiState is available.
    {
        let (mut reconciler, tick_notify, schedule_db, schedule_scheduler, schedule_registry) =
            reconciler_handle;
        let oi_state_for_sched = Arc::clone(&oi_state);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut idle = false;
            // r[impl schedule.tick]
            let mut schedule_ticker = seedling_core::runtime::schedules::ScheduleTicker::new();
            // r[impl backup.execution]
            let mut backup_ticker = seedling_core::runtime::backup_execution::BackupTicker::new();
            loop {
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

                // r[impl schedule.tick]
                // Snapshot generations before acquiring the DB to maintain
                // consistent lock order (registry → db) across the codebase.
                if let Some((now, is_startup)) = schedule_ticker.maybe_tick() {
                    let app_generations: std::collections::HashMap<String, u64> = {
                        let reg = schedule_registry.read();
                        reg.list()
                            .into_iter()
                            .filter_map(|(name, _)| {
                                reg.get(&name).map(|e| (name, e.current_generation))
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
                            is_startup,
                            &|app_name| app_generations.get(app_name).copied(),
                        );
                        drop(sched);
                        fired
                            .into_iter()
                            .filter(|f| f.accepted && f.operation_id.is_some())
                            .collect::<Vec<_>>()
                    });
                    for fire in accepted_fires {
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

                // r[impl backup.execution]
                let due_strategies = if let Some(now) = backup_ticker.maybe_tick() {
                    schedule_db.call(move |db| {
                        seedling_core::runtime::backup_execution::check_due_strategies(db, now)
                    })
                } else {
                    Vec::new()
                };
                for due in due_strategies {
                    let ids: Vec<_> = due
                        .volumes
                        .iter()
                        .map(|_| seedling_core::runtime::barrier::OperationId::new())
                        .collect();
                    seedling_core::oi::handler::backups::spawn_backup_run(
                        Arc::clone(&oi_state_for_sched),
                        seedling_core::runtime::backup_strategies::BackupStrategy {
                            name: due.name,
                            via: due.via,
                            schedule: due.schedule,
                            volumes: due.volumes,
                            last_fired_at: None,
                        },
                        ids,
                        false,
                    );
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

    // i[transport.listen]
    let oi_addrs = resolve_oi_addrs(&args.interface, &args.listen, args.port);
    let (_fingerprint, oi_endpoints) = oi::run(
        Arc::clone(&oi_state),
        &oi_addrs,
        &data_dir,
        args.max_streams,
    )
    .await
    .unwrap_or_else(|e| {
        tracing::error!("OI server failed to start: {e}");
        std::process::exit(1);
    });

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

// r[impl operation.lifecycle.events] r[impl barrier.replay]
// i[impl action.invoke.install.validation]
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
fn replay_interrupted_operation(
    state: Arc<OiState>,
    op: seedling_core::runtime::history::CurrentOperation,
) {
    use seedling_core::runtime::{
        apps::AppPhase,
        history::{clear_current_operation, load_current_install_params},
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
        reg.get(&app_name).map(|e| e.phase.lock().clone())
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
        return;
    };

    // For install, load and decrypt the persisted params.
    let params = if is_install {
        let cipher = Arc::clone(&state.cipher);
        let loaded = state
            .db
            .call(move |db| load_current_install_params(db, &cipher));
        match loaded {
            Ok(Some(params)) => params,
            Ok(None) => {
                tracing::error!(
                    app = %app_name,
                    "interrupted install has no persisted params; reverting to NotInstalled"
                );
                revert_install_and_fault(&state, &app_name);
                return;
            }
            Err(e) => {
                tracing::error!(
                    app = %app_name,
                    "failed to decrypt interrupted install params: {e}; reverting to NotInstalled"
                );
                revert_install_and_fault(&state, &app_name);
                return;
            }
        }
    } else {
        serde_json::Map::new()
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
        return;
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
        return;
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
}

/// Flip a persisted Installing row back to NotInstalled and file a fault
/// indicating the install was interrupted and could not be replayed.
// i[impl action.invoke.install.completion]
fn revert_install_and_fault(state: &Arc<OiState>, app_name: &str) {
    use seedling_core::runtime::apps::AppPhase;
    use seedling_core::runtime::history::clear_current_operation;

    // Only flip if the app is actually in Installing.
    let was_installing = {
        let reg = state.registry.read();
        reg.get(app_name)
            .map(|e| matches!(*e.phase.lock(), AppPhase::Installing))
            .unwrap_or(false)
    };

    if was_installing {
        {
            let mut reg = state.registry.write();
            if let Some(entry) = reg.get_mut(app_name) {
                *entry.phase.lock() = AppPhase::NotInstalled;
            }
        }
        let app_owned = app_name.to_owned();
        state.db.call(move |db| {
            if let Err(e) = db.conn.execute(
                "UPDATE registered_apps SET installing = 0, installed = 0, uninstalling = 0 \
                 WHERE name = ?1",
                [&app_owned],
            ) {
                tracing::warn!(app = %app_owned, "failed to persist revert: {e}");
            }
            let _ = clear_current_operation(db);
            let _ = seedling_core::runtime::faults::file_fault(
                db,
                &app_owned,
                None,
                None,
                None,
                "install_interrupted",
                "install was interrupted by a runtime restart and could not be replayed",
            );
        });
        state
            .event_tx
            .app_phase_changed(app_name, "not_installed", None);
    } else {
        state.db.call(|db| {
            let _ = clear_current_operation(db);
        });
    }
}

// i[transport.listen]
fn resolve_oi_addrs(
    interfaces: &[String],
    explicit: &[std::net::SocketAddr],
    port: u16,
) -> Vec<std::net::SocketAddr> {
    if interfaces.is_empty() && explicit.is_empty() {
        return vec![format!("[::1]:{port}").parse().unwrap()];
    }

    let mut addrs: Vec<std::net::SocketAddr> = explicit.to_vec();

    for iface_name in interfaces {
        let all = if_addrs::get_if_addrs().unwrap_or_else(|e| {
            tracing::error!("failed to list network interfaces: {e}");
            std::process::exit(1);
        });
        let iface_addrs: Vec<_> = all
            .into_iter()
            .filter(|i| &i.name == iface_name)
            .map(|i| std::net::SocketAddr::new(i.ip(), port))
            .collect();
        if iface_addrs.is_empty() {
            tracing::warn!("interface {iface_name:?} not found or has no addresses");
            continue;
        }
        addrs.extend(iface_addrs);
    }

    addrs
}
