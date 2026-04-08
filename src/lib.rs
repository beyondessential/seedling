use rhai::{Engine, Scope};

pub mod defs;
pub mod oi;
pub mod runtime;
pub mod system;

#[cfg(test)]
mod tests;

pub fn setup_language() -> (Engine, Scope<'static>, defs::app::App) {
    let mut engine = Engine::new();
    defs::register(&mut engine);
    let (scope, app) = defs::scope();
    (engine, scope, app)
}
