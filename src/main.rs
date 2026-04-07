use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use parking_lot::RwLock;
use rhai::{AST, Engine, Scope};
use seedling::{
    defs::app::App,
    runtime::{
        InstanceRegistry, OperationProgress,
        barrier::{
            OperationId,
            oracle::DbWorldOracle,
            replay::{ActionLog, DbActionLog, OperationContext, OperationResult, run_operation},
        },
        db::Db,
        history::{
            CurrentOperation, clear_current_operation, load_current_operation,
            save_current_operation,
        },
        registry::DbInstanceRegistry,
        scheduler::{RejectReason, ScheduleResult, Scheduler},
    },
    setup_language,
    system::{
        System,
        reconcile::{Reconciler, node_prefix_from_machine_id},
    },
};
use tokio::sync::Notify;

fn run_file(
    engine: &Engine,
    scope: &mut Scope,
    path: PathBuf,
) -> Result<AST, Box<rhai::EvalAltResult>> {
    let ast = engine.compile_file(path)?;
    engine.run_ast_with_scope(scope, &ast)?;
    Ok(ast)
}

fn parse_args() -> (PathBuf, PathBuf) {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: seedling <SCRIPT.rhai> [--data-dir <DIR>]");
        std::process::exit(1);
    }
    let script_path = PathBuf::from(&args[0]);

    let mut data_dir: Option<PathBuf> = None;
    let mut i = 1;
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
            i += 1;
        }
    }

    let data_dir = data_dir.unwrap_or_else(|| {
        script_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_owned()
    });

    (script_path, data_dir)
}

// r[impl operation.lifecycle.events]
// r[impl barrier.replay]
fn find_or_create_operation(db: &Db, app: &App, app_name: &str) -> Option<CurrentOperation> {
    match load_current_operation(db).unwrap_or_else(|e| {
        eprintln!("error: failed to query current operation: {e}");
        std::process::exit(1);
    }) {
        Some(op) => {
            eprintln!(
                "resuming interrupted '{}/{}' [{}]",
                op.app, op.action_name, op.operation_id.0
            );
            Some(op)
        }
        None => {
            let has_start = app.def.lock().actions.contains_key("start");
            if !has_start {
                return None;
            }
            let op = CurrentOperation {
                operation_id: OperationId::new(),
                app: app_name.to_owned(),
                action_name: "start".to_owned(),
            };
            save_current_operation(db, &op).unwrap_or_else(|e| {
                eprintln!("error: failed to save current operation: {e}");
                std::process::exit(1);
            });
            eprintln!(
                "starting '{}/{}' [{}]",
                op.app, op.action_name, op.operation_id.0
            );
            Some(op)
        }
    }
}

