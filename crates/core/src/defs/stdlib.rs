//! The helpers of the `std` module.

mod kinds;

// spec: STD#the-std-module
/// The `std` module, holding every helper BSL offers.
pub fn module() -> rhai::Module {
    let mut module = rhai::Module::new();
    kinds::register(&mut module);
    module
}
