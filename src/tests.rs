use rhai::Dynamic;
use rhai::{AST, Engine, Scope};

use crate::defs::install::InstallDef;
use crate::runtime::barrier::runtime::{ActionClosureGuard, RuntimeInstance, shell_attach_fn_ptr};
use crate::{defs, setup_language as setup};

mod action;
mod app;
mod barrier;
mod bsl;
mod collection;
mod constants;
mod container;
mod deployment;
mod ingress;
mod job;
mod param;
mod pod;
mod runtime;
mod service;
mod volume;

pub fn run_test_script(source: &str) -> (Engine, Scope<'static>, defs::app::App, AST) {
    let (engine, mut scope, app) = setup();
    let ast = run_script(&engine, &mut scope, source).expect("script should run without error");
    (engine, scope, app, ast)
}

pub fn run_test_script_app(source: &str) -> defs::app::App {
    let (_, _, app, _) = run_test_script(source);
    app
}

pub fn run_test_script_err(source: &str) -> Box<rhai::EvalAltResult> {
    let (engine, mut scope, _app) = setup();
    run_script(&engine, &mut scope, source).expect_err("script should fail")
}

pub fn exercise(source: &str) {
    let (engine, mut scope, app, ast) = run_test_script(source);
    exercise_actions(&engine, &mut scope, &app, &ast);
}

fn exercise_actions(engine: &Engine, scope: &mut Scope, app: &defs::app::App, script_ast: &AST) {
    let rt = RuntimeInstance::stub();
    let attach = shell_attach_fn_ptr();

    // Re-run the script with the TLS capture active to recover FnPtrs,
    // exactly as run_operation does. FnPtrs are never stored persistently.
    let (actions, shells, install, param_changes) = {
        let (mut fresh_scope, fresh_app) = defs::scope();
        fresh_app.def.lock().name = app.def.lock().name.clone();
        defs::app::begin_closure_capture();
        engine
            .run_ast_with_scope(&mut fresh_scope, script_ast)
            .expect("re-run for exercise should succeed");
        let captured = defs::app::end_closure_capture();

        let def = fresh_app.def.lock();
        let actions: Vec<_> = captured.actions.into_iter().collect();
        let shells: Vec<_> = captured.shells.into_iter().collect();
        let install = def
            .install
            .as_ref()
            .and_then(|i| captured.install.map(|c| (c, build_install_reqs_map(i))));
        let param_changes: Vec<_> = captured.param_changes.into_iter().collect();

        (actions, shells, install, param_changes)
    };

    for (name, closure) in &actions {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());

        let call_script = "__bsl_closure.call(__bsl_rt)";

        println!("exercising action: {name}");
        let result = {
            let _guard = ActionClosureGuard::new(
                std::sync::Arc::new(parking_lot::Mutex::new(crate::defs::app::AppDef::default())),
                String::new(),
            );
            eval_merged(engine, scope, script_ast, call_script)
        };
        match result {
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
        let result_two = {
            let _guard = ActionClosureGuard::new(
                std::sync::Arc::new(parking_lot::Mutex::new(crate::defs::app::AppDef::default())),
                String::new(),
            );
            eval_merged(engine, scope, script_ast, two_arg)
        };
        match result_two {
            Ok(_) => println!("  ok (two-arg)"),
            Err(err_two) => {
                let result_one = {
                    let _guard = ActionClosureGuard::new(
                        std::sync::Arc::new(parking_lot::Mutex::new(
                            crate::defs::app::AppDef::default(),
                        )),
                        String::new(),
                    );
                    eval_merged(engine, scope, script_ast, one_arg)
                };
                match result_one {
                    Ok(_) => println!("  ok (one-arg)"),
                    Err(err_one) => {
                        println!("  error (two-arg): {err_two}");
                        println!("  error (one-arg): {err_one}");
                    }
                }
            }
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
        let result = {
            let _guard = ActionClosureGuard::new(
                std::sync::Arc::new(parking_lot::Mutex::new(crate::defs::app::AppDef::default())),
                String::new(),
            );
            eval_merged(engine, scope, script_ast, call_script)
        };
        match result {
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
            let result = {
                let _guard = ActionClosureGuard::new(
                    std::sync::Arc::new(parking_lot::Mutex::new(
                        crate::defs::app::AppDef::default(),
                    )),
                    String::new(),
                );
                eval_merged(engine, scope, script_ast, call_script)
            };
            match result {
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
pub fn run_script(
    engine: &Engine,
    scope: &mut Scope,
    source: &str,
) -> Result<AST, Box<rhai::EvalAltResult>> {
    let ast = engine.compile(source)?;
    engine.run_ast_with_scope(scope, &ast)?;
    Ok(ast)
}
