use rhai::{AST, Engine, Scope};

use crate::{defs, exercise_actions, run_script, setup};

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

mod bsl;
mod app;
mod constants;
mod service;
mod ingress;
mod container;
mod deployment;
mod job;
mod pod;
mod volume;
