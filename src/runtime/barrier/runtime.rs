use std::{cell::RefCell, sync::Arc};

use parking_lot::Mutex;
use rhai::{CustomType, Dynamic, EvalAltResult, Map, TypeBuilder};

use crate::runtime::barrier::{
    ActionLogEntry, BarrierCondition, BarrierRecord, CallKind, SharedContext,
};
use crate::runtime::registry::InstanceRegistry;
use crate::runtime::{LifecycleState, ResourceInstance};

// ---------------------------------------------------------------------------
// Thread-local flag: set when a BarrierHit is thrown so that subsequent
// rt.* calls inside a BSL try/catch block re-throw it instead of proceeding.
// ---------------------------------------------------------------------------

thread_local! {
    static BARRIER_HIT_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn set_barrier_hit() {
    BARRIER_HIT_PENDING.with(|b| b.set(true));
}

pub fn clear_barrier_hit() {
    BARRIER_HIT_PENDING.with(|b| b.set(false));
}

fn is_barrier_hit_pending() -> bool {
    BARRIER_HIT_PENDING.with(|b| b.get())
}

// ---------------------------------------------------------------------------
// Thread-local flag: set while an action closure is executing so that
// top-level-only registrations (e.g. param.on_change) can detect misuse.
// ---------------------------------------------------------------------------

thread_local! {
    static IN_ACTION_CLOSURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn is_in_action_closure() -> bool {
    IN_ACTION_CLOSURE.with(|b| b.get())
}

/// RAII guard that sets the in-action-closure flag on construction and clears
/// it on drop, ensuring the flag is always cleaned up even on early return.
pub struct ActionClosureGuard;

impl Default for ActionClosureGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionClosureGuard {
    pub fn new() -> Self {
        IN_ACTION_CLOSURE.with(|b| b.set(true));
        Self
    }
}

impl Drop for ActionClosureGuard {
    fn drop(&mut self) {
        IN_ACTION_CLOSURE.with(|b| b.set(false));
    }
}

// ---------------------------------------------------------------------------
// BarrierHit — the internal control-flow exception
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BarrierHitPayload(pub BarrierCondition);

pub fn make_barrier_error(condition: BarrierCondition) -> Box<EvalAltResult> {
    set_barrier_hit();
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(BarrierHitPayload(condition)),
        rhai::Position::NONE,
    ))
}

pub fn extract_barrier_hit(err: &EvalAltResult) -> Option<BarrierCondition> {
    match err {
        EvalAltResult::ErrorRuntime(val, _) => {
            val.clone().try_cast::<BarrierHitPayload>().map(|p| p.0)
        }
        // Rhai wraps errors thrown from registered functions in ErrorInFunctionCall.
        // Recurse through the wrapper to find the inner BarrierHitPayload.
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _) => extract_barrier_hit(inner),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Resource extraction from Rhai Dynamic
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// RuntimeInstance
// ---------------------------------------------------------------------------

// l[impl rt.var]
// l[impl rt.type]
// l[impl rt.constructor]
#[derive(Clone)]
pub struct RuntimeInstance {
    pub ctx: Option<SharedContext>,
    pub app_name: String,
    pub registry: Arc<dyn InstanceRegistry>,
}

impl std::fmt::Debug for RuntimeInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeInstance")
            .field("ctx", &self.ctx)
            .field("app_name", &self.app_name)
            .finish_non_exhaustive()
    }
}

impl RuntimeInstance {
    pub fn stub() -> Self {
        use crate::runtime::registry::EphemeralInstanceRegistry;
        Self {
            ctx: None,
            app_name: String::new(),
            registry: Arc::new(EphemeralInstanceRegistry::new()),
        }
    }

    pub fn with_context(
        ctx: SharedContext,
        app_name: impl Into<String>,
        registry: Arc<dyn InstanceRegistry>,
    ) -> Self {
        Self {
            ctx: Some(ctx),
            app_name: app_name.into(),
            registry,
        }
    }

