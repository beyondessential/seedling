use std::{cell::RefCell, sync::Arc};

use parking_lot::Mutex;
use rhai::{CustomType, Dynamic, EvalAltResult, Map, TypeBuilder};

use crate::defs::app::AppDef;
use crate::runtime::barrier::{
    ActionLogEntry, BarrierCondition, BarrierRecord, CallKind, SharedContext,
};
use crate::runtime::db::Db;
use crate::runtime::registry::{InstanceRegistry, RegistryError};
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

// ---------------------------------------------------------------------------
// Thread-local: action-context AppDef, set while an action closure executes.
// The App BSL methods read from this to enforce static/dynamic context rules.
// ---------------------------------------------------------------------------

thread_local! {
    static ACTION_DEF: RefCell<Option<Arc<Mutex<AppDef>>>> = const { RefCell::new(None) };
}

pub fn action_def() -> Option<Arc<Mutex<AppDef>>> {
    ACTION_DEF.with(|cell| cell.borrow().clone())
}

fn set_action_def(def: Arc<Mutex<AppDef>>) {
    ACTION_DEF.with(|cell| *cell.borrow_mut() = Some(def));
}

fn clear_action_def() {
    ACTION_DEF.with(|cell| *cell.borrow_mut() = None);
}

// ---------------------------------------------------------------------------
// Thread-locals: stable anonymous volume naming within an action closure.
// ---------------------------------------------------------------------------

