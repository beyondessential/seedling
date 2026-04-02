use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
use rhai::Dynamic;
use rhai::{AST, Engine, Scope};

use crate::defs::app::App;
#[cfg(test)]
use crate::defs::install::InstallDef;
use crate::runtime::barrier::OperationId;
use crate::runtime::barrier::oracle::DbWorldOracle;
use crate::runtime::barrier::replay::{DbActionLog, OperationResult, run_operation};
use crate::runtime::db::Db;
use crate::runtime::history::{
    CurrentOperation, clear_current_operation, load_current_operation, save_current_operation,
};
use crate::runtime::scheduler::{RejectReason, ScheduleResult, Scheduler};

pub(crate) mod defs;
pub(crate) mod runtime;

#[cfg(test)]
mod tests;

fn setup() -> (Engine, Scope<'static>, defs::app::App) {
    let mut engine = Engine::new();
    defs::register(&mut engine);
    let (scope, app) = defs::scope();
    (engine, scope, app)
}

#[cfg(test)]
fn exercise_actions(engine: &Engine, scope: &mut Scope, app: &defs::app::App, script_ast: &AST) {
    let def = app.0.lock();

    let rt = runtime::barrier::runtime::RuntimeInstance::stub();
    let attach = runtime::barrier::runtime::shell_attach_fn_ptr();

    let actions: Vec<_> = def
        .actions
        .iter()
        .map(|(name, a)| (name.clone(), a.closure.clone()))
        .collect();
    let shells: Vec<_> = def
        .shells
        .iter()
        .map(|(name, s)| (name.clone(), s.closure.clone()))
        .collect();
    let install = def.install.as_ref().map(|i| {
        let reqs_map = build_install_reqs_map(i);
        (i.closure.clone(), reqs_map)
    });
    let param_changes: Vec<_> = def
        .param_changes
        .iter()
        .map(|(name, closure)| (name.clone(), closure.clone()))
        .collect();

    drop(def);

    for (name, closure) in &actions {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());

        let call_script = "__bsl_closure.call(__bsl_rt)";

        println!("exercising action: {name}");
        match eval_merged(engine, scope, script_ast, call_script) {
            Ok(_) => println!("  ok"),
            Err(err) => println!("  error: {err}"),
        }

        let _ = scope.remove::<Dynamic>("__bsl_rt");
        let _ = scope.remove::<Dynamic>("__bsl_closure");
    }

    for (name, closure) in &shells {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());
        scope.push("__bsl_attach", attach.clone());

        println!("exercising shell: {name}");
        let two_arg = "__bsl_closure.call(__bsl_rt, __bsl_attach)";
        let one_arg = "__bsl_closure.call(__bsl_rt)";
        match eval_merged(engine, scope, script_ast, two_arg) {
            Ok(_) => println!("  ok (two-arg)"),
            Err(err_two) => match eval_merged(engine, scope, script_ast, one_arg) {
                Ok(_) => println!("  ok (one-arg)"),
                Err(err_one) => {
                    println!("  error (two-arg): {err_two}");
                    println!("  error (one-arg): {err_one}");
                }
            },
        }

        let _ = scope.remove::<Dynamic>("__bsl_rt");
        let _ = scope.remove::<Dynamic>("__bsl_closure");
        let _ = scope.remove::<Dynamic>("__bsl_attach");
    }

    if let Some((closure, reqs_map)) = &install {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());
        scope.push("__bsl_reqs", reqs_map.clone());

        println!("exercising install");
        let call_script = "__bsl_closure.call(__bsl_rt, __bsl_reqs)";
        match eval_merged(engine, scope, script_ast, call_script) {
            Ok(_) => println!("  ok"),
            Err(err) => println!("  error: {err}"),
        }

        let _ = scope.remove::<Dynamic>("__bsl_rt");
        let _ = scope.remove::<Dynamic>("__bsl_closure");
        let _ = scope.remove::<Dynamic>("__bsl_reqs");
    }

    if !param_changes.is_empty() {
        let old_app = defs::app::App::default();
        for (name, closure) in &param_changes {
            scope.push("__bsl_rt", rt.clone());
            scope.push("__bsl_closure", closure.clone());
            scope.push("__bsl_old_app", old_app.clone());

            println!("exercising param change: {name}");
            let call_script = "__bsl_closure.call(__bsl_rt, __bsl_old_app)";
            match eval_merged(engine, scope, script_ast, call_script) {
                Ok(_) => println!("  ok"),
                Err(err) => println!("  error: {err}"),
            }

            let _ = scope.remove::<Dynamic>("__bsl_rt");
            let _ = scope.remove::<Dynamic>("__bsl_closure");
            let _ = scope.remove::<Dynamic>("__bsl_old_app");
        }
    }
}

#[cfg(test)]
fn eval_merged(
    engine: &Engine,
    scope: &mut Scope,
    script_ast: &AST,
    call_source: &str,
) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
    let call_ast = engine.compile(call_source)?;
    let merged = script_ast.merge(&call_ast);
    engine.eval_ast_with_scope(scope, &merged)
}

#[cfg(test)]
fn build_install_reqs_map(install: &InstallDef) -> rhai::Map {
    let mut map = rhai::Map::new();
    for (key, req) in &install.requirements {
        let value = req
            .default_value
            .clone()
            .unwrap_or_else(|| "<placeholder>".into());
        map.insert(key.as_str().into(), Dynamic::from(value));
    }
    map
}

#[cfg(test)]
fn run_script(
    engine: &Engine,
    scope: &mut Scope,
    source: &str,
) -> Result<AST, Box<rhai::EvalAltResult>> {
    let ast = engine.compile(source)?;
    engine.run_ast_with_scope(scope, &ast)?;
    Ok(ast)
}

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

    let (engine, mut scope, app) = setup();
    let ast = run_file(&engine, &mut scope, script_path.clone()).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });

    let app_name = script_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("app")
        .to_owned();

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

    match run_operation(
        &engine,
        &mut scope,
        &ast,
        current_op.operation_id.clone(),
        &app,
        &current_op.action_name,
        &log,
        Arc::clone(&oracle),
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
