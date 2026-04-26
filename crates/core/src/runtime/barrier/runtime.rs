use std::{cell::RefCell, collections::HashMap, sync::Arc};

use rhai::{CustomType, Dynamic, EvalAltResult, Map, TypeBuilder};
use seedling_protocol::names::AppName;

use crate::defs::app::AppDef;
use crate::defs::container::canonicalise_signal_name;
use crate::defs::volume::OperationVolumeBinding;
use crate::runtime::barrier::{
    ActionLogEntry, BarrierCondition, BarrierRecord, CallKind, SharedContext,
};
use crate::runtime::registry::{InstanceRegistry, RegistryError};
use crate::runtime::{LifecycleState, ResourceInstance, restart_gens};

// ---------------------------------------------------------------------------
// Default barrier deadlines (seconds).
// ---------------------------------------------------------------------------

/// Default deadline for `.scheduled()` — a pod not scheduled inside this
/// window almost always indicates a cluster-level problem, not a slow workload.
// l[impl const.default-deadline]
pub const DEFAULT_SCHEDULED_DEADLINE_SECS: u64 = 30;

/// Default deadline for `.running()` — same reasoning as `.scheduled()`.
// l[impl const.default-deadline]
pub const DEFAULT_RUNNING_DEADLINE_SECS: u64 = 30;

/// Default deadline for `.ready()`. Bounded by default because an unready
/// resource is usually a deployment bug; callers that legitimately need to
/// wait indefinitely (e.g. Let's Encrypt cert provisioning under rate limit)
/// opt in to `.ready_eventually()`.
// l[impl const.default-deadline]
pub const DEFAULT_READY_DEADLINE_SECS: u64 = 30;

/// Default deadline for `.terminated()`. Sized for long-running jobs (the
/// common caller): 6 hours covers almost every realistic batch workload.
/// Callers that truly have no bound on duration use `.terminated_eventually()`.
// l[impl const.default-deadline]
pub const DEFAULT_TERMINATED_DEADLINE_SECS: u64 = 6 * 3600;

/// Default deadline for `rt.stop()` — stops should settle quickly.
// l[impl const.default-deadline]
pub const DEFAULT_STOP_DEADLINE_SECS: u64 = 30;

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
// Thread-local flag: set while a probe pass is evaluating a handler closure.
// BSL-facing functions that need to be robust to probe-supplied stub values
// (e.g. `app.external_volume(...)` called with an unset `param[...]` key)
// consult this flag and fall back to returning stubs instead of errors.
// ---------------------------------------------------------------------------

