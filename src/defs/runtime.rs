// Re-export the real RuntimeInstance, Started, Termination, and shell helpers
// from the barrier engine.  The language-layer registration in src/defs.rs
// continues to work unchanged; the barrier tests inject a live SharedContext
// via RuntimeInstance::with_context(), while all existing language tests use
// RuntimeInstance::stub() (ctx = None), which preserves the previous behaviour.
pub use crate::runtime::barrier::runtime::{
    RuntimeInstance, Started, Termination, register_shell_attach, shell_attach_fn_ptr,
};
