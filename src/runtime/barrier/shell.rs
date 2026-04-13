use std::{cell::RefCell, sync::Arc};

use parking_lot::Mutex;

/// The exec target resolved by `__bsl_shell_attach_impl` from a Job.
/// Passed back to the OI layer which translates it to a `ContainerSpec`
/// via the standard `job_spec` pipeline and runs `podman run --rm -it`.
pub struct ShellExecTarget {
    pub job_def: crate::defs::job::JobDef,
    /// BSL-level name of the Job (from `app.job("name")`), used to derive
    /// the container display name.
    pub job_name: String,
    pub app_name: String,
    // r[impl identity.job.shell]
    /// Fresh randomly-generated instance ID chosen at `attach()` call time.
    /// Each shell session gets a distinct ID so concurrent sessions against
    /// the same Job definition produce distinct container names.
    pub instance_id: crate::runtime::identity::InstanceId,
}

/// Context installed in the thread-local before running a shell closure.
/// Provides a slot for `__bsl_shell_attach_impl` to write the resolved
/// exec target into.
pub struct ShellAttachCtx {
    pub app_name: String,
    pub result: Arc<Mutex<Option<ShellExecTarget>>>,
}

thread_local! {
    static SHELL_ATTACH_CTX: RefCell<Option<ShellAttachCtx>> = const { RefCell::new(None) };
}

pub fn set_shell_attach_ctx(ctx: ShellAttachCtx) {
    SHELL_ATTACH_CTX.with(|c| *c.borrow_mut() = Some(ctx));
}

pub fn clear_shell_attach_ctx() {
    SHELL_ATTACH_CTX.with(|c| *c.borrow_mut() = None);
}

// l[impl action.shell.attach]
pub fn register_shell_attach(engine: &mut rhai::Engine) {
    engine.register_fn("__bsl_shell_attach_impl", |job: rhai::Dynamic| {
        use crate::defs::job::Job;

        SHELL_ATTACH_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            let Some(ref c) = *ctx else { return };

            let Some(j) = job.try_cast::<Job>() else {
                return;
            };

            let job_def = j.def.lock().clone();
            let job_name = j.name.to_string();

            *c.result.lock() = Some(ShellExecTarget {
                job_def,
                job_name,
                app_name: c.app_name.clone(),
                instance_id: crate::runtime::identity::InstanceId::generate(),
            });
        });
    });
}

pub fn shell_attach_fn_ptr() -> rhai::FnPtr {
    rhai::FnPtr::new("__bsl_shell_attach_impl").expect("valid function name")
}