// r[impl image.discover]
thread_local! {
    static IN_PROBE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn is_in_probe() -> bool {
    IN_PROBE.with(|b| b.get())
}

pub fn set_in_probe(value: bool) {
    IN_PROBE.with(|b| b.set(value));
}

/// RAII guard that sets the probe flag on construction and clears it on
/// drop. Intended use: wrap the engine.eval call in `probe_handler`.
pub struct ProbeGuard;

impl ProbeGuard {
    pub fn new() -> Self {
        set_in_probe(true);
        Self
    }
}

impl Default for ProbeGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ProbeGuard {
    fn drop(&mut self) {
        set_in_probe(false);
    }
}

// ---------------------------------------------------------------------------
// Thread-local: action-context AppDef, set while an action closure executes.
// The App BSL methods read from this to enforce static/dynamic context rules.
// ---------------------------------------------------------------------------

thread_local! {
    static ACTION_DEF: RefCell<Option<Arc<arc_swap::ArcSwap<AppDef>>>> = const { RefCell::new(None) };
}

pub fn action_def() -> Option<Arc<arc_swap::ArcSwap<AppDef>>> {
    ACTION_DEF.with(|cell| cell.borrow().clone())
}

fn set_action_def(def: Arc<arc_swap::ArcSwap<AppDef>>) {
    ACTION_DEF.with(|cell| *cell.borrow_mut() = Some(def));
}

fn clear_action_def() {
    ACTION_DEF.with(|cell| *cell.borrow_mut() = None);
}

// ---------------------------------------------------------------------------
// Thread-local: operation-scoped volume bindings, set for the duration of
// an action closure. Populated by the runtime before invoking an action
// that requires bindings (e.g. backup operations); empty for normal actions.
// ---------------------------------------------------------------------------

thread_local! {
    static OPERATION_VOLUME_BINDINGS: RefCell<HashMap<String, OperationVolumeBinding>>
        = RefCell::new(HashMap::new());
}

/// Returns the operation-scoped binding for `name`, if one is set.
pub fn get_operation_volume_binding(name: &str) -> Option<OperationVolumeBinding> {
    OPERATION_VOLUME_BINDINGS.with(|m| m.borrow().get(name).cloned())
}

/// Replaces the current operation-scoped binding map.
pub fn set_operation_volume_bindings(bindings: HashMap<String, OperationVolumeBinding>) {
    OPERATION_VOLUME_BINDINGS.with(|m| *m.borrow_mut() = bindings);
}

fn clear_operation_volume_bindings() {
    OPERATION_VOLUME_BINDINGS.with(|m| m.borrow_mut().clear());
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
    pub fn new(
        action_def: Arc<arc_swap::ArcSwap<AppDef>>,
        op_id: String,
        bindings: HashMap<String, OperationVolumeBinding>,
    ) -> Self {
        IN_ACTION_CLOSURE.with(|b| b.set(true));
        set_action_def(action_def);
        ANON_VOL_OP_ID.with(|cell| *cell.borrow_mut() = Some(op_id));
        ANON_VOL_COUNTER.with(|c| c.set(0));
        set_operation_volume_bindings(bindings);
        Self
    }
}

impl Default for ActionClosureGuard {
    fn default() -> Self {
        Self::new(
            Arc::new(arc_swap::ArcSwap::new(Arc::new(AppDef::default()))),
            String::new(),
            HashMap::new(),
        )
    }
}

impl Drop for ActionClosureGuard {
    fn drop(&mut self) {
        IN_ACTION_CLOSURE.with(|b| b.set(false));
        clear_action_def();
        ANON_VOL_OP_ID.with(|cell| *cell.borrow_mut() = None);
        ANON_VOL_COUNTER.with(|c| c.set(0));
        clear_operation_volume_bindings();
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
// CancelHit — operation cancellation control-flow exception
// ---------------------------------------------------------------------------

// r[impl operation.cancel]
#[derive(Debug, Clone)]
pub struct CancelHitPayload;

pub fn make_cancel_error() -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(CancelHitPayload),
        rhai::Position::NONE,
    ))
}

