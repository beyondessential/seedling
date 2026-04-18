use std::{
    panic::AssertUnwindSafe,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::Parser;
use futures_util::FutureExt;
use lloggs::LoggingArgs;
use parking_lot::{Mutex, RwLock};
use seedling::{
    oi::{self, server::DEFAULT_MAX_STREAMS, state::OiState},
    runtime::{
        AppRegistry, InstanceRegistry, Scheduler, audit, db::Db, gc::GcConfig,
        registry::DbInstanceRegistry,
    },
    system::{
        System, nat64::should_activate_nat64, node_prefix_from_machine_id, reconcile::Reconciler,
        resolver::resolver_addr,
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
    nat64: seedling::system::nat64::Nat64Mode,

    /// Run without BTRFS support; use plain directories for named volumes
    #[arg(long)]
    without_btrfs: bool,
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

impl From<ScriptLimitArgs> for seedling::ScriptLimits {
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

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("ring crypto provider already installed");

    let script_limits: seedling::ScriptLimits = args.script_limits.into();

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
    let use_btrfs = match seedling::system::volume_store::is_btrfs(&data_dir) {
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
    if nat64_active && let Err(e) = seedling::system::jool::setup_nat64().await {
        // r[impl infra.nat64.translator.lifecycle]
        tracing::error!(error = %e, "failed to set up NAT64 translator; exiting");
        std::process::exit(1);
    }

    let dns_servers: Vec<std::net::Ipv6Addr> = vec![resolver_addr(&node_prefix)];

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
        AppRegistry::load_from_db(&db, Arc::clone(&tick_notify), &script_limits)
    })
    .unwrap_or_else(|e| {
        tracing::error!("failed to load registered apps: {e}");
        std::process::exit(1);
    });

    let registry = Arc::new(RwLock::new(registry));
    let db = Arc::new(Mutex::new(db));
    let scheduler = Arc::new(Mutex::new(Scheduler::new()));

    // ---------------------------------------------------------------------------
    // Startup orphan cleanup — remove dynamic resources left by a previous run
    // ---------------------------------------------------------------------------

    {
        let db = db.lock();
        match seedling::runtime::desired::list_dynamic_resources(&db) {
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

                if let Err(e) =
                    seedling::runtime::desired::delete_dynamic_resources_for_operation(&db, "")
                {
                    // delete_dynamic_resources_for_operation with "" won't match.
                    // Use a direct DELETE all instead.
                    let _ = e;
                }
                // Clear all orphaned records.
                if let Err(e) = db.conn.execute("DELETE FROM dynamic_resources", []) {
                    tracing::warn!("failed to clear orphaned dynamic resource records: {e}");
                }
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
        let all_seedling_containers: Vec<seedling::system::types::ContainerSummary> =
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    driver_ref
                        .container
                        .list(seedling::system::types::ContainerFilter {
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

    let instance_registry: Arc<dyn InstanceRegistry> = Arc::new(DbInstanceRegistry::new(Arc::new(
        parking_lot::Mutex::new(Db::open(&db_path).unwrap_or_else(|e| {
            tracing::error!("cannot open instance registry db: {e}");
            std::process::exit(1);
        })),
    )));

    let obs_db = Db::open(&db_path).unwrap_or_else(|e| {
        tracing::error!("cannot open observations db: {e}");
        std::process::exit(1);
    });

    let event_tx = seedling::oi::events::new_event_channel();
    seedling::runtime::faults::init(event_tx.clone());

    // Audit log — subscribe before anything emits events.
    let _audit_handle =
        audit::spawn_audit_task(args.audit_log, event_tx.subscribe(), Arc::clone(&db));

    // Periodic garbage collection of operational tables.
    let _gc_handle = seedling::runtime::gc::spawn_gc_task(Arc::clone(&db), args.gc.into());

    let shells = seedling::oi::shells::ShellRegistry::new();

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
        nat64_active,
        Arc::clone(&shells),
    );

    // The reconciler and schedule ticker are spawned below, after OiState is
    // constructed, so that scheduled-action fires can spawn lifecycle operations.
    let reconciler_handle = {
        let tick_notify = Arc::clone(&tick_notify);
        let schedule_db = Arc::clone(&db);
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
        db: Arc::clone(&db),
        scheduler: Arc::clone(&scheduler),
        tick_notify: Arc::clone(&tick_notify),
        db_path: db_path.clone(),
        trusted_keys: seedling::oi::auth::new_trusted_keys(),
        shells,
        forwards: seedling::oi::forwards::ForwardRegistry::new(),
        container_runtime: Arc::clone(&driver.container),
        driver: Arc::clone(&driver),
        node_prefix,
        event_tx: event_tx.clone(),
        script_limits,
        dns_servers,
    });

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
            let mut schedule_ticker = seedling::runtime::schedules::ScheduleTicker::new();
            // r[impl backup.execution]
            let mut backup_ticker = seedling::runtime::backup_execution::BackupTicker::new();
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
                let accepted_fires = {
                    let db_guard = schedule_db.lock();
                    let mut sched = schedule_scheduler.lock();
                    let reg = schedule_registry.read();
                    let fired = schedule_ticker.maybe_tick(&db_guard, &mut sched, &|app_name| {
                        reg.get(app_name).map(|e| e.current_generation)
                    });
                    fired
                        .into_iter()
                        .filter(|f| f.accepted && f.operation_id.is_some())
                        .collect::<Vec<_>>()
                };
                for fire in accepted_fires {
                    if let Some(op_id) = fire.operation_id {
                        seedling::oi::handler::actions::lifecycle::spawn_accepted_operation(
                            Arc::clone(&oi_state_for_sched),
                            fire.app,
                            fire.action,
                            op_id,
                            serde_json::Map::new(),
                            fire.generation,
                            fire.generation,
                            "schedule".to_owned(),
                        );
                    }
                }

                // r[impl backup.execution]
                let due_strategies = {
                    let db_guard = schedule_db.lock();
                    backup_ticker.maybe_tick(&db_guard)
                };
                for due in due_strategies {
                    seedling::oi::handler::backups::spawn_backup_run(
                        Arc::clone(&oi_state_for_sched),
                        seedling::runtime::backup_strategies::BackupStrategy {
                            name: due.name,
                            via: due.via,
                            schedule: due.schedule,
                            volumes: due.volumes,
                            last_fired_at: None,
                        },
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

    let (_fingerprint, oi_endpoint) = oi::run(
        Arc::clone(&oi_state),
        oi::DEFAULT_PORT,
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
    oi_endpoint.close(quinn::VarInt::from_u32(0), b"shutdown");
    oi_endpoint.wait_idle().await;
}