    /// Extract ResourceInstance values from a Rhai Dynamic argument.
    ///
    /// Recognises the concrete resource types registered with the engine and
    /// resolves them through the instance registry so the returned instances
    /// carry stable UUIDs.  Unknown types yield an empty vec (stub behaviour
    /// that leaves barriers trivially satisfied in language-only tests).
    fn extract_instances(&self, resources: Dynamic) -> Vec<ResourceInstance> {
        use crate::defs::deployment::Deployment;
        use crate::defs::job::Job;
        use crate::defs::resource::ResourceKind;
        use crate::defs::service::Service;

        if let Some(dep) = resources.clone().try_cast::<Deployment>() {
            return vec![self.registry.get_or_create_singleton(
                &self.app_name,
                ResourceKind::Deployment,
                Some(&dep.name),
            )];
        }
        if let Some(job) = resources.clone().try_cast::<Job>() {
            return vec![self.registry.get_or_create_singleton(
                &self.app_name,
                ResourceKind::Job,
                Some(&job.name),
            )];
        }
        if let Some(svc) = resources.clone().try_cast::<Service>() {
            return vec![self.registry.get_or_create_singleton(
                &self.app_name,
                ResourceKind::Service,
                Some(&svc.name),
            )];
        }

        // Unknown / Collection stub — no resources to track.
        vec![]
    }

    // r[impl desired-state.during-operation]
    fn do_start(
        &mut self,
        resources: Vec<ResourceInstance>,
    ) -> Result<Started, Box<EvalAltResult>> {
        if is_barrier_hit_pending()
            && let Some(ctx) = &self.ctx
            && let Some(cond) = ctx.lock().pending_barrier.clone()
        {
            return Err(make_barrier_error(cond));
        }

        let ctx = match &self.ctx {
            None => {
                return Ok(Started {
                    ctx: None,
                    resources,
                });
            }
            Some(c) => Arc::clone(c),
        };

        {
            let mut g = ctx.lock();
            if g.is_replaying() {
                g.call_index += 1;
            } else {
                let idx = g.call_index;
                g.pending.push(ActionLogEntry {
                    call_index: idx,
                    call_kind: CallKind::Start,
                    resources: resources.clone(),
                    barrier: None,
                });
                g.call_index += 1;
            }
        }

        Ok(Started {
            ctx: Some(ctx),
            resources,
        })
    }

    // r[impl barrier.replay.rt-stop]
    fn do_stop(
        &mut self,
        resources: Vec<ResourceInstance>,
        deadline_secs: u64,
    ) -> Result<(), Box<EvalAltResult>> {
        if is_barrier_hit_pending()
            && let Some(ctx) = &self.ctx
            && let Some(cond) = ctx.lock().pending_barrier.clone()
        {
            return Err(make_barrier_error(cond));
        }

        let ctx = match &self.ctx {
            None => return Ok(()),
            Some(c) => Arc::clone(c),
        };

        let mut g = ctx.lock();

        if g.is_replaying() {
            // If the committed entry shows the barrier was already satisfied, skip.
            let already = g
                .committed_entry()
                .and_then(|e| e.barrier.as_ref())
                .is_some_and(|b| b.satisfied);
            g.call_index += 1;
            if already {
                return Ok(());
            }
            // Otherwise fall through to check the oracle.
        }

        let now = (g.now_secs)();
        let all_terminated = resources.iter().all(|r| {
            g.world
                .lifecycle_state(r)
                .has_reached(LifecycleState::Terminated)
        });

        if all_terminated {
            // If replaying, call_index was already incremented above; use index - 1.
            // If live and this is the first call (call_index == 0), increment and use 0.
            // If live and call_index > 0, use call_index - 1 (from a prior increment).
            let idx = if g.call_index > 0 {
                g.call_index - 1
            } else {
                let i = g.call_index;
                g.call_index += 1;
                i
            };
            g.pending.push(ActionLogEntry {
                call_index: idx,
                call_kind: CallKind::Stop,
                resources: resources.clone(),
                barrier: Some(BarrierRecord {
                    required_state: LifecycleState::Terminated,
                    deadline_secs,
                    satisfied: true,
                    started_at_secs: Some(now),
                }),
            });
            Ok(())
        } else {
            let condition = BarrierCondition {
                resources: resources.clone(),
                required_state: LifecycleState::Terminated,
                deadline_secs,
            };
            let idx = g.call_index;
            g.call_index += 1;
            g.pending.push(ActionLogEntry {
                call_index: idx,
                call_kind: CallKind::Stop,
                resources: resources.clone(),
                barrier: Some(BarrierRecord {
                    required_state: LifecycleState::Terminated,
                    deadline_secs,
                    satisfied: false,
                    started_at_secs: Some(now),
                }),
            });
            g.pending_barrier = Some(condition.clone());
            drop(g);
            Err(make_barrier_error(condition))
        }
    }
}

