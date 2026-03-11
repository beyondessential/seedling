use std::sync::Arc;

use parking_lot::Mutex;
use rhai::{Engine, Scope};

type Holder<T> = Arc<Mutex<T>>;

pub mod action;
pub mod app;
pub mod container;
pub mod deployment;
pub mod enums;
pub mod history;
pub mod ingress;
pub mod install;
pub mod job;
pub mod pod;
pub mod resource;
pub mod runtime;
pub mod service;
pub mod volume;

// r[bsl.syntax]
pub fn register(engine: &mut Engine) {
    engine.build_type::<app::App>();
    engine.build_type::<service::Service>();
    engine.build_type::<service::HttpService>();
    engine.build_type::<service::HttpServiceRoute>();
    engine.build_type::<service::ServicePort>();
    engine.build_type::<service::ExternalService>();
    engine.build_type::<ingress::Ingress>();
    engine.build_type::<action::Action>();
    engine.build_type::<deployment::Deployment>();
    engine.build_type::<job::Job>();
    engine.build_type::<volume::Volume>();
    engine.build_type::<volume::ExternalVolume>();
    engine.build_type::<runtime::RuntimeInstance>();
    engine.build_type::<runtime::Started>();
    engine.build_type::<runtime::Termination>();
    engine.build_type::<history::History>();
}

// r[bsl.scope]
// r[bsl.enums]
pub fn scope() -> (Scope<'static>, app::App) {
    let mut scope = Scope::new();

    // r[const.available-threads]
    scope.push_constant("AVAILABLE_THREADS", 16_i64);

    // r[const.default-deadline]
    scope.push_constant("DEFAULT_DEADLINE", 30_i64);

    // r[const.on-update.rolling]
    // r[const.on-update.replace]
    scope.push_constant("OnUpdate", enums::OnUpdate::rhai_constant());

    // r[const.on-terminate.recreate]
    scope.push_constant("OnTerminate", enums::OnTerminate::rhai_constant());

    // r[const.on-exit.restart]
    // r[const.on-exit.terminate]
    // r[const.on-exit.restart-on-failure]
    scope.push_constant("OnExit", enums::OnExit::rhai_constant());

    // r[const.resource-type.enum]
    scope.push_constant("ResourceType", resource::ResourceKind::rhai_constant());

    let app = app::App::default();
    scope.push("app", app.clone());
    (scope, app)
}
