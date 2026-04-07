use std::path::{Path, PathBuf};
use std::sync::Arc;

use rhai::{AST, Engine, Scope};
use seedling::defs::app::App;
use seedling::runtime::{
    barrier::{
        OperationId,
        oracle::DbWorldOracle,
        replay::{DbActionLog, OperationResult, run_operation},
    },
    db::Db,
    history::{
        CurrentOperation, clear_current_operation, load_current_operation, save_current_operation,
    },
    registry::DbInstanceRegistry,
    scheduler::{RejectReason, ScheduleResult, Scheduler},
};
use seedling::setup_language;

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
            let has_start = app.0.lock().actions.contains_key("start");
            if !has_start {
                eprintln!("no interrupted operation and no 'start' action — nothing to do");
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

fn main() {
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

    app.0.lock().name = app_name.clone();

    {
        let def = app.0.lock();
        eprintln!("app: {app_name}");
        eprintln!("  resources: {}", def.resources.len());
        for id in def.resources.keys() {
            eprintln!("    {:?} {:?}", id.kind, id.name);
        }
        eprintln!("  actions: {:?}", def.actions.keys().collect::<Vec<_>>());
    }

    let Some(current_op) = find_or_create_operation(&db, &app, &app_name) else {
        return;
    };

    // Register with the scheduler for the single-active-operation invariant.
    // We use the persisted operation_id for the action log and run_operation;
    // the scheduler's internally-generated id is used only for concurrency tracking.
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

    let registry = Arc::new(DbInstanceRegistry::new(Db::open(&db_path).unwrap_or_else(
        |e| {
            eprintln!("error: cannot open registry database: {e}");
            std::process::exit(1);
        },
    )));

    match run_operation(
        &engine,
        &mut scope,
        &ast,
        current_op.operation_id.clone(),
        &app,
        &current_op.action_name,
        &log,
        Arc::clone(&oracle),
        registry,
    ) {
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
}