pub fn extract_cancel_hit(err: &EvalAltResult) -> bool {
    match err {
        EvalAltResult::ErrorRuntime(val, _) => val.clone().try_cast::<CancelHitPayload>().is_some(),
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _) => extract_cancel_hit(inner),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Image extraction from Deployment/Job resources
// ---------------------------------------------------------------------------

/// For each Deployment/Job in `resources_with_defs`, return `(instance, image)`
/// pairs for those that declare a non-empty image reference. Anonymous
/// resources arrive with a def attached; named ones are looked up in the
/// action-context AppDef (the thread-local set by [`ActionClosureGuard`]).
pub(crate) fn extract_container_images(
    resources_with_defs: &[(ResourceInstance, Option<crate::defs::resource::Resource>)],
) -> Vec<(ResourceInstance, String)> {
    use crate::defs::resource::{Resource, ResourceKind};

    fn image_of(resource: &Resource) -> Option<String> {
        match resource {
            Resource::Deployment(dep) => dep.def.lock().pod.lock().container.lock().image.clone(),
            Resource::Job(job) => job.def.lock().pod.lock().container.lock().image.clone(),
            _ => None,
        }
    }

    let action_def = action_def();
    let mut out = Vec::new();
    for (inst, maybe_def) in resources_with_defs {
        if !matches!(inst.kind, ResourceKind::Deployment | ResourceKind::Job) {
            continue;
        }

        let image = match maybe_def {
            Some(r) => image_of(r),
            None => action_def.as_ref().and_then(|arc| {
                let def = arc.load_full();
                let name = inst.name.as_deref()?;
                let id = crate::defs::resource::ResourceId {
                    kind: inst.kind,
                    name: std::sync::Arc::new(name.to_owned()),
                };
                def.resources.get(&id).and_then(image_of)
            }),
        };

        if let Some(img) = image.filter(|s| !s.is_empty()) {
            out.push((inst.clone(), img));
        }
    }
    out
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
    pub app_name: AppName,
    pub registry: Arc<dyn InstanceRegistry>,
    pub db: Option<crate::runtime::db::DbHandle>,
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
            app_name: AppName::default(),
            registry: Arc::new(EphemeralInstanceRegistry::new()),
            db: None,
        }
    }

    pub fn with_context(
        ctx: SharedContext,
        app_name: AppName,
        registry: Arc<dyn InstanceRegistry>,
        db: Option<crate::runtime::db::DbHandle>,
    ) -> Self {
        Self {
            ctx: Some(ctx),
            app_name,
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
        use crate::defs::ingress::Ingress;
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
            // r[impl identity.scaled]
            // Named deployments resolve through the scaled-group API: the
            // steady-state reconciler keeps every replica as `is_scaled=1`,
            // so creating a singleton here would only produce a transient
            // placeholder that the next reconciliation tick has to retire.
            // The group's declared scale (low end of the range, default 1)
            // determines how many instances exist; an action that needs to
            // refer to all of them — `rt.signal`, `rt.stop`, `rt.query` —
            // gets every replica back from this single call.
            let scale = dep.def.lock().scale.start.max(1);
            let group = self.registry.ensure_scaled_group(
                &self.app_name,
                ResourceKind::Deployment,
                Some(&dep.name),
                scale,
            )?;
            return Ok(group.keep.into_iter().map(|inst| (inst, None)).collect());
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
            if let Some(ctx) = &self.ctx {
                let op_id_str = ctx.lock().operation_id.0.clone();
                let ns = uuid::Uuid::parse_str(&op_id_str).unwrap_or(uuid::Uuid::nil());
                let key = format!("job:{}", job.name.as_str());
                let id = InstanceId(uuid::Uuid::new_v5(&ns, key.as_bytes()));
                let instance = ResourceInstance {
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
                };
                // Persist named jobs to dynamic_resources so they are
                // visible in `apps show` while the operation is in progress.
                return Ok(vec![(instance, Some(Resource::Job(job)))]);
            }
            // Stub / steady-state context: fall back to registry which
            // returns the nil-UUID singleton for Jobs.
            return Ok(vec![(
                self.registry.get_or_create_singleton(
                    &self.app_name,
                    ResourceKind::Job,
                    Some(job.name.as_str()),
                )?,
                None,
            )]);
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

        if let Some(ing) = resources.clone().try_cast::<Ingress>() {
            let instance = self.registry.get_or_create_singleton(
                &self.app_name,
                ResourceKind::Ingress,
                Some(ing.name.as_str()),
            )?;
            return Ok(vec![(instance, Some(Resource::Ingress(ing)))]);
        }

        // l[impl rt.start]
        // l[impl collection.col]
        // App or Collection (e.g. `rt.start(app)` or `rt.start(col(app).except(...))`):
        // expand into the underlying resource handles via the standard
        // collection coercion, then recurse for each handle. The recursive
        // call hits the per-kind branches above (Deployment / Service /
        // Volume / Ingress / Job) and produces a flat list of resource
        // instances suitable for the action log.
        {
            use crate::defs::app::App;
            use crate::defs::collection::{Collection, col};
            let collection: Option<Collection> =
                if let Some(c) = resources.clone().try_cast::<Collection>() {
                    Some(c)
                } else if resources.clone().try_cast::<App>().is_some() {
                    Some(col(resources.clone()))
                } else {
                    None
                };
            if let Some(collection) = collection {
                let mut out = Vec::new();
                for handle in collection.resolve() {
                    let dyn_val = handle.fetch();
                    if dyn_val.is_unit() {
                        continue;
                    }
                    out.extend(self.extract_instances(dyn_val)?);
                }
                return Ok(out);
            }
        }

        // Unknown — language-only / stub context.
        Ok(vec![])
    }

    // r[impl desired-state.during-operation]
    fn do_start(
        &mut self,
        resources_with_defs: Vec<(ResourceInstance, Option<crate::defs::resource::Resource>)>,
    ) -> Result<Started, Box<EvalAltResult>> {
        let resources: Vec<ResourceInstance> =
            resources_with_defs.iter().map(|(r, _)| r.clone()).collect();

        // r[impl image.discover]
        // Probe mode: capture images that would be pulled and return an
        // already-satisfied Started. No log entries, no DB writes.
        if let Some(ctx) = &self.ctx
            && let Some(probe) = ctx.lock().probe_images.clone()
        {
            for (_, img) in extract_container_images(&resources_with_defs) {
                probe.lock().insert(img);
            }
            return Ok(Started {
                ctx: Some(Arc::clone(ctx)),
                resources,
                warm: WarmMode::None,
            });
        }

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
                    let instance = instance.clone();
                    let op_id = op_id.clone();
                    if let Err(e) = db.call(move |db| {
                        crate::runtime::desired::insert_dynamic_resource(db, &instance, &op_id)
                    }) {
                        tracing::warn!("failed to persist dynamic resource: {e}");
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
                    warm: WarmMode::None,
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
                    extra: None,
                });
                g.call_index += 1;
            }
        }

        Ok(Started {
            ctx: Some(ctx),
            resources,
            warm: WarmMode::None,
        })
    }

    // l[impl rt.warm-certs]
    // r[impl actuate.ingress.warm-certs]
    fn do_warm_certs(
        &mut self,
        resources_with_defs: Vec<(ResourceInstance, Option<crate::defs::resource::Resource>)>,
    ) -> Result<Started, Box<EvalAltResult>> {
        // Filter to TLS-terminating ingresses; ignore everything else
        // (per the language spec rule l[rt.warm-certs]).
        let resources: Vec<ResourceInstance> = resources_with_defs
            .into_iter()
            .filter(|(inst, def)| {
                if inst.kind != crate::defs::resource::ResourceKind::Ingress {
                    return false;
                }
                // Filter to TLS-terminating ingresses by inspecting the def.
                if let Some(crate::defs::resource::Resource::Ingress(ing)) = def {
                    let i = ing.def.lock();
                    i.tls
                } else {
                    // No definition handy: assume yes — the caller passed an
                    // Ingress resource explicitly. The Caddy reconciler will
                    // re-check before pushing config.
                    true
                }
            })
            .map(|(inst, _)| inst)
            .collect();

        // r[impl image.discover]
        // Probe mode: warm_certs has no image content to surface and no
        // relevant side effect for discovery; return an immediate Started.
        if let Some(ctx) = &self.ctx
            && ctx.lock().probe_mode()
        {
            return Ok(Started {
                ctx: Some(Arc::clone(ctx)),
                resources,
                warm: WarmMode::Certs,
            });
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
                    warm: WarmMode::Certs,
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
                    call_kind: CallKind::WarmCerts,
                    resources: resources.clone(),
                    barrier: None,
                    extra: None,
                });
                g.call_index += 1;
            }
        }

        Ok(Started {
            ctx: Some(ctx),
            resources,
            warm: WarmMode::Certs,
        })
    }

    // l[impl rt.warm-images]
    // r[impl actuate.image.warm]
    fn do_warm_images(
        &mut self,
        resources_with_defs: Vec<(ResourceInstance, Option<crate::defs::resource::Resource>)>,
    ) -> Result<Started, Box<EvalAltResult>> {
        let pairs = extract_container_images(&resources_with_defs);

        // Deduplicate image refs while keeping the resource list for audit.
        let mut refs: Vec<String> = Vec::new();
        for (_, img) in &pairs {
            if !refs.contains(img) {
                refs.push(img.clone());
            }
        }
        let resources: Vec<ResourceInstance> = pairs.into_iter().map(|(r, _)| r).collect();

        // r[impl image.discover]
        // Probe mode: capture refs, skip pin writes and log entries, and
        // return a Started whose barrier resolves immediately.
        if let Some(ctx) = &self.ctx
            && let Some(probe) = ctx.lock().probe_images.clone()
        {
            let mut p = probe.lock();
            for r in &refs {
                p.insert(r.clone());
            }
            drop(p);
            return Ok(Started {
                ctx: Some(Arc::clone(ctx)),
                resources,
                warm: WarmMode::Images(refs),
            });
        }

        // Persist pins immediately. Pins are idempotent and outlive the
        // in-memory action log — replay is free to re-upsert the same rows.
        if let Some(db) = &self.db {
            for reference in &refs {
                let app = self.app_name.clone();
                let reference = reference.clone();
                if let Err(e) =
                    db.call(move |db| crate::runtime::images::upsert_pin(db, &app, &reference))
                {
                    tracing::warn!(error = %e, "rt.warm_images: failed to persist image pin");
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
                    warm: WarmMode::Images(refs),
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
                    call_kind: CallKind::WarmImages,
                    resources: resources.clone(),
                    barrier: None,
                    extra: None,
                });
                g.call_index += 1;
            }
        }

        Ok(Started {
            ctx: Some(ctx),
            resources,
            warm: WarmMode::Images(refs),
        })
    }

    // l[impl rt.signal]
    // r[impl rt.signal]
    /// Deliver a POSIX signal to one or more running container instances.
    /// Replay-safe: an entry committed in a prior pass is skipped on the
    /// next replay so the signal is sent at most once. The actual signal
    /// delivery goes through the operation's `container_signaler` hook
    /// (set by the operation loop in `oi/handler/actions/lifecycle.rs`),
    /// which calls into the system actuator.
    fn do_signal(
        &mut self,
        resources: Vec<ResourceInstance>,
        signal: &str,
    ) -> Result<(), Box<EvalAltResult>> {
        use crate::defs::resource::ResourceKind;
        let canonical = canonicalise_signal_name(signal).ok_or_else(|| {
            Box::<EvalAltResult>::from(format!(
                "rt.signal: unknown signal {signal:?}; expected something like \"SIGHUP\" or \"HUP\""
            ))
        })?;

        if resources.is_empty() {
            return Err(
                format!("rt.signal: target resolved to no instances (signal={canonical})").into(),
            );
        }

        // Expand each named container target to all of its currently-existing
        // instances so a scaled deployment receives the signal on every
        // replica, not the placeholder singleton that extract_instances
        // returns. Anonymous targets (e.g. a Job created inside an action
        // closure) keep their deterministic UUID.
        let mut expanded: Vec<ResourceInstance> = Vec::new();
        for r in &resources {
            let needs_lookup =
                matches!(r.kind, ResourceKind::Deployment | ResourceKind::Job) && r.name.is_some();
            if needs_lookup {
                match self
                    .registry
                    .find_all_instances(&r.app, r.kind, r.name.as_deref())
                {
                    Ok(found) if !found.is_empty() => {
                        for inst in found {
                            if !expanded.iter().any(|e| e.id == inst.id) {
                                expanded.push(inst);
                            }
                        }
                        continue;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(
                            target = %r.display_name,
                            "rt.signal: registry lookup failed; falling back to literal target: {e}",
                        );
                    }
                }
            }
            if !expanded.iter().any(|e| e.id == r.id) {
                expanded.push(r.clone());
            }
        }

        if let Some(ctx) = &self.ctx
            && ctx.lock().probe_mode()
        {
            return Ok(());
        }

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

        {
            let mut g = ctx.lock();
            let already = g.committed.iter().any(|e| {
                matches!(e.call_kind, CallKind::Signal)
                    && e.resources == expanded
                    && e.extra.as_deref() == Some(canonical.as_str())
            });
            if already {
                if g.is_replaying() {
                    g.call_index += 1;
                }
                return Ok(());
            }
        }

        let signaler = ctx.lock().container_signaler.clone();
        if let Some(signaler) = signaler {
            for r in &expanded {
                if let Err(e) = signaler.signal(&r.display_name, canonical.as_str()) {
                    tracing::warn!(
                        instance = %r.display_name,
                        signal = %canonical,
                        "rt.signal: signal delivery failed: {e}",
                    );
                }
            }
        }

        {
            let mut g = ctx.lock();
            let idx = g.call_index;
            g.pending.push(ActionLogEntry {
                call_index: idx,
                call_kind: CallKind::Signal,
                resources: expanded.clone(),
                barrier: None,
                extra: Some(canonical.clone()),
            });
            g.call_index += 1;
        }

        Ok(())
    }

    // r[impl barrier.replay.rt-stop]
    fn do_stop(
        &mut self,
        resources: Vec<ResourceInstance>,
        deadline_secs: Option<u64>,
    ) -> Result<(), Box<EvalAltResult>> {
        // r[impl image.discover]
        // Probe mode: stop has no image content and no discovery-relevant
        // side effect. Return immediately without touching the log.
        if let Some(ctx) = &self.ctx
            && ctx.lock().probe_mode()
        {
            let _ = (resources, deadline_secs);
            return Ok(());
        }

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

        // r[impl operation.cancel]
        if ctx.lock().cancel_token.is_cancelled() {
            return Err(make_cancel_error());
        }

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

        // Always consult the oracle BEFORE the deadline check: if the world
        // says these resources are already terminated, the call must succeed
        // even when more than `deadline_secs` have elapsed. See the matching
        // comment in check_barrier — the committed log can lag the world.
        let all_terminated = resources.iter().all(|r| {
            g.world
                .lifecycle_state(r)
                .has_reached(LifecycleState::Terminated)
        });

        if !all_terminated {
            // r[impl barrier.deadline]
            // r[impl barrier.replay.rt-stop]
            // Enforce the stop deadline against the earliest unsatisfied record
            // for these resources. Mirrors the check in check_barrier so
            // rt.stop() participates in the same suspension semantics as the
            // Started barrier methods; before this, the deadline was stored but
            // never read.
            let started_at = g
                .committed
                .iter()
                .chain(g.pending.iter())
                .find(|e| {
                    e.resources == resources
                        && e.barrier.as_ref().is_some_and(|b| {
                            b.required_state == LifecycleState::Terminated && !b.satisfied
                        })
                })
                .and_then(|e| e.barrier.as_ref()?.started_at_secs);
            if let (Some(d), Some(started_at)) = (deadline_secs, started_at)
                && now.saturating_sub(started_at) >= d
            {
                return Err(Box::new(EvalAltResult::ErrorRuntime(
                    format!("Barrier deadline of {d}s exceeded waiting for Terminated (rt.stop)")
                        .into(),
                    rhai::Position::NONE,
                )));
            }
        }

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
                extra: None,
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
                extra: None,
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
            // l[impl rt.restart]
            .with_fn(
                "restart",
                |this: &mut Self, resource: Dynamic| -> Result<(), Box<EvalAltResult>> {
                    use crate::defs::deployment::Deployment;
                    let dep = resource.try_cast::<Deployment>().ok_or_else(
                        || -> Box<EvalAltResult> { "rt.restart expects a Deployment".into() },
                    )?;
                    let dep_name = dep.name.as_str().to_owned();
                    if dep_name.is_empty() {
                        return Err("rt.restart requires a named deployment".into());
                    }
                    // r[impl image.discover]
                    if let Some(ctx) = &this.ctx
                        && ctx.lock().probe_mode()
                    {
                        return Ok(());
                    }
                    if let Some(db) = &this.db {
                        let app_name = this.app_name.clone();
                        db.call(move |db| restart_gens::bump_restart_gen(db, &app_name, &dep_name))
                            .map_err(|e| -> Box<EvalAltResult> {
                                format!("rt.restart db error: {e}").into()
                            })?;
                    }
                    Ok(())
                },
            )
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
                    this.do_stop(instances, Some(DEFAULT_STOP_DEADLINE_SECS))
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
                    this.do_stop(instances, Some(deadline.max(0) as u64))
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
            // l[impl rt.warm-certs]
            .with_fn(
                "warm_certs",
                |this: &mut Self, resources: Dynamic| -> Result<Started, Box<EvalAltResult>> {
                    let resources_with_defs = this.extract_instances(resources).map_err(|e| {
                        Box::new(EvalAltResult::ErrorRuntime(
                            e.to_string().into(),
                            rhai::Position::NONE,
                        ))
                    })?;
                    this.do_warm_certs(resources_with_defs)
                },
            )
            // l[impl rt.warm-images]
            .with_fn(
                "warm_images",
                |this: &mut Self, resources: Dynamic| -> Result<Started, Box<EvalAltResult>> {
                    let resources_with_defs = this.extract_instances(resources).map_err(|e| {
                        Box::new(EvalAltResult::ErrorRuntime(
                            e.to_string().into(),
                            rhai::Position::NONE,
                        ))
                    })?;
                    this.do_warm_images(resources_with_defs)
                },
            )
            // l[impl rt.signal]
            .with_fn(
                "signal",
                |this: &mut Self,
                 target: Dynamic,
                 signal: &str|
                 -> Result<(), Box<EvalAltResult>> {
                    let resources_with_defs = this.extract_instances(target).map_err(|e| {
                        Box::new(EvalAltResult::ErrorRuntime(
                            e.to_string().into(),
                            rhai::Position::NONE,
                        ))
                    })?;
                    let resources: Vec<ResourceInstance> =
                        resources_with_defs.into_iter().map(|(r, _)| r).collect();
                    this.do_signal(resources, signal)
                },
            );
    }
}

