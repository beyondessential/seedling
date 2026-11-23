//! The helpers of the `std` module.

pub(crate) mod kinds;
mod random;
pub(crate) mod version;

// spec: STD#the-std-module
/// The `std` module, holding every helper BSL offers.
pub fn module() -> rhai::Module {
    let mut module = rhai::Module::new();
    kinds::register(&mut module);
    random::register(&mut module);
    module
}
