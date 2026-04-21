use std::{cell::RefCell, sync::Arc};

use parking_lot::Mutex;
use rhai::{CustomType, Dynamic, EvalAltResult, TypeBuilder};

/// The exec target resolved by `ShellControl::attach` from a Job.
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

/// Outcome of a shell closure: either an exec target or an error message.
pub enum ShellOutcome {
    Attach(ShellExecTarget),
    Error(String),
}

/// Context installed in the thread-local before running a shell closure.
/// Provides a slot for `ShellControl` methods to write results into.
pub struct ShellAttachCtx {
    pub app_name: String,
    pub result: Arc<Mutex<Option<ShellOutcome>>>,
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

// l[impl action.shell.control]
#[derive(Debug, Clone)]
pub struct ShellControl {
    attached: bool,
}

impl Default for ShellControl {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellControl {
    pub fn new() -> Self {
        Self { attached: false }
    }

    // l[impl action.shell.attach]
    fn do_attach(&mut self, job: Dynamic) -> Result<(), Box<EvalAltResult>> {
        use crate::defs::job::Job;

        if self.attached {
            return Err(Box::new(EvalAltResult::ErrorRuntime(
                "shell.attach() already called".into(),
                rhai::Position::NONE,
            )));
        }

        let j = job.try_cast::<Job>().ok_or_else(|| {
            Box::new(EvalAltResult::ErrorRuntime(
                "shell.attach() requires a Job argument".into(),
                rhai::Position::NONE,
            ))
        })?;

        SHELL_ATTACH_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            let Some(ref c) = *ctx else { return };

            let job_def = j.def.lock().clone();
            let job_name = j.name.to_string();

            *c.result.lock() = Some(ShellOutcome::Attach(ShellExecTarget {
                job_def,
                job_name,
                app_name: c.app_name.clone(),
                instance_id: crate::runtime::identity::InstanceId::generate(),
            }));
        });

        self.attached = true;
        Ok(())
    }

    fn do_error(&mut self, msg: &str) -> Result<(), Box<EvalAltResult>> {
        SHELL_ATTACH_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            if let Some(ref c) = *ctx {
                *c.result.lock() = Some(ShellOutcome::Error(msg.to_owned()));
            }
        });

        Err(Box::new(EvalAltResult::ErrorRuntime(
            format!("shell error: {msg}").into(),
            rhai::Position::NONE,
        )))
    }
}

impl CustomType for ShellControl {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("ShellControl")
            .with_fn(
                "attach",
                |this: &mut Self, job: Dynamic| -> Result<(), Box<EvalAltResult>> {
                    this.do_attach(job)
                },
            )
            .with_fn(
                "error",
                |this: &mut Self, msg: &str| -> Result<(), Box<EvalAltResult>> {
                    this.do_error(msg)
                },
            );
    }
}
