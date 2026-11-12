use std::sync::Arc;

use rhai::Dynamic;
use rhai::{AST, Engine, Scope};

use crate::defs;
use crate::defs::install::InstallDef;
use crate::runtime::OperationProgress;
use crate::runtime::barrier::runtime::{ActionClosureGuard, RuntimeInstance};
use crate::runtime::barrier::shell::ShellControl;
use crate::runtime::barrier::{
    OperationId,
    replay::{InMemoryActionLog, OperationContext, OperationResult, run_operation},
};

/// An engine, scope and app evaluating no particular definition.
pub fn setup(limits: &crate::ScriptLimits) -> (Engine, Scope<'static>, defs::app::App) {
    crate::setup_language(limits, Default::default())
}

mod action;
mod action_call;
mod app;
mod barrier;
mod bsl;
mod bundle;
mod collection;
mod constants;
mod container;
mod deployment;
mod description;
mod exec;
mod ingress;
mod job;
mod param;
mod pod;
mod runtime;
mod service;
mod volume;

pub fn run_test_script(source: &str) -> (Engine, Scope<'static>, defs::app::App, AST) {
    let (engine, mut scope, app) = setup(&crate::ScriptLimits::default());
    let ast = run_script(&engine, &mut scope, source).expect("script should run without error");
    (engine, scope, app, ast)
}

/// Run `source` as the script of a bundle that also holds `files`.
pub fn run_test_script_in(
    source: &str,
    files: &[(&str, &[u8])],
) -> Result<(Engine, Scope<'static>, defs::app::App, AST), Box<rhai::EvalAltResult>> {
    let mut all: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|(p, c)| ((*p).to_owned(), c.to_vec()))
        .collect();
    all.push(("app.seed.rhai".to_owned(), source.as_bytes().to_vec()));
    let bundle = std::sync::Arc::new(
        crate::runtime::definition::Bundle::from_files(all).expect("a valid test bundle"),
    );
    let (engine, mut scope, app) = crate::setup_language(&crate::ScriptLimits::default(), bundle);
    let ast = run_script(&engine, &mut scope, source)?;
    Ok((engine, scope, app, ast))
}

pub fn run_test_script_app(source: &str) -> defs::app::App {
    let (_, _, app, _) = run_test_script(source);
    app
}

pub fn run_test_script_err(source: &str) -> Box<rhai::EvalAltResult> {
    let (engine, mut scope, _app) = setup(&crate::ScriptLimits::default());
    run_script(&engine, &mut scope, source).expect_err("script should fail")
}

/// The hooks and operation-scoped state one action run differs by.
///
/// Everything a test does not name is the language-only default: no executor,
/// no volume writer, no signaller, no volume bindings, no progress sink.
#[derive(Default)]
pub struct ActionHooks {
    pub active_progress: Option<Arc<parking_lot::RwLock<Option<OperationProgress>>>>,
    pub operation_volume_bindings:
        std::collections::HashMap<String, crate::defs::volume::OperationVolumeBinding>,
    pub container_signaler: Option<Arc<dyn crate::runtime::barrier::ContainerSignaler>>,
    pub volume_writer: Option<Arc<dyn crate::runtime::barrier::VolumeWriter>>,
    pub executor: Option<Arc<dyn crate::runtime::barrier::Executor>>,
}

/// An [`OperationContext`] for `action_name`, carrying `hooks` over the
/// defaults every language test wants.
///
/// The single home for this field plumbing: a field added to
/// `OperationContext` is given its test default here once, rather than in each
/// test that builds one.
pub fn action_context<'a, W: crate::runtime::WorldStateOracle + 'static>(
    engine: &'a Engine,
    script_ast: &'a AST,
    app: &'a defs::app::App,
    action_name: &'a str,
    log: &'a dyn crate::runtime::barrier::replay::ActionLog,
    world: Arc<W>,
    hooks: ActionHooks,
) -> OperationContext<'a, W> {
    OperationContext {
        engine,
        script_ast,
        app,
        action_name,
        log,
        world,
        operation_id: OperationId::new(),
        registry: Arc::new(crate::runtime::EphemeralInstanceRegistry::new()),
        active_progress: hooks.active_progress,
        tick_notify: None,
        params: serde_json::Map::new(),
        is_shell: false,
        db: None,
        source_generation: 0,
        target_generation: 0,
        script_limits: None,
        cipher: None,
        operation_volume_bindings: hooks.operation_volume_bindings,
        cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
        container_signaler: hooks.container_signaler,
        volume_writer: hooks.volume_writer,
        executor: hooks.executor,
    }
}

/// Run the action `action_name` defined by `source` to its end and report how
/// it ended, so a test can assert on the error an action closure raised.
pub fn run_action(source: &str, action_name: &str) -> OperationResult {
    run_action_with(source, &[], action_name, None, ActionHooks::default())
}

/// The error message of the action `action_name`, which must fail.
pub fn run_action_err(source: &str, action_name: &str) -> String {
    match run_action(source, action_name) {
        OperationResult::Failed(e) => e.to_string(),
        other => panic!("expected the action to fail, got {other:?}"),
    }
}

/// Run `action_name` from a script bundled with `files`, under `hooks`.
///
/// Pass a `log` to assert on what the run recorded; without one the run gets a
/// log of its own that nothing reads.
pub fn run_action_with(
    source: &str,
    files: &[(&str, &[u8])],
    action_name: &str,
    log: Option<&InMemoryActionLog>,
    hooks: ActionHooks,
) -> OperationResult {
    run_action_on(source, files, action_name, log, hooks).0
}

