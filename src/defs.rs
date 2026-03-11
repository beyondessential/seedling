use std::sync::Arc;

use parking_lot::Mutex;
use rhai::{Engine, Scope};

type Holder<T> = Arc<Mutex<T>>;

pub mod action;
pub mod app;
pub mod collection;
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

// l[impl bsl.syntax]
// l[impl bsl.builder]
// l[impl bsl.errors]
// l[impl bsl.name]
// l[impl bsl.resource]
// l[impl bsl.script]
// l[impl bsl.placeholder]
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
    engine.build_type::<collection::Collection>();
    runtime::register_shell_attach(engine);
}

// l[impl bsl.scope]
// l[impl bsl.enums]
pub fn scope() -> (Scope<'static>, app::App) {
    let mut scope = Scope::new();

    // l[impl const.available-threads]
    scope.push_constant("AVAILABLE_THREADS", 16_i64);

    // l[impl const.default-deadline]
    scope.push_constant("DEFAULT_DEADLINE", 30_i64);

    // l[impl const.on-update.rolling]
    // l[impl const.on-update.replace]
    scope.push_constant("OnUpdate", enums::OnUpdate::rhai_constant());

    // l[impl const.on-terminate.recreate]
    scope.push_constant("OnTerminate", enums::OnTerminate::rhai_constant());

    // l[impl const.on-exit.restart]
    // l[impl const.on-exit.terminate]
    // l[impl const.on-exit.restart-on-failure]
    scope.push_constant("OnExit", enums::OnExit::rhai_constant());

    // l[impl const.resource-type.enum]
    scope.push_constant("ResourceType", resource::ResourceKind::rhai_constant());

    let app = app::App::default();
    scope.push("app", app.clone());
    (scope, app)
}