// ---------------------------------------------------------------------------
// Started
// ---------------------------------------------------------------------------

/// Discriminator for the "warm" variants of a `Started`. Changes how
/// `Started::ready()` evaluates its barrier.
#[derive(Debug, Clone, Default)]
pub enum WarmMode {
    /// Standard: `.ready()` waits for the lifecycle to reach `Ready`.
    #[default]
    None,
    /// `rt.warm_certs` returned this `Started`. `.ready()` waits for the
    /// TLS cert of every resource to be observed valid.
    // l[impl rt.warm-certs]
    Certs,
    /// `rt.warm_images` returned this `Started`. `.ready()` waits until each
    /// image reference is present in local container storage.
    // l[impl rt.warm-images]
    Images(Vec<String>),
}

// l[impl rt.started.type]
#[derive(Debug, Clone)]
pub struct Started {
    pub ctx: Option<SharedContext>,
    pub resources: Vec<ResourceInstance>,
    pub warm: WarmMode,
}

impl Started {
    // r[impl barrier.suspension]
    fn check_barrier(
        &mut self,
        required: LifecycleState,
        deadline_secs: Option<u64>,
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

        // r[impl image.discover]
        // Probe mode: every barrier resolves immediately.
        if ctx.lock().probe_mode() {
            let _ = (required, deadline_secs);
            return Ok(self.clone());
        }

        // r[impl operation.cancel]
        // Cooperatively honour a cancel request before doing any more work.
        if ctx.lock().cancel_token.is_cancelled() {
            return Err(make_cancel_error());
        }

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

        // Check if condition is currently satisfied. Warm variants swap out
        // the predicate for `.ready()`:
        //
        // - `rt.warm_certs` checks cert validity directly via the oracle,
        //   since the standard ingress `Ready` lifecycle also requires routing
        //   and warm_certs intentionally does not route traffic.
        // - `rt.warm_images` checks that each recorded image reference is
        //   present in local container storage; the resources it holds are
        //   only for audit (and may be empty for anonymous references).
        let all_reached = match (&self.warm, required) {
            // l[impl rt.warm-certs]
            // An empty resource list here means every element was filtered
            // out (e.g. no TLS-terminating ingresses in the selection); the
            // language spec says such a `Started` is immediately satisfied.
            (WarmMode::Certs, LifecycleState::Ready) => {
                self.resources.iter().all(|r| g.world.cert_valid_for(r))
            }
            // l[impl rt.warm-images]
            // r[impl actuate.image.warm]
            // Empty `refs` means the selection contained no container
            // resources with an image to warm — immediately satisfied.
            (WarmMode::Images(refs), LifecycleState::Ready) => {
                refs.iter().all(|r| g.world.image_present(r))
            }
            _ => {
                !self.resources.is_empty()
                    && self
                        .resources
                        .iter()
                        .all(|r| g.world.lifecycle_state(r).has_reached(required))
            }
        };

        // r[impl barrier.resume]
        // Always consult the oracle BEFORE the deadline check: a barrier that
        // is currently satisfied must succeed even if more than `deadline_secs`
        // have elapsed since the original suspension. The committed log can
        // lag the world (e.g. a short-lived job that completed during a
        // replay where the satisfied=true update never landed in committed),
        // so deadline-then-oracle would spuriously time out a barrier whose
        // resources have actually reached the required state.
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

        // Not yet reached — check deadline: find when we first started waiting.
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
        if let (Some(d), Some(started_at)) = (deadline_secs, started_at)
            && now.saturating_sub(started_at) >= d
        {
            return Err(Box::new(EvalAltResult::ErrorRuntime(
                format!("Barrier deadline of {d}s exceeded waiting for {required:?}").into(),
                rhai::Position::NONE,
            )));
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
                extra: None,
            });
        }
        g.pending_barrier = Some(condition.clone());
        drop(g);
        Err(make_barrier_error(condition))
    }

    // l[impl rt.termination.ensure-success]
    /// Consult the world oracle for every resource in this `Started` group
    /// and aggregate their termination outcomes. All resources must report
    /// `Some(true)` for the group to be considered successful; any `Some(false)`
    /// makes the group a failure; any `None` (resource didn't record a
    /// meaningful outcome — e.g. a Deployment or Job that the oracle has no
    /// exit observation for) is conservatively treated as failure too, so an
    /// ensure_success() call never silently passes for a resource whose
    /// success state is unknown.
    fn compute_termination(&self) -> Termination {
        let Some(ctx) = &self.ctx else {
            // Stub context (no real world to query) — treat as success so
            // BSL parse/type-check runs don't flap.
            return Termination { success: true };
        };
        // r[impl image.discover]
        // Probe mode: pretend every terminated resource succeeded. We
        // accept that this misses error-path image references (code
        // guarded by `termination.ensure_success()` throwing); catching
        // those would require speculatively executing both branches.
        if ctx.lock().probe_mode() {
            return Termination { success: true };
        }
        let world = Arc::clone(&ctx.lock().world);
        let success = self
            .resources
            .iter()
            .all(|r| world.termination_success(r).unwrap_or(false));
        Termination { success }
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
                    this.check_barrier(
                        LifecycleState::Scheduled,
                        Some(DEFAULT_SCHEDULED_DEADLINE_SECS),
                    )
                },
            )
            .with_fn(
                "scheduled",
                |this: &mut Self, d: i64| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Scheduled, Some(d.max(0) as u64))
                },
            )
            .with_fn(
                "running",
                |this: &mut Self| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Running, Some(DEFAULT_RUNNING_DEADLINE_SECS))
                },
            )
            .with_fn(
                "running",
                |this: &mut Self, d: i64| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Running, Some(d.max(0) as u64))
                },
            )
            .with_fn(
                "ready",
                |this: &mut Self| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Ready, Some(DEFAULT_READY_DEADLINE_SECS))
                },
            )
            .with_fn(
                "ready",
                |this: &mut Self, d: i64| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Ready, Some(d.max(0) as u64))
                },
            )
            // l[impl rt.started.ready-eventually]
            .with_fn(
                "ready_eventually",
                |this: &mut Self| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Ready, None)
                },
            )
            // l[impl rt.started.terminated]
            .with_fn(
                "terminated",
                |this: &mut Self| -> Result<Termination, Box<EvalAltResult>> {
                    this.check_barrier(
                        LifecycleState::Terminated,
                        Some(DEFAULT_TERMINATED_DEADLINE_SECS),
                    )?;
                    Ok(this.compute_termination())
                },
            )
            .with_fn(
                "terminated",
                |this: &mut Self, d: i64| -> Result<Termination, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Terminated, Some(d.max(0) as u64))?;
                    Ok(this.compute_termination())
                },
            )
            // l[impl rt.started.terminated-eventually]
            .with_fn(
                "terminated_eventually",
                |this: &mut Self| -> Result<Termination, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Terminated, None)?;
                    Ok(this.compute_termination())
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
