//! The helpers of the `std` module.

use rhai::{EvalAltResult, NativeCallContext, Position};

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

/// The limits the `std` helpers hold an evaluation to, which the engine has
/// none of its own for, so each evaluation of an engine starts with them as
/// its tag.
// spec: LIM#limits
#[derive(Clone, Copy, Debug)]
pub struct HelperLimits {
    pub derived_values: usize,
    pub password_hashes: usize,
}

/// The limits the calling evaluation is held to.
///
/// The tag is the runtime's, never the script's: an engine that does not
/// carry the limits was not built by `setup_language`, and its helpers derive
/// nothing rather than deriving without them.
fn helper_limits(ctx: &NativeCallContext) -> Result<HelperLimits, Box<EvalAltResult>> {
    ctx.tag()
        .and_then(|tag| tag.read_lock::<HelperLimits>().map(|l| *l))
        .ok_or_else(|| "this evaluation carries no limits for its helpers, so it derives nothing".into())
}

/// A value beyond the engine's limit for it, which a script cannot catch.
fn too_large(what: &str) -> Box<EvalAltResult> {
    EvalAltResult::ErrorDataTooLarge(what.into(), Position::NONE).into()
}