// l[impl rt.methods]
impl CustomType for RuntimeInstance {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("RuntimeInstance")
            // l[impl rt.start]
            .with_fn(
                "start",
                |this: &mut Self, resources: Dynamic| -> Result<Started, Box<EvalAltResult>> {
                    let instances = this.extract_instances(resources);
                    this.do_start(instances)
                },
            )
            // l[impl rt.stop]
            .with_fn(
                "stop",
                |this: &mut Self, resources: Dynamic| -> Result<(), Box<EvalAltResult>> {
                    let instances = this.extract_instances(resources);
                    this.do_stop(instances, 30)
                },
            )
            .with_fn(
                "stop",
                |this: &mut Self,
                 resources: Dynamic,
                 deadline: i64|
                 -> Result<(), Box<EvalAltResult>> {
                    let instances = this.extract_instances(resources);
                    this.do_stop(instances, deadline.max(0) as u64)
                },
            )
            // l[impl rt.query]
            .with_fn(
                "query",
                |this: &mut Self, resources: Dynamic| -> Result<Started, Box<EvalAltResult>> {
                    let instances = this.extract_instances(resources);
                    this.do_start(instances)
                },
            )
            // l[impl rt.reconcile]
            // r[impl reconcile.operation]
            // r[impl reconcile.supported-pairs]
            .with_fn(
                "reconcile",
                |this: &mut Self,
                 _old: Dynamic,
                 _new: Dynamic|
                 -> Result<Started, Box<EvalAltResult>> { this.do_start(vec![]) },
            );
    }
}

// ---------------------------------------------------------------------------
// Started
// ---------------------------------------------------------------------------

// l[impl rt.started.type]
#[derive(Debug, Clone)]
pub struct Started {
    pub ctx: Option<SharedContext>,
    pub resources: Vec<ResourceInstance>,
}

impl Started {
    // r[impl barrier.suspension]
    fn check_barrier(
        &mut self,
        required: LifecycleState,
        deadline_secs: u64,
    ) -> Result<Self, Box<EvalAltResult>> {
        if is_barrier_hit_pending()
            && let Some(ctx) = &self.ctx
            && let Some(cond) = ctx.lock().pending_barrier.clone()
        {
            return Err(make_barrier_error(cond));
        }

        let ctx = match &self.ctx {
            None => return Ok(self.clone()),
            Some(c) => Arc::clone(c),
        };

        let mut g = ctx.lock();
        let now = (g.now_secs)();

        // r[impl barrier.replay]
        // Check if this barrier was already satisfied in the committed log.
        let already_satisfied = g.committed.iter().any(|e| {
            e.resources == self.resources
                && e.barrier
                    .as_ref()
                    .is_some_and(|b| b.required_state == required && b.satisfied)
        });
        if already_satisfied {
            return Ok(self.clone());
        }

        // Check deadline: find when we first started waiting.
        let started_at = g
            .committed
            .iter()
            .chain(g.pending.iter())
            .find(|e| {
                e.resources == self.resources
                    && e.barrier
                        .as_ref()
                        .is_some_and(|b| b.required_state == required && !b.satisfied)
            })
            .and_then(|e| e.barrier.as_ref()?.started_at_secs);

        // r[impl barrier.deadline]
        if let Some(started_at) = started_at
            && now.saturating_sub(started_at) >= deadline_secs
        {
            return Err(Box::new(EvalAltResult::ErrorRuntime(
                format!(
                    "Barrier deadline of {}s exceeded waiting for {:?}",
                    deadline_secs, required
                )
                .into(),
                rhai::Position::NONE,
            )));
        }

        // Check if condition is currently satisfied.
        let all_reached = !self.resources.is_empty()
            && self
                .resources
                .iter()
                .all(|r| g.world.lifecycle_state(r).has_reached(required));

        // r[impl barrier.resume]
        if all_reached {
            // Mark the relevant pending entry's barrier as satisfied.
            for e in g.pending.iter_mut() {
                if e.resources == self.resources
                    && let Some(b) = e.barrier.as_mut()
                    && b.required_state == required
                {
                    b.satisfied = true;
                }
            }
            return Ok(self.clone());
        }

        // Not satisfied: attach/update barrier record in pending, then throw.
        let condition = BarrierCondition {
            resources: self.resources.clone(),
            required_state: required,
            deadline_secs,
        };

        // Attach to the most recent pending Start entry for these resources,
        // or push a synthetic entry if none exists.
        let attached = g
            .pending
            .iter_mut()
            .rev()
            .find(|e| e.resources == self.resources && e.barrier.is_none());
        if let Some(entry) = attached {
            entry.barrier = Some(BarrierRecord {
                required_state: required,
                deadline_secs,
                satisfied: false,
                started_at_secs: Some(now),
            });
        } else {
            let idx = g.call_index;
            g.call_index += 1;
            g.pending.push(ActionLogEntry {
                call_index: idx,
                call_kind: CallKind::Start,
                resources: self.resources.clone(),
                barrier: Some(BarrierRecord {
                    required_state: required,
                    deadline_secs,
                    satisfied: false,
                    started_at_secs: Some(now),
                }),
            });
        }
        g.pending_barrier = Some(condition.clone());
        drop(g);
        Err(make_barrier_error(condition))
    }
}