#[tokio::main]
async fn main() {
    let (script_path, data_dir) = parse_args();

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

    let (engine, mut scope, app) = setup_language();
    let ast = run_file(&engine, &mut scope, script_path.clone()).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });

    let app_name = script_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("app")
        .to_owned();

    app.def.lock().name = app_name.clone();

    {
        let def = app.def.lock();
        eprintln!("app: {app_name}");
        eprintln!("  resources: {}", def.resources.len());
        for id in def.resources.keys() {
            eprintln!("    {:?} {:?}", id.kind, id.name);
        }
        eprintln!("  actions: {:?}", def.actions.keys().collect::<Vec<_>>());
    }

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
    // Instance registry (shared between reconciler and operation runner)
    // ---------------------------------------------------------------------------

    let registry: Arc<dyn InstanceRegistry> = Arc::new(DbInstanceRegistry::new(
        Db::open(&db_path).unwrap_or_else(|e| {
            eprintln!("error: cannot open registry database: {e}");
            std::process::exit(1);
        }),
    ));

    // ---------------------------------------------------------------------------
    // Reconciler
    // ---------------------------------------------------------------------------

    let active_progress: Arc<RwLock<Option<OperationProgress>>> = Arc::new(RwLock::new(None));

    let obs_db = Db::open(&db_path).unwrap_or_else(|e| {
        eprintln!("error: cannot open observations database: {e}");
        std::process::exit(1);
    });

    let mut reconciler = Reconciler::new(
        app_name.clone(),
        app.clone(),
        Arc::clone(&active_progress),
        Arc::clone(&driver),
        node_prefix,
        Arc::clone(&registry),
        HashMap::new(), // bridge_names: populated by populate_bridge_names below
        caddy_admin_addr,
        data_dir.clone(),
        obs_db,
    );

    reconciler.populate_bridge_names().await;

    let tick_notify = Arc::new(Notify::new());

    tokio::spawn({
        let notify = Arc::clone(&tick_notify);
        async move {
            let mut r = reconciler;
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            // Skip missed ticks rather than firing a burst of them on resume.
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {},
                    _ = notify.notified() => {},
                }
                r.tick().await;
            }
        }
    });

    // ---------------------------------------------------------------------------
    // Operation runner
    // ---------------------------------------------------------------------------

    let Some(current_op) = find_or_create_operation(&db, &app, &app_name) else {
        eprintln!("no pending operation; reconciler running in steady state. Ctrl-C to exit.");
        tokio::signal::ctrl_c().await.ok();
        return;
    };

    let mut scheduler = Scheduler::new();
    match scheduler.request(&current_op.app, &current_op.action_name) {
        ScheduleResult::Accepted => {}
        ScheduleResult::Rejected(reason) => {
            let msg = match reason {
                RejectReason::SameAppOperationInProgress => "operation already in progress",
                RejectReason::SameAppAlreadyQueued => "operation already queued",
            };
            eprintln!("internal error: scheduler rejected boot operation: {msg}");
            std::process::exit(1);
        }
    }

    let oracle = Arc::new(DbWorldOracle::new(Db::open(&db_path).unwrap_or_else(|e| {
        eprintln!("error: cannot open oracle database: {e}");
        std::process::exit(1);
    })));
    let log = DbActionLog::new(
        Db::open(&db_path).unwrap_or_else(|e| {
            eprintln!("error: cannot open log database: {e}");
            std::process::exit(1);
        }),
        current_op.operation_id.clone(),
        &current_op.app,
        &current_op.action_name,
    );

    // Seed active_progress from any already-committed log entries. For a fresh
    // operation this is empty; Some(empty) signals the reconciler that an
    // operation is in progress and it should not fall back to steady state.
    {
        let entries = log.load();
        *active_progress.write() = Some(OperationProgress::from_log(&entries));
    }

    // run_operation is synchronous and uses Rhai types that are not Send;
    // block_in_place runs it on the current thread without moving anything.
    let result = tokio::task::block_in_place(|| {
        run_operation(
            OperationContext {
                engine: &engine,
                script_ast: &ast,
                operation_id: current_op.operation_id.clone(),
                app: &app,
                action_name: &current_op.action_name,
                log: &log,
                world: oracle,
                registry: Arc::clone(&registry),
                active_progress: Some(Arc::clone(&active_progress)),
                tick_notify: Some(Arc::clone(&tick_notify)),
            },
            &mut scope,
        )
    });

    // Operation finished — return to steady state and trigger an immediate tick.
    *active_progress.write() = None;
    tick_notify.notify_one();

    match result {
        OperationResult::Completed => {
            eprintln!("completed.");
            clear_current_operation(&db).unwrap_or_else(|e| {
                eprintln!("warning: failed to clear current operation record: {e}");
            });
            scheduler.complete_current();
        }
        OperationResult::Suspended(cond) => {
            let names: Vec<_> = cond
                .resources
                .iter()
                .map(|r| r.name.as_deref().unwrap_or("<anonymous>"))
                .collect();
            eprintln!(
                "suspended — waiting for {names:?} to reach {:?} (deadline {}s)",
                cond.required_state, cond.deadline_secs,
            );
            eprintln!("operation state saved; run again to resume.");
        }
        OperationResult::Failed(err) => {
            eprintln!("operation failed: {err}");
            clear_current_operation(&db).unwrap_or_else(|e| {
                eprintln!("warning: failed to clear current operation record: {e}");
            });
            std::process::exit(1);
        }
    }

    // Keep the reconciler alive after the operation so it maintains steady
    // state without requiring a restart. Ctrl-C for clean exit.
    eprintln!("reconciler running in steady state. Ctrl-C to exit.");
    tokio::signal::ctrl_c().await.ok();
}