/// As [`run_action_with`], also handing back the `App` the action ran against,
/// for the tests that assert on what the run left in it.
pub fn run_action_on(
    source: &str,
    files: &[(&str, &[u8])],
    action_name: &str,
    log: Option<&InMemoryActionLog>,
    hooks: ActionHooks,
) -> (OperationResult, defs::app::App) {
    let (engine, mut scope, app, ast) =
        run_test_script_in(source, files).expect("script should run without error");
    let own_log;
    let log = match log {
        Some(log) => log,
        None => {
            own_log = InMemoryActionLog::new();
            &own_log
        }
    };
    let result = run_operation(
        action_context(
            &engine,
            &ast,
            &app,
            action_name,
            log,
            Arc::new(crate::runtime::TestWorldOracle::new()),
            hooks,
        ),
        &mut scope,
    );
    (result, app)
}

pub fn exercise(source: &str) {
    let (engine, mut scope, app, ast) = run_test_script(source);
    exercise_actions(&engine, &mut scope, &app, &ast);
}

fn exercise_actions(engine: &Engine, scope: &mut Scope, app: &defs::app::App, script_ast: &AST) {
    let rt = RuntimeInstance::stub();

    // Re-run the script with the TLS capture active to recover FnPtrs,
    // exactly as run_operation does. FnPtrs are never stored persistently.
    let (actions, shells, install, param_changes) = {
        let (mut fresh_scope, fresh_app) = defs::scope(Default::default());
        fresh_app.def.rcu(|old| {
            let mut new_def = (**old).clone();
            new_def.name = app.def.load().name.clone();
            new_def
        });
        defs::app::begin_closure_capture();
        engine
            .run_ast_with_scope(&mut fresh_scope, script_ast)
            .expect("re-run for exercise should succeed");
        let captured = defs::app::end_closure_capture();

        let def = fresh_app.def.load();
        let actions: Vec<_> = captured.actions.into_iter().collect();
        let shells: Vec<_> = captured.shells.into_iter().collect();
        let install = def
            .install
            .as_ref()
            .and_then(|i| captured.install.map(|c| (c, build_install_reqs_map(i))));
        let param_changes: Vec<_> = captured.param_changes.into_iter().collect();

        (actions, shells, install, param_changes)
    };

    let empty_param = rhai::Map::new();

    for (name, closure) in &actions {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());
        scope.push("__bsl_param", empty_param.clone());

        println!("exercising action: {name}");
        let result = {
            let _guard = ActionClosureGuard::new(
                std::sync::Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                    crate::defs::app::AppDef::default(),
                ))),
                String::new(),
                std::collections::HashMap::new(),
            );
            eval_merged(
                engine,
                scope,
                script_ast,
                "__bsl_closure.call(__bsl_rt, __bsl_param)",
            )
        };
        match result {
            Ok(_) => println!("  ok"),
            Err(err) => println!("  error: {err}"),
        }

        let _ = scope.remove::<Dynamic>("__bsl_rt");
        let _ = scope.remove::<Dynamic>("__bsl_closure");
        let _ = scope.remove::<rhai::Map>("__bsl_param");
    }

    for (name, closure) in &shells {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());
        scope.push("__bsl_shell", ShellControl::new());
        scope.push("__bsl_param", empty_param.clone());

        println!("exercising shell: {name}");
        let result = {
            let _guard = ActionClosureGuard::new(
                std::sync::Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                    crate::defs::app::AppDef::default(),
                ))),
                String::new(),
                std::collections::HashMap::new(),
            );
            eval_merged(
                engine,
                scope,
                script_ast,
                "__bsl_closure.call(__bsl_rt, __bsl_shell, __bsl_param)",
            )
        };
        match result {
            Ok(_) => println!("  ok"),
            Err(err) => println!("  error: {err}"),
        }

        let _ = scope.remove::<Dynamic>("__bsl_rt");
        let _ = scope.remove::<Dynamic>("__bsl_closure");
        let _ = scope.remove::<ShellControl>("__bsl_shell");
        let _ = scope.remove::<rhai::Map>("__bsl_param");
    }

    if let Some((closure, reqs_map)) = &install {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());
        scope.push("__bsl_param", reqs_map.clone());

        println!("exercising install");
        let result = {
            let _guard = ActionClosureGuard::new(
                std::sync::Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                    crate::defs::app::AppDef::default(),
                ))),
                String::new(),
                std::collections::HashMap::new(),
            );
            eval_merged(
                engine,
                scope,
                script_ast,
                "__bsl_closure.call(__bsl_rt, __bsl_param)",
            )
        };
        match result {
            Ok(_) => println!("  ok"),
            Err(err) => println!("  error: {err}"),
        }

        let _ = scope.remove::<Dynamic>("__bsl_rt");
        let _ = scope.remove::<Dynamic>("__bsl_closure");
        let _ = scope.remove::<rhai::Map>("__bsl_param");
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
                    std::sync::Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                        crate::defs::app::AppDef::default(),
                    ))),
                    String::new(),
                    std::collections::HashMap::new(),
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
    // The script's functions only: its statements already ran, as in replay.
    let merged = script_ast.clone_functions_only().merge(&call_ast);
    engine.eval_ast_with_scope(scope, &merged)
}

#[cfg(test)]
fn build_install_reqs_map(install: &InstallDef) -> rhai::Map {
    let mut map = rhai::Map::new();
    for (key, req) in &install.requirements {
        map.insert(
            key.as_str().into(),
            Dynamic::from(req.default_value.clone()),
        );
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
