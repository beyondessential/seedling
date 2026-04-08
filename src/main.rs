use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::Parser;
use lloggs::LoggingArgs;
use parking_lot::{Mutex, RwLock};
use seedling::{
    oi::{
        self,
        handler::{OiState, ReconcilerFactory},
    },
    runtime::{AppRegistry, InstanceRegistry, Scheduler, db::Db, registry::DbInstanceRegistry},
    system::{
        System,
        reconcile::{Reconciler, node_prefix_from_machine_id},
    },
};

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
                0 => "seedling=info,warn",
                1 => "seedling=debug,warn",
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
        tokio::task::block_in_place(|| AppRegistry::load_from_db(&db)).unwrap_or_else(|e| {
            tracing::error!("failed to load registered apps: {e}");
            std::process::exit(1);
        });

    let installed_apps: Vec<_> = registry
        .list()
        .into_iter()
        .filter(|(_, status)| !matches!(status, seedling::runtime::AppStatus::NotInstalled))
        .map(|(name, _)| name)
        .collect();

    let registry = Arc::new(RwLock::new(registry));
    let db = Arc::new(Mutex::new(db));
    let scheduler = Arc::new(Mutex::new(Scheduler::new()));
    let reconciler_factory = Arc::new(ReconcilerFactory {
        system: Arc::clone(&driver),
        node_prefix,
        db_path: db_path.clone(),
        data_dir: data_dir.clone(),
        caddy_admin_addr: Arc::clone(&caddy_admin_addr),
    });

    // ---------------------------------------------------------------------------
    // Reconcilers — one per installed app
    // ---------------------------------------------------------------------------

    for app_name in &installed_apps {
        let reg = registry.read();
        let entry = match reg.get(app_name) {
            Some(e) => e,
            None => continue,
        };

        let app = entry.app.clone();
        let active_progress = Arc::clone(&entry.active_progress);
        let tick_notify = Arc::clone(&entry.tick_notify);
        let app_name = app_name.clone();
        drop(reg);

        let instance_registry: Arc<dyn InstanceRegistry> = Arc::new(DbInstanceRegistry::new(
            Db::open(&db_path).unwrap_or_else(|e| {
                tracing::error!("cannot open registry db for app {app_name}: {e}");
                std::process::exit(1);
            }),
        ));

        let obs_db = Db::open(&db_path).unwrap_or_else(|e| {
            tracing::error!("cannot open observations db for app {app_name}: {e}");
            std::process::exit(1);
        });

        let driver = Arc::clone(&driver);
        let caddy_admin_addr = Arc::clone(&caddy_admin_addr);
        let data_dir = data_dir.clone();

        let mut reconciler = Reconciler::new(
            app_name.clone(),
            app,
            active_progress,
            driver,
            node_prefix,
            instance_registry,
            HashMap::new(),
            caddy_admin_addr,
            data_dir,
            obs_db,
        );

        reconciler.populate_bridge_names().await;

        let handle = tokio::spawn(async move {
            let mut r = reconciler;
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {},
                    _ = tick_notify.notified() => {},
                }
                r.tick().await;
            }
        });

        {
            let mut reg = registry.write();
            if let Some(entry) = reg.get_mut(&app_name) {
                entry.reconciler_handle = Some(handle);
            }
        }

        tracing::info!("started reconciler for app: {app_name}");
    }

    // ---------------------------------------------------------------------------
    // OI server
    // ---------------------------------------------------------------------------

    let oi_state = Arc::new(OiState {
        registry: Arc::clone(&registry),
        spki_fingerprint: std::sync::OnceLock::new(),
        start_time: Instant::now(),
        db: Arc::clone(&db),
        scheduler: Arc::clone(&scheduler),
        reconciler_factory: Arc::clone(&reconciler_factory),
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
