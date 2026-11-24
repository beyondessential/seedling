//! The helpers of the `std` module.

use rhai::{EvalAltResult, Position};

mod fields;
pub(crate) mod kinds;
mod random;

// spec: STD#the-std-module
/// The `std` module, holding every helper BSL offers.
pub fn module() -> rhai::Module {
    let mut module = rhai::Module::new();
    kinds::register(&mut module);
    random::register(&mut module);
    module
}

/// A value beyond the engine's limit for it, which a script cannot catch.
fn too_large(what: &str) -> Box<EvalAltResult> {
    EvalAltResult::ErrorDataTooLarge(what.into(), Position::NONE).into()
}
