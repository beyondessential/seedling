use std::path::PathBuf;

use rhai::{AST, Dynamic, Engine, Scope};

mod defs;
use defs::install::InstallDef;

#[cfg(test)]
mod tests;

fn setup() -> (Engine, Scope<'static>, defs::app::App) {
    let mut engine = Engine::new();
    defs::register(&mut engine);
    let (scope, app) = defs::scope();
    (engine, scope, app)
}

fn exercise_actions(engine: &Engine, scope: &mut Scope, app: &defs::app::App, script_ast: &AST) {
    let def = app.0.lock();

    let rt = defs::runtime::RuntimeInstance;
    let attach = defs::runtime::shell_attach_fn_ptr();
    let history = defs::history::History;
    let old_app = defs::app::App::default();

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

    drop(def);

    for (name, closure) in &actions {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());
        scope.push("__bsl_old_app", old_app.clone());
        scope.push("__bsl_history", history.clone());

        let call_script = match name.as_str() {
            "upgrade" => "__bsl_closure.call(__bsl_rt, __bsl_old_app)",
            "crash_recovery" => "__bsl_closure.call(__bsl_rt, __bsl_history)",
            _ => "__bsl_closure.call(__bsl_rt)",
        };

        println!("exercising action: {name}");
        match eval_merged(engine, scope, script_ast, call_script) {
            Ok(_) => println!("  ok"),
            Err(err) => println!("  error: {err}"),
        }

        let _ = scope.remove::<Dynamic>("__bsl_rt");
        let _ = scope.remove::<Dynamic>("__bsl_closure");
        let _ = scope.remove::<Dynamic>("__bsl_old_app");
        let _ = scope.remove::<Dynamic>("__bsl_history");
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
}

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

fn main() {
    let filepath = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("Usage: beset <RHAI FILE>"),
    );

    let (engine, mut scope, app) = setup();

    let ast = match run_file(&engine, &mut scope, filepath) {
        Ok(ast) => ast,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };

    let def = app.0.lock();
    println!("params: {:?}", def.params.keys().collect::<Vec<_>>());
    println!("resources: {}", def.resources.len());
    for id in def.resources.keys() {
        println!("  {:?} {:?}", id.kind, id.name);
    }
    println!("actions: {:?}", def.actions.keys().collect::<Vec<_>>());
    println!("shells: {:?}", def.shells.keys().collect::<Vec<_>>());
    println!("install: {}", def.install.is_some());
    drop(def);

    println!();
    println!("--- exercising actions ---");
    exercise_actions(&engine, &mut scope, &app, &ast);
}
