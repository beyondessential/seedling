use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::Parser;
use lloggs::LoggingArgs;
use parking_lot::{Mutex, RwLock};
use seedling::{
    oi::{self, handler::OiState},
    runtime::{AppRegistry, InstanceRegistry, Scheduler, db::Db, registry::DbInstanceRegistry},
    system::{
        System,
        reconcile::{Reconciler, node_prefix_from_machine_id},
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

    let (driver, caddy_admin_addr) =
        System::setup(node_prefix, &data_dir)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("system setup failed: {e}");
                std::process::exit(1);
            });

    // ---------------------------------------------------------------------------
    // App registry — load registered apps from DB
    // ---------------------------------------------------------------------------

    let registry =
        tokio::task::block_in_place(|| AppRegistry::load_from_db(&db, Arc::clone(&tick_notify)))
            .unwrap_or_else(|e| {
                tracing::error!("failed to load registered apps: {e}");
                std::process::exit(1);
            });

    let registry = Arc::new(RwLock::new(registry));
    let db = Arc::new(Mutex::new(db));
    let scheduler = Arc::new(Mutex::new(Scheduler::new()));

    // ---------------------------------------------------------------------------
    // Global reconciler
    // ---------------------------------------------------------------------------

    let instance_registry: Arc<dyn InstanceRegistry> = Arc::new(DbInstanceRegistry::new(
        Db::open(&db_path).unwrap_or_else(|e| {
            tracing::error!("cannot open instance registry db: {e}");
            std::process::exit(1);
        }),
    ));

    let obs_db = Db::open(&db_path).unwrap_or_else(|e| {
        tracing::error!("cannot open observations db: {e}");
        std::process::exit(1);
    });

    let mut reconciler = Reconciler::new(
        Arc::clone(&driver),
        node_prefix,
        instance_registry,
        Arc::clone(&caddy_admin_addr),
        data_dir.clone(),
        obs_db,
        Arc::clone(&registry),
    );

    {
        let tick_notify = Arc::clone(&tick_notify);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut idle = false;
            loop {
                if idle {
                    // No apps installed — suspend the interval and wait for an
                    // explicit wake (app install/update).
                    tick_notify.notified().await;
                    idle = false;
                    // Reset the interval so the first active tick fires immediately.
                    interval.reset();
                } else {
                    tokio::select! {
                        _ = interval.tick() => {},
                        _ = tick_notify.notified() => {},
                    }
                }
                let active = reconciler.tick().await;
                if !active {
                    idle = true;
                }
            }
        });
    }

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
        shells: seedling::oi::shells::ShellRegistry::new(),
        forwards: seedling::oi::forwards::ForwardRegistry::new(),
        container_runtime: Arc::clone(&driver.container),
        node_prefix,
    });

    oi::run(Arc::clone(&oi_state), oi::DEFAULT_PORT, &data_dir)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("OI server failed to start: {e}");
            std::process::exit(1);
        });

    tracing::info!("seedling ready");
    tokio::signal::ctrl_c().await.ok();
}
