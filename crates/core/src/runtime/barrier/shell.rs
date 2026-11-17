use std::{cell::RefCell, sync::Arc};

use parking_lot::Mutex;
use rhai::{CustomType, Dynamic, EvalAltResult, TypeBuilder};
use seedling_protocol::names::AppName;

/// The exec target resolved by `ShellControl::attach` from a Job.
/// Handed to the session's `ShellHost`, which translates it to a
/// `ContainerSpec` via the standard `job_spec` pipeline and runs
/// `podman run --rm -it`.
pub struct ShellExecTarget {
    pub job_def: crate::defs::job::JobDef,
    /// BSL-level name of the Job (from `app.job("name")`), used to derive
    /// the container display name.
    pub job_name: String,
    pub app_name: AppName,
    // spec: IDN#job-instances
    /// Fresh randomly-generated instance ID chosen at `attach()` call time.
    /// Each shell session gets a distinct ID so concurrent sessions against
    /// the same Job definition produce distinct container names.
    pub instance_id: crate::runtime::identity::InstanceId,
}

// spec: ACN#shellexit
/// How a shell's attachment ended: the job's exit status if its process
/// exited, and nothing if the attachment ended first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellExit {
    exit_code: Option<i32>,
}

impl ShellExit {
    /// The job's process exited with this status.
    pub fn exited(exit_code: i32) -> Self {
        Self {
            exit_code: Some(exit_code),
        }
    }

    /// The attachment ended without the job's process exiting, or its exit
    /// was never observed.
    pub fn ended() -> Self {
        Self { exit_code: None }
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }
}

impl CustomType for ShellExit {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("ShellExit")
            // spec: ACN#shellexit
            .with_fn("exited", |this: &mut Self| -> bool {
                this.exit_code.is_some()
            })
            // spec: ACN#shellexit
            .with_fn("exit_code", |this: &mut Self| -> Dynamic {
                this.exit_code
                    .map_or(Dynamic::UNIT, |code| Dynamic::from(i64::from(code)))
            })
            // spec: ACN#shellexit
            .with_fn("success", |this: &mut Self| -> bool {
                this.exit_code == Some(0)
            })
            // spec: ACN#shellexit
            .with_fn(
                "ensure_success",
                |this: &mut Self| -> Result<(), Box<EvalAltResult>> {
                    match this.exit_code {
                        Some(0) => Ok(()),
                        Some(code) => Err(Box::new(EvalAltResult::ErrorRuntime(
                            format!(
                                "shell job failed with exit code {}",
                                super::runtime::format_exit_code(code)
                            )
                            .into(),
                            rhai::Position::NONE,
                        ))),
                        None => Err(Box::new(EvalAltResult::ErrorRuntime(
                            "shell job did not exit".into(),
                            rhai::Position::NONE,
                        ))),
                    }
                },
            );
    }
}

/// What starts and runs a shell's job while the closure waits in
/// `shell.attach`.
pub trait ShellHost: Send + Sync {
    /// Starts the job, connects the operator's input and output to it, and
    /// returns once the attachment has ended, with the job stopped and the
    /// output finished. Called on the closure's own thread, so it blocks.
    /// An error is why the job could not be attached.
    fn attach(&self, target: ShellExecTarget) -> Result<ShellExit, String>;
}

/// What a shell closure has done, as the session needs to read it once the
/// closure has returned.
#[derive(Default)]
pub struct ShellProgress {
    attachment: Mutex<Option<Result<ShellExit, String>>>,
    error: Mutex<Option<String>>,
}

impl ShellProgress {
    /// Whether the closure called `shell.attach`, whether or not it worked.
    pub fn attached(&self) -> bool {
        self.attachment.lock().is_some()
    }

    /// Why the job could not be attached, if the closure tried.
    pub fn attach_failure(&self) -> Option<String> {
        self.attachment.lock().as_ref()?.as_ref().err().cloned()
    }

    /// The message of the closure's `shell.error`, if it called it.
    pub fn error(&self) -> Option<String> {
        self.error.lock().clone()
    }
}

/// Context installed in the thread-local before running a shell closure.
pub struct ShellAttachCtx {
    pub app_name: AppName,
    pub host: Arc<dyn ShellHost>,
    pub progress: Arc<ShellProgress>,
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

// spec: ACN#shell-control
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

    // spec: ACN#shell-control
    fn do_attach(&mut self, job: Dynamic) -> Result<ShellExit, Box<EvalAltResult>> {
        use crate::defs::job::Job;

        let throw = |message: String| {
            Box::new(EvalAltResult::ErrorRuntime(
                message.into(),
                rhai::Position::NONE,
            ))
        };

        if self.attached {
            return Err(throw("shell.attach() already called".into()));
        }

        let j = job
            .try_cast::<Job>()
            .ok_or_else(|| throw("shell.attach() requires a Job argument".into()))?;
        self.attached = true;

        let Some((host, progress, app_name)) = SHELL_ATTACH_CTX.with(|ctx| {
            ctx.borrow().as_ref().map(|c| {
                (
                    Arc::clone(&c.host),
                    Arc::clone(&c.progress),
                    c.app_name.clone(),
                )
            })
        }) else {
            // Image discovery and script exercising run the closure with no
            // session to attach to.
            return Ok(ShellExit::exited(0));
        };

        // The closure runs again from its start each time a later call
        // suspends it, and the attachment is not undone by that.
        if let Some(recorded) = progress.attachment.lock().clone() {
            return recorded.map_err(throw);
        }

        let attached = host.attach(ShellExecTarget {
            job_def: j.def.lock().clone(),
            job_name: j.name.to_string(),
            app_name,
            // spec: IDN#job-instances
            instance_id: crate::runtime::identity::InstanceId::generate(),
        });
        *progress.attachment.lock() = Some(attached.clone());
        attached.map_err(throw)
    }

    fn do_error(&mut self, msg: &str) -> Result<(), Box<EvalAltResult>> {
        SHELL_ATTACH_CTX.with(|ctx| {
            if let Some(c) = ctx.borrow().as_ref() {
                *c.progress.error.lock() = Some(msg.to_owned());
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
                |this: &mut Self, job: Dynamic| -> Result<ShellExit, Box<EvalAltResult>> {
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
