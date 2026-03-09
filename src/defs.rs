use std::sync::Arc;

use parking_lot::Mutex;
use rhai::{Engine, Scope};

type Holder<T> = Arc<Mutex<T>>;

pub mod action;
pub mod app;
pub mod container;
pub mod deployment;
pub mod ingress;
pub mod install;
pub mod job;
pub mod pod;
pub mod resource;
pub mod service;
pub mod volume;

// r[impl lang.syntax]
pub fn register(engine: &mut Engine) {
    engine.build_type::<app::App>();
    engine.build_type::<service::Service>();
    engine.build_type::<service::HttpService>();
    engine.build_type::<service::PartialRoute>();
    engine.build_type::<service::ServicePort>();
    engine.build_type::<ingress::Ingress>();
    engine.build_type::<deployment::Deployment>();
    engine.build_type::<volume::Volume>();
    dbg!(engine.gen_fn_signatures(false));
}

pub fn scope() -> (Scope<'static>, app::App) {
    let mut scope = Scope::new();
    scope.push_constant("AVAILABLE_THREADS", 16_i64); // TODO
    scope.push_constant(
        "DeploymentStrategy",
        deployment::DeploymentStrategy::rhai_constant(),
    );

    let app = app::App::default();
    scope.push("app", app.clone());
    (scope, app)
}