impl CustomType for Started {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Started")
            // l[impl rt.started.state-methods]
            .with_fn(
                "scheduled",
                |this: &mut Self| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Scheduled, 30)
                },
            )
            .with_fn(
                "scheduled",
                |this: &mut Self, d: i64| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Scheduled, d.max(0) as u64)
                },
            )
            .with_fn(
                "running",
                |this: &mut Self| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Running, 30)
                },
            )
            .with_fn(
                "running",
                |this: &mut Self, d: i64| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Running, d.max(0) as u64)
                },
            )
            .with_fn(
                "ready",
                |this: &mut Self| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Ready, 30)
                },
            )
            .with_fn(
                "ready",
                |this: &mut Self, d: i64| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Ready, d.max(0) as u64)
                },
            )
            // l[impl rt.started.terminated]
            .with_fn(
                "terminated",
                |this: &mut Self| -> Result<Termination, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Terminated, 30)?;
                    Ok(Termination { success: true })
                },
            )
            .with_fn(
                "terminated",
                |this: &mut Self, d: i64| -> Result<Termination, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Terminated, d.max(0) as u64)?;
                    Ok(Termination { success: true })
                },
            )
            // l[impl rt.started.type]: Collection methods on Started return Started
            .with_fn("one", |this: &mut Self| this.clone())
            .with_fn("only", |this: &mut Self, _: Dynamic| this.clone())
            .with_fn("except", |this: &mut Self, _: Dynamic| this.clone())
            .with_fn("select", |this: &mut Self, _: Map| this.clone());
    }
}

// ---------------------------------------------------------------------------
// Termination
// ---------------------------------------------------------------------------

// l[impl rt.termination.type]
#[derive(Debug, Clone)]
pub struct Termination {
    pub success: bool,
}

impl CustomType for Termination {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Termination")
            // l[impl rt.termination.ensure-success]
            .with_fn(
                "ensure_success",
                |this: &mut Self| -> Result<(), Box<EvalAltResult>> {
                    if this.success {
                        Ok(())
                    } else {
                        Err(Box::new(EvalAltResult::ErrorRuntime(
                            "Resource did not terminate successfully".into(),
                            rhai::Position::NONE,
                        )))
                    }
                },
            );
    }
}

// ---------------------------------------------------------------------------
// Shell attach
// ---------------------------------------------------------------------------

thread_local! {
    static SHELL_ATTACH_CTX: RefCell<Option<ShellAttachCtx>> = const { RefCell::new(None) };
}

/// Context installed in the thread-local before running a shell closure.
/// Provides the registry for resolving job names → container display_names,
/// and a slot for `__bsl_shell_attach_impl` to write the result into.
pub struct ShellAttachCtx {
    pub app_name: String,
    pub registry: Arc<dyn crate::runtime::InstanceRegistry>,
    pub result: Arc<Mutex<Option<String>>>,
}

pub fn set_shell_attach_ctx(ctx: ShellAttachCtx) {
    SHELL_ATTACH_CTX.with(|c| *c.borrow_mut() = Some(ctx));
}

pub fn clear_shell_attach_ctx() {
    SHELL_ATTACH_CTX.with(|c| *c.borrow_mut() = None);
}

// l[impl action.shell.attach]
pub fn register_shell_attach(engine: &mut rhai::Engine) {
    engine.register_fn("__bsl_shell_attach_impl", |job: Dynamic| {
        use crate::defs::deployment::Deployment;
        use crate::defs::job::Job;
        use crate::defs::resource::ResourceKind;

        SHELL_ATTACH_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            let Some(ref c) = *ctx else { return };

            let container_name = if let Some(j) = job.clone().try_cast::<Job>() {
                c.registry
                    .get_or_create_singleton(&c.app_name, ResourceKind::Job, Some(j.name.as_str()))
                    .display_name
                    .clone()
            } else if let Some(d) = job.clone().try_cast::<Deployment>() {
                c.registry
                    .get_or_create_singleton(
                        &c.app_name,
                        ResourceKind::Deployment,
                        Some(d.name.as_str()),
                    )
                    .display_name
                    .clone()
            } else {
                return;
            };

            *c.result.lock() = Some(container_name);
        });
    });
}

pub fn shell_attach_fn_ptr() -> rhai::FnPtr {
    rhai::FnPtr::new("__bsl_shell_attach_impl").expect("valid function name")
}
