use rhai::{Engine, Scope};

pub mod defs;
pub mod oi;
pub mod runtime;
pub mod system;

#[cfg(test)]
mod tests;

/// Configurable limits for the Rhai script engine.
// r[impl engine.limits]
#[derive(Debug, Clone)]
pub struct ScriptLimits {
    // r[impl engine.limits.operations]
    pub max_operations: u64,
    // r[impl engine.limits.call-depth]
    pub max_call_levels: usize,
    // r[impl engine.limits.expr-depth]
    pub max_expr_depth: usize,
    // r[impl engine.limits.string-size]
    pub max_string_size: usize,
    // r[impl engine.limits.array-size]
    pub max_array_size: usize,
    // r[impl engine.limits.map-size]
    pub max_map_size: usize,
}

impl Default for ScriptLimits {
    fn default() -> Self {
        Self {
            max_operations: 100_000,
            max_call_levels: 64,
            max_expr_depth: 64,
            max_string_size: 1_048_576,
            max_array_size: 10_000,
            max_map_size: 10_000,
        }
    }
}

pub fn setup_language(limits: &ScriptLimits) -> (Engine, Scope<'static>, defs::app::App) {
    let mut engine = Engine::new();

    engine.set_max_operations(limits.max_operations);
    engine.set_max_call_levels(limits.max_call_levels);
    engine.set_max_expr_depths(limits.max_expr_depth, limits.max_expr_depth);
    engine.set_max_string_size(limits.max_string_size);
    engine.set_max_array_size(limits.max_array_size);
    engine.set_max_map_size(limits.max_map_size);

    defs::register(&mut engine);
    let (scope, app) = defs::scope();
    (engine, scope, app)
}
