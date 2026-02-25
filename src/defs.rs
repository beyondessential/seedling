use std::sync::Arc;

use parking_lot::Mutex;
use rhai::Engine;

type Holder<T> = Arc<Mutex<T>>;

pub mod action;
pub mod app;
pub mod deployment;
pub mod ingress;
pub mod install;
pub mod resource;
pub mod service;
pub mod volume;

pub fn register(engine: &mut Engine) {
    engine.build_type::<app::App>();
    engine.build_type::<service::Service>();
    engine.build_type::<service::HttpService>();
    engine.build_type::<service::PartialRoute>();
    engine.build_type::<service::ServicePort>();
    engine.build_type::<ingress::Ingress>();
    engine.build_type::<deployment::Deployment>();
    engine.build_type::<volume::Volume>();
    engine.register_fn("__app", || app::App::default());
    dbg!(engine.gen_fn_signatures(false));
}
