use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use parking_lot::RwLock;
use seedling::{
    oi::{self, handler::OiState},
    runtime::{
        AppRegistry, InstanceRegistry, db::Db, desired::OperationProgress,
        registry::DbInstanceRegistry,
    },
    system::{
        System,
        reconcile::{Reconciler, node_prefix_from_machine_id},
    },
};
use tokio::sync::Notify;

fn parse_data_dir() -> PathBuf {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut data_dir: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--data-dir" {
            match args.get(i + 1) {
                Some(dir) => {
                    data_dir = Some(PathBuf::from(dir));
                    i += 2;
                }
                None => {
                    eprintln!("error: --data-dir requires an argument");
                    std::process::exit(1);
                }
            }
        } else {
            eprintln!("error: unexpected argument: {}", args[i].to_string_lossy());
            eprintln!("usage: seedling [--data-dir <DIR>]");
            std::process::exit(1);
        }
    }
    data_dir.unwrap_or_else(|| PathBuf::from("."))
}

#[tokio::main]
async fn main() {
    let data_dir = parse_data_dir();

    std::fs::create_dir_all(&data_dir).unwrap_or_else(|e| {
        eprintln!(
            "error: cannot create data directory {}: {e}",
            data_dir.display()
        );
        std::process::exit(1);
    });

    let db_path = data_dir.join("seedling.db");
    let db = Db::open(&db_path).unwrap_or_else(|e| {
        eprintln!("error: cannot open database {}: {e}", db_path.display());
        std::process::exit(1);
    });

    // ---------------------------------------------------------------------------
    // System backends
    // ---------------------------------------------------------------------------

    let node_prefix = node_prefix_from_machine_id().unwrap_or_else(|e| {
        eprintln!("error: cannot derive node prefix from machine-id: {e}");
        std::process::exit(1);
    });

    let (driver, caddy_admin_addr) =
        System::setup(node_prefix, &data_dir)
            .await
            .unwrap_or_else(|e| {
                eprintln!("error: system setup failed: {e}");
                std::process::exit(1);
            });

    // ---------------------------------------------------------------------------
    // App registry — load registered apps from DB
    // ---------------------------------------------------------------------------

    let registry =
        tokio::task::block_in_place(|| AppRegistry::load_from_db(&db)).unwrap_or_else(|e| {
            eprintln!("error: failed to load registered apps: {e}");
            std::process::exit(1);
        });

    let installed_apps: Vec<_> = registry
        .list()
        .into_iter()
        .filter(|(_, status)| !matches!(status, seedling::runtime::AppStatus::NotInstalled))
        .map(|(name, _)| name)
        .collect();

    let registry = Arc::new(RwLock::new(registry));

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
                eprintln!("error: cannot open registry db for app {app_name}: {e}");
                std::process::exit(1);
            }),
        ));

        let obs_db = Db::open(&db_path).unwrap_or_else(|e| {
            eprintln!("error: cannot open observations db for app {app_name}: {e}");
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

        tokio::spawn(async move {
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

        eprintln!("started reconciler for app: {app_name}");
    }

    // ---------------------------------------------------------------------------
    // OI server
    // ---------------------------------------------------------------------------

    let oi_state = Arc::new(OiState {
        registry: Arc::clone(&registry),
        spki_fingerprint: std::sync::OnceLock::new(),
        start_time: Instant::now(),
    });

    // Run the server; it prints the fingerprint to stderr.
    oi::run(Arc::clone(&oi_state), oi::DEFAULT_PORT, &data_dir)
        .await
        .unwrap_or_else(|e| {
            eprintln!("error: OI server failed to start: {e}");
            std::process::exit(1);
        });

    eprintln!("seedling ready. Ctrl-C to exit.");
    tokio::signal::ctrl_c().await.ok();
}