thread_local! {
    static ANON_VOL_OP_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    static ANON_VOL_COUNTER: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Generate a stable anonymous volume name for the current operation.
/// Returns `None` if not in an action context.
pub fn next_anon_vol_id() -> Option<String> {
    ANON_VOL_OP_ID.with(|cell| {
        let op_id = cell.borrow();
        op_id.as_ref().map(|id| {
            let counter = ANON_VOL_COUNTER.with(|c| {
                let n = c.get();
                c.set(n + 1);
                n
            });
            let short = &id[..8.min(id.len())];
            format!("seedling-anon-{short}-vol-{counter}")
        })
    })
}

/// RAII guard that sets the in-action-closure flag on construction and clears
/// it on drop, ensuring the flag is always cleaned up even on early return.
pub struct ActionClosureGuard;

impl ActionClosureGuard {
    pub fn new(action_def: Arc<Mutex<AppDef>>, op_id: String) -> Self {
        IN_ACTION_CLOSURE.with(|b| b.set(true));
        set_action_def(action_def);
        ANON_VOL_OP_ID.with(|cell| *cell.borrow_mut() = Some(op_id));
        ANON_VOL_COUNTER.with(|c| c.set(0));
        Self
    }
}

impl Default for ActionClosureGuard {
    fn default() -> Self {
        Self::new(Arc::new(Mutex::new(AppDef::default())), String::new())
    }
}

impl Drop for ActionClosureGuard {
    fn drop(&mut self) {
        IN_ACTION_CLOSURE.with(|b| b.set(false));
        clear_action_def();
        ANON_VOL_OP_ID.with(|cell| *cell.borrow_mut() = None);
        ANON_VOL_COUNTER.with(|c| c.set(0));
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
    pub db: Option<Arc<parking_lot::Mutex<Db>>>,
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
            db: None,
        }
    }

    pub fn with_context(
        ctx: SharedContext,
        app_name: impl Into<String>,
        registry: Arc<dyn InstanceRegistry>,
        db: Option<Arc<parking_lot::Mutex<Db>>>,
    ) -> Self {
        Self {
            ctx: Some(ctx),
            app_name: app_name.into(),
            registry,
            db,
        }
    }

    /// Extract ResourceInstance values from a Rhai Dynamic argument.
    ///
    /// Recognises the concrete resource types registered with the engine and
    /// resolves them through the instance registry so the returned instances
    /// carry stable UUIDs.  Unknown types yield an empty vec (stub behaviour
    /// that leaves barriers trivially satisfied in language-only tests).
    fn extract_instances(
        &self,
        resources: Dynamic,
    ) -> Result<Vec<(ResourceInstance, Option<crate::defs::resource::Resource>)>, RegistryError>
    {
        use crate::defs::deployment::Deployment;
        use crate::defs::job::Job;
        use crate::defs::resource::{Resource, ResourceKind};
        use crate::defs::service::Service;
        use crate::runtime::identity::{InstanceId, InstanceVariant, ResourceInstance};

        if let Some(dep) = resources.clone().try_cast::<Deployment>() {
            if dep.name.is_empty()
                && let Some(ctx) = &self.ctx
            {
                let mut g = ctx.lock();
                let op_id = g.operation_id.0.clone();
                let counter = g.anon_counter;
                g.anon_counter += 1;
                drop(g);
                let ns = uuid::Uuid::parse_str(&op_id).unwrap_or(uuid::Uuid::nil());
                let key = format!("anon-deployment:{counter}");
                let id = InstanceId(uuid::Uuid::new_v5(&ns, key.as_bytes()));
                let display = format!("{}-anon-dep-{}", self.app_name, id.display_suffix());
                let instance = ResourceInstance {
                    id,
                    app: self.app_name.clone(),
                    kind: ResourceKind::Deployment,
                    name: None,
                    variant: InstanceVariant::Singleton,
                    display_name: display,
                };
                return Ok(vec![(instance, Some(Resource::Deployment(dep)))]);
            }
            return Ok(vec![(
                self.registry.get_or_create_singleton(
                    &self.app_name,
                    ResourceKind::Deployment,
                    Some(&dep.name),
                )?,
                None,
            )]);
        }

        if let Some(job) = resources.clone().try_cast::<Job>() {
            if job.name.is_empty()
                && let Some(ctx) = &self.ctx
            {
                let mut g = ctx.lock();
                let op_id = g.operation_id.0.clone();
                let counter = g.anon_counter;
                g.anon_counter += 1;
                drop(g);
                let ns = uuid::Uuid::parse_str(&op_id).unwrap_or(uuid::Uuid::nil());
                let key = format!("anon-job:{counter}");
                let id = InstanceId(uuid::Uuid::new_v5(&ns, key.as_bytes()));
                let display = format!("{}-anon-job-{}", self.app_name, id.display_suffix());
                let instance = ResourceInstance {
                    id,
                    app: self.app_name.clone(),
                    kind: ResourceKind::Job,
                    name: None,
                    variant: InstanceVariant::Singleton,
                    display_name: display,
                };
                return Ok(vec![(instance, Some(Resource::Job(job)))]);
            }
            // r[impl identity.job]
            // Named jobs inside an action closure get an instance ID derived
            // from the operation ID via UUID v5, giving a stable identity
            // within one operation (replay-safe) while ensuring distinct
            // invocations of the same action produce different container names.
            let instance = if let Some(ctx) = &self.ctx {
                let op_id_str = ctx.lock().operation_id.0.clone();
                let ns = uuid::Uuid::parse_str(&op_id_str).unwrap_or(uuid::Uuid::nil());
                let key = format!("job:{}", job.name.as_str());
                let id = InstanceId(uuid::Uuid::new_v5(&ns, key.as_bytes()));
                ResourceInstance {
                    id,
                    app: self.app_name.clone(),
                    kind: ResourceKind::Job,
                    name: Some(job.name.to_string()),
                    variant: InstanceVariant::Singleton,
                    display_name: format!(
                        "{}-{}-{}",
                        self.app_name,
                        job.name.as_str(),
                        id.display_suffix()
                    ),
                }
            } else {
                // Stub / steady-state context: fall back to registry which
                // returns the nil-UUID singleton for Jobs.
                self.registry.get_or_create_singleton(
                    &self.app_name,
                    ResourceKind::Job,
                    Some(job.name.as_str()),
                )?
            };
            return Ok(vec![(instance, None)]);
        }

        if let Some(svc) = resources.clone().try_cast::<Service>() {
            if svc.name.is_empty()
                && let Some(ctx) = &self.ctx
            {
                let mut g = ctx.lock();
                let op_id = g.operation_id.0.clone();
                let counter = g.anon_counter;
                g.anon_counter += 1;
                drop(g);
                let ns = uuid::Uuid::parse_str(&op_id).unwrap_or(uuid::Uuid::nil());
                let key = format!("anon-service:{counter}");
                let id = InstanceId(uuid::Uuid::new_v5(&ns, key.as_bytes()));
                let display = format!("{}-anon-svc-{}", self.app_name, id.display_suffix());
                let instance = ResourceInstance {
                    id,
                    app: self.app_name.clone(),
                    kind: ResourceKind::Service,
                    name: None,
                    variant: InstanceVariant::Singleton,
                    display_name: display,
                };
                return Ok(vec![(instance, Some(Resource::Service(svc)))]);
            }
            return Ok(vec![(
                self.registry.get_or_create_singleton(
                    &self.app_name,
                    ResourceKind::Service,
                    Some(&svc.name),
                )?,
                None,
            )]);
        }

        // Unknown / Collection stub — no resources to track.
        Ok(vec![])
    }

    // r[impl desired-state.during-operation]
    fn do_start(
        &mut self,
        resources_with_defs: Vec<(ResourceInstance, Option<crate::defs::resource::Resource>)>,
    ) -> Result<Started, Box<EvalAltResult>> {
        let resources: Vec<ResourceInstance> =
            resources_with_defs.iter().map(|(r, _)| r.clone()).collect();

        if let Some(ctx) = &self.ctx {
            let mut g = ctx.lock();
            for (instance, maybe_def) in &resources_with_defs {
                if let Some(def) = maybe_def {
                    g.dynamic_defs.insert(instance.clone(), def.clone());
                }
            }
        }

        if let (Some(db), Some(ctx)) = (&self.db, &self.ctx) {
            let op_id = ctx.lock().operation_id.0.clone();
            for (instance, maybe_def) in &resources_with_defs {
                if maybe_def.is_some() {
                    let db = db.lock();
                    if let Err(e) =
                        crate::runtime::desired::insert_dynamic_resource(&db, instance, &op_id)
                    {
                        tracing::warn!(
                            instance_id = %instance.id.to_hex(),
                            "failed to persist dynamic resource: {e}"
                        );
                    }
                }
            }
        }

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
                    let resources_with_defs = this.extract_instances(resources).map_err(|e| {
                        Box::new(EvalAltResult::ErrorRuntime(
                            e.to_string().into(),
                            rhai::Position::NONE,
                        ))
                    })?;
                    this.do_start(resources_with_defs)
                },
            )
            // l[impl rt.stop]
            .with_fn(
                "stop",
                |this: &mut Self, resources: Dynamic| -> Result<(), Box<EvalAltResult>> {
                    let instances = this
                        .extract_instances(resources)
                        .map_err(|e| {
                            Box::new(EvalAltResult::ErrorRuntime(
                                e.to_string().into(),
                                rhai::Position::NONE,
                            ))
                        })?
                        .into_iter()
                        .map(|(r, _)| r)
                        .collect();
                    this.do_stop(instances, 30)
                },
            )
            .with_fn(
                "stop",
                |this: &mut Self,
                 resources: Dynamic,
                 deadline: i64|
                 -> Result<(), Box<EvalAltResult>> {
                    let instances = this
                        .extract_instances(resources)
                        .map_err(|e| {
                            Box::new(EvalAltResult::ErrorRuntime(
                                e.to_string().into(),
                                rhai::Position::NONE,
                            ))
                        })?
                        .into_iter()
                        .map(|(r, _)| r)
                        .collect();
                    this.do_stop(instances, deadline.max(0) as u64)
                },
            )
            // l[impl rt.query]
            .with_fn(
                "query",
                |this: &mut Self, resources: Dynamic| -> Result<Started, Box<EvalAltResult>> {
                    let resources_with_defs = this.extract_instances(resources).map_err(|e| {
                        Box::new(EvalAltResult::ErrorRuntime(
                            e.to_string().into(),
                            rhai::Position::NONE,
                        ))
                    })?;
                    this.do_start(resources_with_defs)
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
