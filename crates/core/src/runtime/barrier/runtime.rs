use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
};

use rhai::{CustomType, Dynamic, EvalAltResult, Map, TypeBuilder};
use seedling_protocol::names::AppName;

use crate::defs::app::AppDef;
use crate::defs::collection::Selector;
use crate::defs::container::canonicalise_signal_name;
use crate::defs::volume::{
    ExternalVolume, OperationVolumeBinding, Volume, validate_volume_write_path,
};
use crate::runtime::barrier::{
    ActionLogEntry, BarrierCondition, BarrierRecord, CallKind, ReplayContext, ReplayMismatch,
    SharedContext, VolumeWriteTarget,
};
use crate::runtime::registry::{InstanceRegistry, InstanceReuse, RegistryError};
use crate::runtime::{LifecycleState, ResourceInstance, restart_gens};

// ---------------------------------------------------------------------------
// Default barrier deadlines (seconds).
// ---------------------------------------------------------------------------

/// Default deadline for `.scheduled()` — a pod not scheduled inside this
/// window almost always indicates a cluster-level problem, not a slow workload.
// spec: RT#barriers
/// Resolve an explicit deadline argument against the state's default.
///
/// `RT#barriers` says the argument must be a positive integer
/// number of seconds, and that zero or absent means the state's default.
/// Both used to go through `d.max(0) as u64`, so `0` became `Some(0)` — which
/// the `elapsed >= d` check trips on the very next pass, the exact opposite
/// of the documented fallback. A script writing `ready(0)` expecting the
/// default 30s got "Barrier deadline of 0s exceeded" about two seconds later.
///
/// A negative is not a deadline at all, and the defs layer's rule is to throw
/// on malformed input rather than coerce it into something that means
/// something else.
// spec: RT#barriers
fn resolve_deadline(d: i64, default_secs: u64) -> Result<u64, Box<EvalAltResult>> {
    match d {
        0 => Ok(default_secs),
        d if d < 0 => Err(format!("deadline must be a positive number of seconds, got {d}").into()),
        d => Ok(d as u64),
    }
}

pub const DEFAULT_SCHEDULED_DEADLINE_SECS: u64 = 30;

/// Default deadline for `.running()` — same reasoning as `.scheduled()`.
// spec: RT#barriers
pub const DEFAULT_RUNNING_DEADLINE_SECS: u64 = 30;

/// Default deadline for `.ready()`. Bounded by default because an unready
/// resource is usually a deployment bug; callers that legitimately need to
/// wait indefinitely (e.g. Let's Encrypt cert provisioning under rate limit)
/// opt in to `.ready_eventually()`.
// spec: RT#barriers
pub const DEFAULT_READY_DEADLINE_SECS: u64 = 30;

/// Default deadline for `.terminated()`. Sized for long-running jobs (the
/// common caller): 6 hours covers almost every realistic batch workload.
/// Callers that truly have no bound on duration use `.terminated_eventually()`.
// spec: RT#barriers
pub const DEFAULT_TERMINATED_DEADLINE_SECS: u64 = 6 * 3600;

/// Default deadline for `rt.stop()` — stops should settle quickly.
// spec: RT#barriers
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

// spec: IMG#image-discovery
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
// Thread-local: active RuntimeInstance handle for the currently-executing
// action closure. Stashed so `Action.call()` can invoke the called action's
// FnPtr with the same `rt` the outer closure received without threading it
// through the Rhai scope. Set/cleared by `ActionClosureGuard`.
// ---------------------------------------------------------------------------

// spec: ACN#action-invocation
thread_local! {
    static ACTIVE_RT: RefCell<Option<RuntimeInstance>> = const { RefCell::new(None) };
}

pub fn active_rt() -> Option<RuntimeInstance> {
    ACTIVE_RT.with(|cell| cell.borrow().clone())
}

/// Build a [`Breadcrumb`](crate::system::breadcrumb::Breadcrumb) for the
/// active action context and emit it. The app name and script position
/// are pulled from thread-local context that the closure-running scope
/// always sets up; this is a no-op when called outside an action body.
fn emit_breadcrumb(kind: crate::system::breadcrumb::BreadcrumbKind<'_>) {
    let app_arc = action_def().map(|holder| holder.load_full());
    let app_name = app_arc.as_ref().map(|d| &d.name);
    crate::system::breadcrumb::Breadcrumb {
        app: app_name,
        kind,
        // Script positions are wired in a follow-up; once the rt.*
        // registration sites accept NativeCallContext we can pass it
        // through and surface it here.
        script_pos: None,
    }
    .emit();
}

fn set_active_rt(rt: RuntimeInstance) {
    ACTIVE_RT.with(|cell| *cell.borrow_mut() = Some(rt));
}

fn clear_active_rt() {
    ACTIVE_RT.with(|cell| *cell.borrow_mut() = None);
}

// ---------------------------------------------------------------------------
// Thread-local: operation-scoped volume bindings, set for the duration of
// an action closure. Populated by the runtime before invoking an action
// that requires bindings (e.g. one with a `kind: "volume"` param); empty for
// other actions.
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

    // spec: ACN#action-invocation
    /// Stash the active runtime handle so `Action.call(...)` can invoke
    /// sub-actions with the same `rt` the outer closure received.
    pub fn with_active_rt(self, rt: RuntimeInstance) -> Self {
        set_active_rt(rt);
        self
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
        clear_active_rt();
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

// spec: OPS#cancellation
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
// spec: SUSP#positional-replay
// spec: HIST#action-calls
/// `rt.stop` blocks on exactly one barrier, which is therefore the first
/// and only one on its entry.
const STOP_BARRIER_SEQ: usize = 0;

/// Claim this call's position in the action log, returning the index of the
/// entry the call belongs to.
///
/// On a replay pass the committed entry at that position is consumed, and
/// a divergence is reported rather than guessed at: a script edit that puts
/// a different kind of call at this position cannot be replayed from an
/// entry describing the call it replaced, whose barrier record and
/// resources belong to something else. The sub-action surface answered that
/// way already; every `rt.*` surface that records an entry now does.
///
/// On a pass that is making the call for the first time the entry is
/// recorded and `breadcrumb` is emitted, with the context lock released
/// first so it is not held across the journald send.
fn claim_log_position(
    ctx: &SharedContext,
    call_kind: CallKind,
    resources: &[ResourceInstance],
    breadcrumb: impl FnOnce(),
) -> Result<usize, Box<EvalAltResult>> {
    let mut g = ctx.lock();
    let idx = g.call_index;
    match g.replay_step(call_kind, None) {
        Ok(Some(_)) => return Ok(idx),
        Ok(None) => {}
        Err(mismatch) => return Err(Box::<EvalAltResult>::from(mismatch.to_string())),
    }
    g.pending.push(ActionLogEntry {
        call_index: idx,
        call_kind,
        resources: resources.to_vec(),
        barriers: Vec::new(),
        extra: None,
    });
    g.call_index += 1;
    drop(g);
    breadcrumb();
    Ok(idx)
}

// spec: SUSP#barrier-anchoring
/// Record `record` against the log entry of the call a barrier belongs to,
/// reporting whether that entry was found.
///
/// The entry is this pass's own where the pass has made the call, and the
/// committed one where the pass is replaying past it. Correcting a
/// committed entry means re-emitting it, so the copy carries the rest of
/// the entry unchanged and only its barriers differ; synthesising a fresh
/// entry instead would claim a position the committed one already holds and
/// leave every later `rt.*` call reading the wrong one.
///
/// The record goes under the barrier's own position on the call, so the
/// other barriers on it are untouched. A start already recorded for that
/// barrier, for the state being recorded, is kept: that is the start its
/// deadline is measured from, and rewriting it on each pass would restart
/// the wait it measures.
fn record_at_anchor(ctx: &mut ReplayContext, call_index: usize, record: BarrierRecord) -> bool {
    /// The start to record: an earlier one for this same barrier and state,
    /// or the one being written.
    fn started_at(held: Option<&BarrierRecord>, record: &BarrierRecord) -> Option<u64> {
        held.filter(|b| b.required_state == record.required_state)
            .and_then(|b| b.started_at_secs)
            .or(record.started_at_secs)
    }

    // This pass's own entry is already held mutably, so it is corrected in
    // place rather than copied out and back: the copy carried the whole
    // resource list, on every poll of every anchored barrier.
    if let Some(entry) = ctx.pending.iter_mut().find(|e| e.call_index == call_index) {
        let started_at_secs = started_at(entry.barrier(record.seq), &record);
        entry.set_barrier(BarrierRecord {
            started_at_secs,
            ..record
        });
        return true;
    }

    let Some(committed) = ctx.committed.iter().find(|e| e.call_index == call_index) else {
        return false;
    };
    let started_at_secs = started_at(committed.barrier(record.seq), &record);
    let mut corrected = committed.clone();
    corrected.set_barrier(BarrierRecord {
        started_at_secs,
        ..record
    });
    ctx.pending.push(corrected);
    true
}

// spec: COL#query-methods
/// How a `Started`'s resources are matched against another collection: by
/// type and name, which is all a
/// [`ResourceId`](crate::defs::resource::ResourceId) carries. Two instances
/// of one scaled deployment share a key, so naming the deployment selects
/// every replica of it, which is what naming a deployment means.
///
/// Held as names under their type so an instance can be looked up from the
/// name it borrows, without building an owned key per resource tested.
type ResourceKeys = HashMap<crate::defs::resource::ResourceKind, HashSet<String>>;

/// The keys of whatever a collection method was given: another `Started`, a
/// Collection, or a bare resource.
///
/// A `Started` is read through its lock rather than cast out of the
/// `Dynamic`, which would clone every resource it holds, and the warm pairs
/// besides, only to read the names off them and drop the copy.
fn resource_keys(other: Dynamic) -> ResourceKeys {
    let mut keys = ResourceKeys::new();
    if let Some(started) = other.read_lock::<Started>() {
        for r in &started.resources {
            keys.entry(r.kind)
                .or_default()
                .insert(r.name.clone().unwrap_or_default());
        }
        return keys;
    }
    for handle in crate::defs::collection::col(other).resolve() {
        keys.entry(handle.kind())
            .or_default()
            .insert(handle.name().to_owned());
    }
    keys
}

fn keys_hold(keys: &ResourceKeys, instance: &ResourceInstance) -> bool {
    keys.get(&instance.kind)
        .is_some_and(|names| names.contains(instance.name.as_deref().unwrap_or("")))
}

/// The distinct image references in a resource-to-image pairing, in the
/// order they first appear.
///
/// Borrows rather than clones: this runs on every poll of a warm-images
/// barrier, for the life of the wait.
fn dedup_image_refs(pairs: &[(ResourceInstance, String)]) -> Vec<&str> {
    let mut refs: Vec<&str> = Vec::new();
    for (_, img) in pairs {
        if !refs.contains(&img.as_str()) {
            refs.push(img);
        }
    }
    refs
}

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

// spec: RT#handle-availability
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

    // spec: AUTO#scale
    /// How many replicas a named Deployment currently has, as the reconciler
    /// resolves it: the stored scaling decision clamped to the declared
    /// range, or the lower bound where no decision has been stored.
    ///
    /// A read that fails is an error rather than the lower bound. Falling
    /// back would resolve the deployment to fewer replicas than it has, and
    /// a `rt.stop` that silently reached only some of them is exactly the
    /// failure this answers.
    ///
    /// Without a database there are no stored decisions to read, so the
    /// lower bound is the answer rather than a failure.
    ///
    /// The floor of one holds whatever the bounds say, as it did before
    /// effective scale was consulted here. It is not what keeps a barrier
    /// from waiting on nothing — a group holding no resources now returns
    /// at once — so it stands only to leave `scale(0..n)` resolving as it
    /// always has.
    fn effective_scale(&self, deployment: &str, low: u16, high: u16) -> Result<u16, RegistryError> {
        let Some(db) = &self.db else {
            return Ok(low.max(1));
        };
        let app = self.app_name.clone();
        let deployment = deployment.to_owned();
        let effective = db.call(move |db| {
            crate::runtime::scaling::effective_scale(db, &app, &deployment, low, high)
        })?;
        Ok(effective.max(1))
    }

    /// Extract ResourceInstance values from a Rhai Dynamic argument.
    ///
    /// Recognises the concrete resource types registered with the engine and
    /// resolves them through the instance registry so the returned instances
    /// carry stable UUIDs.  Unknown types yield an empty vec (stub behaviour
    /// that leaves barriers trivially satisfied in language-only tests).
    ///
    /// `reuse` says whether a scaled group may resolve to instances that
    /// have already been torn down. Only `rt.start` passes
    /// [`InstanceReuse::Live`]: it is the call that brings replicas up, so
    /// it is the one that must not adopt an instance retirement is about to
    /// delete. Every other call addresses the instances the deployment
    /// already has, and refusing the torn-down ones would have it mint
    /// fresh identities and address those instead.
    fn extract_instances(
        &self,
        resources: Dynamic,
        reuse: InstanceReuse,
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
            // spec: IDN#scaled-instances
            // Named deployments resolve through the scaled-group API: the
            // steady-state reconciler keeps every replica as `is_scaled=1`,
            // so creating a singleton here would only produce a transient
            // placeholder that the next reconciliation tick has to retire.
            // An action that needs to refer to all of them — `rt.signal`,
            // `rt.stop`, `rt.query` — gets every replica back from this
            // single call.
            // spec: AUTO#scale
            // Which is the effective scale, as the reconciler resolves it,
            // and not the low end of the declared range. A deployment
            // declared `scale(2..10)` and scaled to 5 used to yield 2
            // instances here, so a maintenance action stopped 2 of the 5
            // and carried on as though the workload were down while 3 went
            // on serving.
            let scale = {
                let def = dep.def.lock();
                (def.scale.start, def.scale.end)
            };
            let scale = self.effective_scale(&dep.name, scale.0, scale.1)?;
            // spec: GC#instances-started-by-actions
            let group = self.registry.ensure_scaled_group(
                &self.app_name,
                ResourceKind::Deployment,
                Some(&dep.name),
                scale,
                reuse,
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
            // spec: IDN#job-instances
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

        // spec: RT#starting-resources
        // spec: RT#started
        // A Started handle (returned from rt.start / rt.query / etc.)
        // exposes its underlying ResourceInstance set so it can be
        // forwarded to rt.stop / rt.signal / rt.exec without the
        // caller re-deriving the resources from scratch. Without this
        // arm, the fall-through silently produced an empty list and
        // do_stop's "all-terminated" fast path then wrote at
        // call_index-1, clobbering the preceding entry.
        if let Some(started) = resources.clone().try_cast::<Started>() {
            return Ok(started.resources.into_iter().map(|r| (r, None)).collect());
        }

        // spec: RT#starting-resources
        // spec: ACN#action-declaration
        // Action handles are not resources; rejecting here surfaces the
        // mistake as a clear script error instead of silently dropping
        // the call. Action invocation goes through `Action.call(...)`.
        if let Some(action) = resources.clone().try_cast::<crate::defs::action::Action>() {
            return Err(RegistryError::message(format!(
                "rt.start does not invoke actions: pass Action {:?} to .call() instead",
                action.name.as_str()
            )));
        }

        // spec: RT#starting-resources
        // spec: COL#coercion
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
                    out.extend(self.extract_instances(dyn_val, reuse)?);
                }
                return Ok(out);
            }
        }

        // Anything else is a non-instance-bearing value: Volumes,
        // ExternalVolumes, ExternalServices, and other items that
        // legitimately appear in a Collection (e.g. via `col(app)`)
        // but contribute nothing to the action log. Silently skipping
        // them lets `rt.start(app)`, `rt.warm_certs(app)`, etc. accept
        // the App's full Collection without each caller filtering.
        // Direct misuse (`rt.start(some_volume)`) is also accepted as
        // a no-op for symmetry with the pre-Action-call behaviour;
        // the Action arm above catches the one case where silent
        // dropping would mask a real script bug.
        Ok(vec![])
    }

    // spec: RCN#desired-state
    fn do_start(
        &mut self,
        resources_with_defs: Vec<(ResourceInstance, Option<crate::defs::resource::Resource>)>,
    ) -> Result<Started, Box<EvalAltResult>> {
        self.do_start_or_query(resources_with_defs, CallKind::Start)
    }

    // spec: RT#querying-resources
    fn do_query(
        &mut self,
        resources_with_defs: Vec<(ResourceInstance, Option<crate::defs::resource::Resource>)>,
    ) -> Result<Started, Box<EvalAltResult>> {
        self.do_start_or_query(resources_with_defs, CallKind::Query)
    }

    /// Shared implementation behind `rt.start` and `rt.query`. The two
    /// differ only in the `CallKind` they record on the action log
    /// entry and on the breadcrumb, so `do_query` had previously been
    /// implemented as a wrapper that forwarded into `do_start` —
    /// which mislabelled both records as `Start`. Take the kind as a
    /// parameter so each call surface emits its own label.
    fn do_start_or_query(
        &mut self,
        resources_with_defs: Vec<(ResourceInstance, Option<crate::defs::resource::Resource>)>,
        call_kind: CallKind,
    ) -> Result<Started, Box<EvalAltResult>> {
        let resources: Vec<ResourceInstance> =
            resources_with_defs.iter().map(|(r, _)| r.clone()).collect();

        // spec: IMG#image-discovery
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
                origin_index: None,
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
                if let Some(resource) = maybe_def {
                    let instance = instance.clone();
                    let op_id = op_id.clone();
                    // spec: BSL#descriptions
                    let description = resource.description();
                    if let Err(e) = db.call(move |db| {
                        crate::runtime::desired::insert_dynamic_resource(
                            db,
                            &instance,
                            &op_id,
                            description.as_deref(),
                        )
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
                    origin_index: None,
                    warm: WarmMode::None,
                });
            }
            Some(c) => Arc::clone(c),
        };

        let origin_index = claim_log_position(&ctx, call_kind, &resources, || {
            let defs: Vec<Option<&crate::defs::resource::Resource>> = resources_with_defs
                .iter()
                .map(|(_, d)| d.as_ref())
                .collect();
            emit_breadcrumb(match call_kind {
                CallKind::Query => crate::system::breadcrumb::BreadcrumbKind::Query {
                    resources: &resources,
                },
                _ => crate::system::breadcrumb::BreadcrumbKind::Start {
                    resources: &resources,
                    defs: &defs,
                },
            });
        })?;

        Ok(Started {
            ctx: Some(ctx),
            resources,
            origin_index: Some(origin_index),
            warm: WarmMode::None,
        })
    }

    // spec: RT#certificate-warming
    fn do_warm_certs(
        &mut self,
        resources_with_defs: Vec<(ResourceInstance, Option<crate::defs::resource::Resource>)>,
    ) -> Result<Started, Box<EvalAltResult>> {
        // Filter to TLS-terminating ingresses; ignore everything else
        // (per the language spec rule RT#certificate-warming).
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

        // spec: IMG#image-discovery
        // Probe mode: warm_certs has no image content to surface and no
        // relevant side effect for discovery; return an immediate Started.
        if let Some(ctx) = &self.ctx
            && ctx.lock().probe_mode()
        {
            return Ok(Started {
                ctx: Some(Arc::clone(ctx)),
                resources,
                origin_index: None,
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
                    origin_index: None,
                    warm: WarmMode::Certs,
                });
            }
            Some(c) => Arc::clone(c),
        };

        let origin_index = claim_log_position(&ctx, CallKind::WarmCerts, &resources, || {
            emit_breadcrumb(crate::system::breadcrumb::BreadcrumbKind::WarmCerts {
                resources: &resources,
            });
        })?;

        Ok(Started {
            ctx: Some(ctx),
            resources,
            origin_index: Some(origin_index),
            warm: WarmMode::Certs,
        })
    }

    // spec: RT#image-warming
    // spec: IMG#image-warming
    fn do_warm_images(
        &mut self,
        resources_with_defs: Vec<(ResourceInstance, Option<crate::defs::resource::Resource>)>,
    ) -> Result<Started, Box<EvalAltResult>> {
        let pairs = extract_container_images(&resources_with_defs);
        let refs: Vec<String> = dedup_image_refs(&pairs)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let resources: Vec<ResourceInstance> = pairs.iter().map(|(r, _)| r.clone()).collect();

        // spec: IMG#image-discovery
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
                origin_index: None,
                warm: WarmMode::Images(pairs),
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
                    origin_index: None,
                    warm: WarmMode::Images(pairs),
                });
            }
            Some(c) => Arc::clone(c),
        };

        let origin_index = claim_log_position(&ctx, CallKind::WarmImages, &resources, || {
            emit_breadcrumb(crate::system::breadcrumb::BreadcrumbKind::WarmImages { refs: &refs });
        })?;

        Ok(Started {
            ctx: Some(ctx),
            resources,
            origin_index: Some(origin_index),
            warm: WarmMode::Images(pairs),
        })
    }

    // spec: RT#signal-delivery
    // spec: SUSP#side-effecting-calls
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

        // spec: RT#signal-delivery, SUSP#positional-replay
        // At-most-once is per *call site*, not per (resources, signal) value.
        // Scanning the whole committed log for a matching value got both
        // halves wrong: a second, identical rt.signal later in the same
        // closure matched the first entry and was swallowed, so the signal was
        // never delivered; and when the resolved instance set changed between
        // passes — a replica added or retired — nothing matched, so a signal
        // already delivered before the crash was delivered again.
        {
            let mut g = ctx.lock();
            // The signal name is a literal in the script, so unlike the
            // resolved instance set it must match the log.
            match g.replay_step(CallKind::Signal, Some(canonical.as_str())) {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => {}
                Err(mismatch) => return Err(Box::<EvalAltResult>::from(mismatch.to_string())),
            }
        }

        for r in &expanded {
            emit_breadcrumb(crate::system::breadcrumb::BreadcrumbKind::Signal {
                target: r,
                signal: canonical.as_str(),
            });
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
                barriers: Vec::new(),
                extra: Some(canonical.clone()),
            });
            g.call_index += 1;
        }

        Ok(())
    }

    // spec: RT#command-execution
    // spec: SUSP#side-effecting-calls
    /// Run a command inside a running container. Blocks until the command
    /// exits and returns an `Executed` carrying the exit code. Replay-safe:
    /// an entry committed in a prior pass is not re-executed; the recorded
    /// exit code is recovered from the entry. Delivery goes through the
    /// operation's `executor` hook.
    fn do_exec(
        &mut self,
        target: ResourceInstance,
        argv: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<Executed, Box<EvalAltResult>> {
        if argv.is_empty() {
            return Err("rt.exec: argv must be non-empty".into());
        }

        // spec: IMG#image-discovery
        if let Some(ctx) = &self.ctx
            && ctx.lock().probe_mode()
        {
            return Ok(Executed {
                exit_code: 0,
                target_display: Some(target.display_name.clone()),
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
                return Ok(Executed {
                    exit_code: 0,
                    target_display: Some(target.display_name.clone()),
                });
            }
            Some(c) => Arc::clone(c),
        };

        // spec: OPS#cancellation
        if ctx.lock().cancel_token.is_cancelled() {
            return Err(make_cancel_error());
        }

        // spec: SUSP#replay
        // On replay, recover the exit code from the committed entry without
        // re-issuing the exec.
        {
            let mut g = ctx.lock();
            if g.is_replaying() {
                let entry = g.committed_entry().cloned();
                let summary: Vec<String> = g
                    .committed
                    .iter()
                    .map(|e| {
                        format!(
                            "  [{}] {:?} resources={} barrier={} extra={:?}",
                            e.call_index,
                            e.call_kind,
                            e.resources.len(),
                            if e.barriers.is_empty() {
                                "none".to_string()
                            } else {
                                e.barriers
                                    .iter()
                                    .map(|b| {
                                        format!(
                                            "{}:{:?}/sat={}",
                                            b.seq, b.required_state, b.satisfied
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join(",")
                            },
                            e.extra
                        )
                    })
                    .collect();
                g.call_index += 1;
                let entry = entry.ok_or_else(|| -> Box<EvalAltResult> {
                    "rt.exec: replay log entry missing".into()
                })?;
                if entry.call_kind != CallKind::Exec {
                    return Err(format!(
                        "rt.exec: replay log entry mismatch at call_index {}: \
                         expected Exec, found {:?} (resources={}, extra={:?}, \
                         barrier={:?}). Full committed log:\n{}",
                        entry.call_index,
                        entry.call_kind,
                        entry.resources.len(),
                        entry.extra,
                        entry.barriers,
                        summary.join("\n"),
                    )
                    .into());
                }
                let recovered = entry.extra.as_deref().and_then(|s| s.parse::<i32>().ok());
                if let Some(exit_code) = recovered {
                    // Recover the target name from the committed entry so
                    // ensure_success messages name the exact instance even
                    // on a replayed run. Falls back to the live argument
                    // when the entry happens to carry no resources (legacy
                    // entries from before this change).
                    let target_display = entry
                        .resources
                        .first()
                        .map(|r| r.display_name.clone())
                        .or_else(|| Some(target.display_name.clone()));
                    return Ok(Executed {
                        exit_code,
                        target_display,
                    });
                }
                return Err(format!(
                    "rt.exec: replay log entry at call_index {} has no parseable exit code (extra={:?})",
                    entry.call_index, entry.extra
                )
                .into());
            }
        }

        emit_breadcrumb(crate::system::breadcrumb::BreadcrumbKind::Exec {
            target: &target,
            argv: &argv,
        });

        let executor = ctx.lock().executor.clone();
        let exit_code = if let Some(executor) = executor {
            executor
                .exec(&target.display_name, &argv, &env)
                .map_err(|e| -> Box<EvalAltResult> { format!("rt.exec: {e}").into() })?
        } else {
            0
        };

        let target_display = target.display_name.clone();
        {
            let mut g = ctx.lock();
            let idx = g.call_index;
            g.pending.push(ActionLogEntry {
                call_index: idx,
                call_kind: CallKind::Exec,
                resources: vec![target],
                barriers: Vec::new(),
                extra: Some(exit_code.to_string()),
            });
            g.call_index += 1;
        }

        Ok(Executed {
            exit_code,
            target_display: Some(target_display),
        })
    }

    // spec: RT#volume-writes
    // spec: SUSP#side-effecting-calls
    /// Write a file into a volume at action runtime. Replay-safe: an entry
    /// committed in a prior pass is skipped on the next replay so the write
    /// is performed at most once. Resolution of the target volume to a host
    /// path goes through the operation's `volume_writer` hook (set by the
    /// operation loop in `oi/handler/actions/lifecycle.rs`).
    fn do_write(
        &mut self,
        target: VolumeWriteTarget,
        target_resource: ResourceInstance,
        path: &str,
        contents: &[u8],
    ) -> Result<(), Box<EvalAltResult>> {
        validate_volume_write_path(path)?;

        // Probe mode: rt.write has no image content and no discovery-relevant
        // side effect; return immediately without touching the log.
        // spec: IMG#image-discovery
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

        // spec: OPS#cancellation
        if ctx.lock().cancel_token.is_cancelled() {
            return Err(make_cancel_error());
        }

        // spec: SUSP#replay
        // spec: SUSP#positional-replay
        // On replay, skip a write that was already performed at this call
        // site. The entry at this position must be that write: advancing
        // past whatever occupied the slot would take a different call as
        // evidence this write had been done, and skip it. The path is not
        // checked, only the kind, because a path built from the action's
        // params is as much this call's path as a literal one is.
        {
            let mut g = ctx.lock();
            match g.replay_step(CallKind::Write, None) {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => {}
                Err(mismatch) => return Err(Box::<EvalAltResult>::from(mismatch.to_string())),
            }
        }

        emit_breadcrumb(crate::system::breadcrumb::BreadcrumbKind::Write {
            target: &target,
            path,
            len: contents.len(),
        });

        let writer = ctx.lock().volume_writer.clone();
        if let Some(writer) = writer {
            let app = self.app_name.as_str().to_owned();
            writer
                .write(&app, target, path, contents)
                .map_err(|e| -> Box<EvalAltResult> { format!("rt.write: {e}").into() })?;
        }

        {
            let mut g = ctx.lock();
            let idx = g.call_index;
            g.pending.push(ActionLogEntry {
                call_index: idx,
                call_kind: CallKind::Write,
                resources: vec![target_resource],
                barriers: Vec::new(),
                extra: Some(path.to_owned()),
            });
            g.call_index += 1;
        }

        Ok(())
    }

    // spec: SUSP#rtstop
    fn do_stop(
        &mut self,
        resources: Vec<ResourceInstance>,
        deadline_secs: Option<u64>,
    ) -> Result<(), Box<EvalAltResult>> {
        // spec: IMG#image-discovery
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

        // spec: OPS#cancellation
        if ctx.lock().cancel_token.is_cancelled() {
            return Err(make_cancel_error());
        }

        let mut g = ctx.lock();

        // spec: SUSP#replay
        // spec: SUSP#positional-replay
        // The entry at this position must be the stop this call is making.
        // Taking whatever occupied the slot let a satisfied Ready record
        // left there by an rt.start return Ok from a stop that stopped
        // nothing, and otherwise let the stop overwrite that entry on
        // commit. The other rt.* surfaces answer through
        // `claim_log_position`; this one keeps its own path for the
        // started_at it preserves, so it makes the same check itself.
        let recorded = g.committed_entry().map(|e| {
            (
                e.call_kind,
                e.barrier(STOP_BARRIER_SEQ)
                    .is_some_and(|b| b.required_state == LifecycleState::Terminated && b.satisfied),
            )
        });
        if let Some((kind, already_terminated)) = recorded {
            if kind != CallKind::Stop {
                return Err(Box::<EvalAltResult>::from(
                    ReplayMismatch::Kind {
                        call_index: g.call_index,
                        expected: CallKind::Stop,
                        found: kind,
                    }
                    .to_string(),
                ));
            }
            // Fast-skip: the committed entry already records this stop's
            // barrier as satisfied, so advance without re-checking the
            // world.
            if already_terminated {
                g.call_index += 1;
                return Ok(());
            }
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
            // spec: SUSP#deadlines
            // spec: SUSP#rtstop
            // Enforce the stop deadline against the earliest unsatisfied record
            // for these resources. Mirrors the check in check_barrier so
            // rt.stop() participates in the same suspension semantics as the
            // Started barrier methods.
            let started_at = g
                .committed
                .iter()
                .chain(g.pending.iter())
                .find(|e| {
                    e.resources == resources
                        && e.barrier(STOP_BARRIER_SEQ).is_some_and(|b| {
                            b.required_state == LifecycleState::Terminated && !b.satisfied
                        })
                })
                .and_then(|e| e.barrier(STOP_BARRIER_SEQ)?.started_at_secs);
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

        // Single push site shared by fresh and replay paths. The previous
        // implementation pre-advanced call_index in the replay block, then
        // pushed at the post-advance slot in the not-yet-terminated branch,
        // writing to call_index+1 instead of replacing the original entry —
        // leaving an orphan Stop one slot ahead and shifting every later
        // rt.* call's expected position by one. The "fast path" also reused
        // call_index-1 to avoid claiming a new slot, for no documented
        // reason. Both quirks are gone: rt.stop now claims one slot,
        // advances by one, and lets INSERT OR REPLACE overwrite any prior
        // entry at the same call_index on commit.
        let idx = g.call_index;
        // Preserve the original started_at on replay so the deadline counter
        // doesn't reset on every retry.
        let is_first_emit = g.committed.get(idx).is_none();
        let started_at_secs = g
            .committed
            .get(idx)
            .and_then(|e| e.barrier(STOP_BARRIER_SEQ)?.started_at_secs)
            .or(Some(now));
        g.call_index += 1;
        g.pending.push(ActionLogEntry {
            call_index: idx,
            call_kind: CallKind::Stop,
            resources: resources.clone(),
            barriers: vec![BarrierRecord {
                seq: STOP_BARRIER_SEQ,
                required_state: LifecycleState::Terminated,
                deadline_secs,
                satisfied: all_terminated,
                started_at_secs,
            }],
            extra: None,
        });
        // Drop the lock before the breadcrumb's journald send so we
        // don't hold it across an arbitrarily slow syscall. Skip if
        // we're re-entering rt.stop on a replay (already emitted).
        drop(g);
        if is_first_emit {
            emit_breadcrumb(crate::system::breadcrumb::BreadcrumbKind::Stop {
                resources: &resources,
            });
        }

        if all_terminated {
            Ok(())
        } else {
            let condition = BarrierCondition {
                resources,
                required_state: LifecycleState::Terminated,
                deadline_secs,
            };
            // Re-acquire the lock to set pending_barrier, since we
            // dropped it above for the breadcrumb. The make_barrier_error
            // helper expects the condition to be set on the ctx.
            ctx.lock().pending_barrier = Some(condition.clone());
            Err(make_barrier_error(condition))
        }
    }
}

// spec: RT
impl CustomType for RuntimeInstance {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("RuntimeInstance")
            // spec: RT#deployment-restart
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
                    // spec: IMG#image-discovery
                    if let Some(ctx) = &this.ctx
                        && ctx.lock().probe_mode()
                    {
                        return Ok(());
                    }
                    emit_breadcrumb(crate::system::breadcrumb::BreadcrumbKind::Restart {
                        deployment: &dep_name,
                    });
                    if let Some(db) = &this.db {
                        let app_name = this.app_name.clone();
                        let dep_name_for_db = dep_name.clone();
                        db.call(move |db| {
                            restart_gens::bump_restart_gen(db, &app_name, &dep_name_for_db)
                        })
                        .map_err(|e| -> Box<EvalAltResult> {
                            format!("rt.restart db error: {e}").into()
                        })?;
                    }
                    Ok(())
                },
            )
            // spec: RT#starting-resources
            .with_fn(
                "start",
                |this: &mut Self, resources: Dynamic| -> Result<Started, Box<EvalAltResult>> {
                    // spec: GC#instances-started-by-actions
                    // The call that brings replicas up is the one that must
                    // not adopt an instance retirement is about to delete.
                    let resources_with_defs = this
                        .extract_instances(resources, InstanceReuse::Live)
                        .map_err(|e| {
                            Box::new(EvalAltResult::ErrorRuntime(
                                e.to_string().into(),
                                rhai::Position::NONE,
                            ))
                        })?;
                    this.do_start(resources_with_defs)
                },
            )
            // spec: RT#stopping-resources
            .with_fn(
                "stop",
                |this: &mut Self, resources: Dynamic| -> Result<(), Box<EvalAltResult>> {
                    let instances = this
                        .extract_instances(resources, InstanceReuse::Any)
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
                        .extract_instances(resources, InstanceReuse::Any)
                        .map_err(|e| {
                            Box::new(EvalAltResult::ErrorRuntime(
                                e.to_string().into(),
                                rhai::Position::NONE,
                            ))
                        })?
                        .into_iter()
                        .map(|(r, _)| r)
                        .collect();
                    this.do_stop(
                        instances,
                        Some(resolve_deadline(deadline, DEFAULT_STOP_DEADLINE_SECS)?),
                    )
                },
            )
            // spec: RT#querying-resources
            .with_fn(
                "query",
                |this: &mut Self, resources: Dynamic| -> Result<Started, Box<EvalAltResult>> {
                    let resources_with_defs = this
                        .extract_instances(resources, InstanceReuse::Any)
                        .map_err(|e| {
                            Box::new(EvalAltResult::ErrorRuntime(
                                e.to_string().into(),
                                rhai::Position::NONE,
                            ))
                        })?;
                    this.do_query(resources_with_defs)
                },
            )
            // spec: RT#certificate-warming
            .with_fn(
                "warm_certs",
                |this: &mut Self, resources: Dynamic| -> Result<Started, Box<EvalAltResult>> {
                    let resources_with_defs = this
                        .extract_instances(resources, InstanceReuse::Any)
                        .map_err(|e| {
                            Box::new(EvalAltResult::ErrorRuntime(
                                e.to_string().into(),
                                rhai::Position::NONE,
                            ))
                        })?;
                    this.do_warm_certs(resources_with_defs)
                },
            )
            // spec: RT#image-warming
            .with_fn(
                "warm_images",
                |this: &mut Self, resources: Dynamic| -> Result<Started, Box<EvalAltResult>> {
                    let resources_with_defs = this
                        .extract_instances(resources, InstanceReuse::Any)
                        .map_err(|e| {
                            Box::new(EvalAltResult::ErrorRuntime(
                                e.to_string().into(),
                                rhai::Position::NONE,
                            ))
                        })?;
                    this.do_warm_images(resources_with_defs)
                },
            )
            // spec: RT#signal-delivery
            .with_fn(
                "signal",
                |this: &mut Self,
                 target: Dynamic,
                 signal: &str|
                 -> Result<(), Box<EvalAltResult>> {
                    let resources_with_defs = this
                        .extract_instances(target, InstanceReuse::Any)
                        .map_err(|e| {
                            Box::new(EvalAltResult::ErrorRuntime(
                                e.to_string().into(),
                                rhai::Position::NONE,
                            ))
                        })?;
                    let resources: Vec<ResourceInstance> =
                        resources_with_defs.into_iter().map(|(r, _)| r).collect();
                    this.do_signal(resources, signal)
                },
            )
            // spec: RT#volume-writes
            .with_fn(
                "write",
                |this: &mut Self,
                 target: Dynamic,
                 path: &str,
                 contents: &str|
                 -> Result<(), Box<EvalAltResult>> {
                    if !is_in_action_closure() {
                        return Err("rt.write may only be called inside an action closure".into());
                    }
                    let (write_target, instance) = resolve_volume_write_target(this, target)?;
                    this.do_write(write_target, instance, path, contents.as_bytes())
                },
            )
            // spec: RT#volume-writes
            .with_fn(
                "write",
                |this: &mut Self,
                 target: Dynamic,
                 path: &str,
                 file: crate::defs::file::File|
                 -> Result<(), Box<EvalAltResult>> {
                    if !is_in_action_closure() {
                        return Err("rt.write may only be called inside an action closure".into());
                    }
                    let (write_target, instance) = resolve_volume_write_target(this, target)?;
                    this.do_write(write_target, instance, path, &file.contents)
                },
            )
            // spec: RT#command-execution
            .with_fn(
                "exec",
                |this: &mut Self,
                 target: Dynamic,
                 args: rhai::Array|
                 -> Result<Executed, Box<EvalAltResult>> {
                    if !is_in_action_closure() {
                        return Err("rt.exec may only be called inside an action closure".into());
                    }
                    let argv = exec_argv_from_array(args)?;
                    let instance = resolve_exec_target(this, target)?;
                    this.do_exec(instance, argv, Vec::new())
                },
            )
            .with_fn(
                "exec",
                |this: &mut Self,
                 target: Dynamic,
                 args: rhai::Array,
                 options: Map|
                 -> Result<Executed, Box<EvalAltResult>> {
                    if !is_in_action_closure() {
                        return Err("rt.exec may only be called inside an action closure".into());
                    }
                    let argv = exec_argv_from_array(args)?;
                    let env = parse_exec_options(options)?;
                    let instance = resolve_exec_target(this, target)?;
                    this.do_exec(instance, argv, env)
                },
            );
    }
}

// spec: RT#command-execution
fn exec_argv_from_array(args: rhai::Array) -> Result<Vec<String>, Box<EvalAltResult>> {
    if args.is_empty() {
        return Err("rt.exec: argv must be non-empty".into());
    }
    let mut out = Vec::with_capacity(args.len());
    for v in args {
        let s = v.into_string().map_err(|t| -> Box<EvalAltResult> {
            format!("rt.exec: argv entry must be a string, got {t}").into()
        })?;
        out.push(s);
    }
    Ok(out)
}

// spec: RT#command-execution
fn parse_exec_options(options: Map) -> Result<Vec<(String, String)>, Box<EvalAltResult>> {
    let mut env = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "env" => {
                let env_map = value
                    .try_cast::<Map>()
                    .ok_or_else(|| -> Box<EvalAltResult> {
                        "rt.exec: options.env must be a map".into()
                    })?;
                for (k, v) in env_map {
                    let k_str = k.to_string();
                    if !is_valid_env_var_name(&k_str) {
                        return Err(format!(
                            "rt.exec: invalid env var name '{k_str}'; must match [A-Za-z_][A-Za-z0-9_]*"
                        )
                        .into());
                    }
                    let v_str = v.into_string().map_err(|t| -> Box<EvalAltResult> {
                        format!("rt.exec: env value for '{k_str}' must be a string, got {t}").into()
                    })?;
                    env.push((k_str, v_str));
                }
            }
            other => {
                return Err(format!("rt.exec: unknown option '{other}' (supported: env)").into());
            }
        }
    }
    Ok(env)
}

fn is_valid_env_var_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// spec: RT#command-execution
/// Resolve a BSL `Started`, `Deployment`, or `Job` argument to the single
/// container instance the exec should run in. Rejects targets that resolve
/// to zero or more than one instance, or that are anonymous and have not
/// been started (those must be passed via the `Started` returned by
/// `rt.start`).
fn resolve_exec_target(
    rt: &RuntimeInstance,
    target: Dynamic,
) -> Result<ResourceInstance, Box<EvalAltResult>> {
    use crate::defs::deployment::Deployment;
    use crate::defs::job::Job;
    use crate::defs::resource::ResourceKind;

    if let Some(started) = target.clone().try_cast::<Started>() {
        if started.resources.len() != 1 {
            return Err(format!(
                "rt.exec: target Started must carry exactly one container instance, got {}",
                started.resources.len()
            )
            .into());
        }
        let r = started.resources.into_iter().next().unwrap();
        if !matches!(r.kind, ResourceKind::Deployment | ResourceKind::Job) {
            return Err(format!(
                "rt.exec: target must be a Deployment or Job, got {:?}",
                r.kind
            )
            .into());
        }
        return Ok(r);
    }

    if let Some(dep) = target.clone().try_cast::<Deployment>() {
        if dep.name.as_str().is_empty() {
            return Err("rt.exec: anonymous deployments must be passed via the Started returned by rt.start, not directly".into());
        }
        let scale = dep.def.lock().scale.clone();
        if scale.end > 1 {
            return Err(format!(
                "rt.exec: deployment '{}' has scale upper bound {}; rt.exec requires scale 1 (so the target is unambiguous)",
                dep.name, scale.end
            )
            .into());
        }
        let with_defs = rt
            .extract_instances(Dynamic::from(dep), InstanceReuse::Any)
            .map_err(|e| -> Box<EvalAltResult> { format!("rt.exec: {e}").into() })?;
        let mut iter = with_defs.into_iter();
        let first = iter.next().ok_or_else(|| -> Box<EvalAltResult> {
            "rt.exec: target deployment resolved to no instances".into()
        })?;
        if iter.next().is_some() {
            return Err("rt.exec: target deployment resolved to more than one instance".into());
        }
        return Ok(first.0);
    }

    if let Some(job) = target.try_cast::<Job>() {
        if job.name.as_str().is_empty() {
            return Err("rt.exec: anonymous jobs must be passed via the Started returned by rt.start, not directly".into());
        }
        let with_defs = rt
            .extract_instances(Dynamic::from(job), InstanceReuse::Any)
            .map_err(|e| -> Box<EvalAltResult> { format!("rt.exec: {e}").into() })?;
        let mut iter = with_defs.into_iter();
        let first = iter.next().ok_or_else(|| -> Box<EvalAltResult> {
            "rt.exec: target job resolved to no instances".into()
        })?;
        if iter.next().is_some() {
            return Err("rt.exec: target job resolved to more than one instance".into());
        }
        return Ok(first.0);
    }

    Err("rt.exec: target must be a Started, Deployment, or Job".into())
}

// spec: RT#volume-writes
/// Resolve a BSL `Volume` or `ExternalVolume` argument into a runtime
/// [`VolumeWriteTarget`] plus the [`ResourceInstance`] used for log keying.
fn resolve_volume_write_target(
    rt: &RuntimeInstance,
    target: Dynamic,
) -> Result<(VolumeWriteTarget, ResourceInstance), Box<EvalAltResult>> {
    use crate::defs::resource::ResourceKind;
    use crate::runtime::identity::{InstanceId, InstanceVariant};

    if let Some(vol) = target.clone().try_cast::<Volume>() {
        let tmpfs = vol.def.lock().tmpfs;
        if let Some(name) = vol.name.as_deref() {
            let display = crate::runtime::identity::VolumeName::for_app(rt.app_name.as_str(), name);
            let instance =
                ResourceInstance::new_singleton(rt.app_name.clone(), ResourceKind::Volume, name);
            let _ = display;
            return Ok((
                VolumeWriteTarget::NamedVolume {
                    name: name.to_owned(),
                    tmpfs,
                },
                instance,
            ));
        }
        if let Some(anon_id) = vol.anon_id.as_deref() {
            let id = InstanceId(uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                anon_id.as_bytes(),
            ));
            let instance = ResourceInstance {
                id,
                app: rt.app_name.clone(),
                kind: ResourceKind::Volume,
                name: None,
                variant: InstanceVariant::Singleton,
                display_name: anon_id.to_owned(),
            };
            return Ok((
                VolumeWriteTarget::AnonymousVolume {
                    anon_id: anon_id.to_owned(),
                    tmpfs,
                },
                instance,
            ));
        }
        return Err("rt.write: volume has neither a name nor an anon_id".into());
    }

    if let Some(ext) = target.try_cast::<ExternalVolume>() {
        let binding = ext.operation_binding.ok_or_else(|| -> Box<EvalAltResult> {
            "rt.write: external volume has no operation-scoped binding; rt.write is only supported for operation-bound external volumes".into()
        })?;
        let instance = ResourceInstance::new_singleton(
            rt.app_name.clone(),
            ResourceKind::ExternalVolume,
            ext.name.as_str(),
        );
        return Ok((
            VolumeWriteTarget::ExternalBound {
                host_path: binding.host_path,
            },
            instance,
        ));
    }

    Err("rt.write: target must be a Volume or ExternalVolume".into())
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
    // spec: RT#certificate-warming
    Certs,
    /// `rt.warm_images` returned this `Started`. `.ready()` waits until each
    /// image reference is present in local container storage.
    /// Each image reference the warm covers, with the resource it came
    /// from. Keeping the pairing rather than the bare references is what
    /// lets a [`Started`] that has been narrowed by a collection method
    /// narrow what its barrier waits on: the references alone could not say
    /// which resource had been selected away.
    // spec: RT#image-warming
    Images(Vec<(ResourceInstance, String)>),
}

// spec: RT#started
#[derive(Debug, Clone)]
pub struct Started {
    pub ctx: Option<SharedContext>,
    pub resources: Vec<ResourceInstance>,
    pub warm: WarmMode,
    // spec: SUSP#barrier-anchoring
    /// The `call_index` of the log entry the originating `rt.*` call
    /// produced. A barrier called on this `Started` is decided from that
    /// entry and no other.
    ///
    /// `None` for a `Started` that has no entry behind it — a stub context
    /// or probe mode — where there is nothing to anchor to and barriers
    /// resolve immediately anyway. Collection methods carry the anchor
    /// through, so `rt.start(x).except(y).ready()` is still the `rt.start`
    /// call's barrier.
    pub origin_index: Option<usize>,
}

impl Started {
    // spec: RT#started
    /// This `Started` over `kept` alone.
    ///
    /// The anchor carries through, so a barrier on the narrowed group is
    /// still the originating `rt.*` call's barrier: narrowing selects which
    /// resources are waited on, it does not make a new call. A warm-images
    /// group narrows what its barrier waits on too, since the references it
    /// checks are held against the resources they came from.
    fn narrowed_to(&self, keep: impl Fn(&ResourceInstance) -> bool) -> Self {
        // One predicate decides both. Filtering the pairs by membership of
        // the kept resources instead would rescan that list for every pair.
        let warm = match &self.warm {
            WarmMode::Images(pairs) => {
                WarmMode::Images(pairs.iter().filter(|(r, _)| keep(r)).cloned().collect())
            }
            other => other.clone(),
        };
        Self {
            ctx: self.ctx.clone(),
            resources: self.resources.iter().filter(|r| keep(r)).cloned().collect(),
            origin_index: self.origin_index,
            warm,
        }
    }

    // spec: SUSP#barrier-anchoring
    /// The entry this barrier's record belongs to.
    ///
    /// A group a collection method narrowed uses the entry of the call it
    /// came from, as the whole group does. The two do not collide: each
    /// barrier holds its record under its own position on that call, so a
    /// narrowed one neither takes the broader one's passing for its own nor
    /// displaces the start the broader one is timed from.
    fn anchor(&self) -> Option<usize> {
        self.origin_index
    }

    // spec: SUSP#barriers
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

        // spec: IMG#image-discovery
        // Probe mode: every barrier resolves immediately.
        if ctx.lock().probe_mode() {
            let _ = (required, deadline_secs);
            return Ok(self.clone());
        }

        // spec: OPS#cancellation
        // Cooperatively honour a cancel request before doing any more work.
        if ctx.lock().cancel_token.is_cancelled() {
            return Err(make_cancel_error());
        }

        let mut g = ctx.lock();
        let now = (g.now_secs)();

        // spec: SUSP#barrier-anchoring
        // Which barrier on its call this is. Drawn afresh each pass, in the
        // order the closure asserts them, so the same barrier draws the
        // same number every time and reads back its own record.
        let anchor = self.anchor();
        let seq = anchor.map_or(0, |idx| g.next_barrier_seq(idx));

        // spec: SUSP#replay
        // spec: SUSP#barrier-anchoring
        // Ask the entry of the call this barrier was called on, and no
        // other. Matching by resources and required state instead let a
        // `.terminated()` barrier short-circuit against the record an
        // earlier `rt.stop` over the same resources had left satisfied —
        // `do_stop` being the one writer of a satisfied Terminated record.
        // A named job resolves to one operation-scoped instance UUID, so
        // the two calls carry byte-identical resources and the scan could
        // not tell them apart.
        let already_satisfied = anchor
            .and_then(|idx| g.committed.iter().find(|e| e.call_index == idx))
            .and_then(|e| e.barrier(seq))
            .is_some_and(|b| b.required_state == required && b.satisfied);
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
            // spec: RT#certificate-warming
            // An empty resource list here means every element was filtered
            // out (e.g. no TLS-terminating ingresses in the selection); the
            // language spec says such a `Started` is immediately satisfied.
            (WarmMode::Certs, LifecycleState::Ready) => {
                self.resources.iter().all(|r| g.world.cert_valid_for(r))
            }
            // spec: RT#image-warming
            // spec: IMG#image-warming
            // Empty `refs` means the selection contained no container
            // resources with an image to warm — immediately satisfied.
            (WarmMode::Images(pairs), LifecycleState::Ready) => {
                pairs.iter().all(|(_, image)| g.world.image_present(image))
            }
            // spec: RT#barriers
            // A group holding no resources has nothing to wait on. The warm
            // variants above already answered that way; this arm did not,
            // so a group a collection method had narrowed down to nothing
            // would have waited forever rather than returning at once.
            _ => self
                .resources
                .iter()
                .all(|r| g.world.lifecycle_state(r).has_reached(required)),
        };

        // spec: SUSP#barriers
        // Always consult the oracle BEFORE the deadline check: a barrier that
        // is currently satisfied must succeed even if more than `deadline_secs`
        // have elapsed since the original suspension. The committed log can
        // lag the world (e.g. a short-lived job that completed during a
        // replay where the satisfied=true update never landed in committed),
        // so deadline-then-oracle would spuriously time out a barrier whose
        // resources have actually reached the required state.
        if all_reached {
            // spec: SUSP#barrier-anchoring
            // Record the pass against this barrier's own entry. Marking
            // pending entries only lost it on a replay pass, where pending
            // is empty until the closure makes its first fresh rt.* call:
            // the committed record kept satisfied=false and its original
            // started_at forever. `earliest_unsatisfied_barrier_wait_secs`
            // takes the maximum elapsed across unsatisfied records, so one
            // long wait that had since passed went on inflating the
            // re-poll cadence of every barrier after it.
            if let Some(idx) = anchor {
                record_at_anchor(
                    &mut g,
                    idx,
                    BarrierRecord {
                        seq,
                        required_state: required,
                        deadline_secs,
                        satisfied: true,
                        started_at_secs: Some(now),
                    },
                );
            }
            return Ok(self.clone());
        }

        // Not yet reached — check deadline: find when we first started
        // waiting. The wait belongs to this call, so it is read off this
        // call's own entry.
        // spec: SUSP#barrier-anchoring
        let started_at = anchor
            .and_then(|idx| {
                g.pending
                    .iter()
                    .chain(g.committed.iter())
                    .find(|e| e.call_index == idx)
            })
            .and_then(|e| e.barrier(seq))
            .filter(|b| b.required_state == required && !b.satisfied)
            .and_then(|b| b.started_at_secs);

        // spec: SUSP#deadlines
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

        // The barrier records the start its deadline is measured from
        // under its own position on its call's entry.
        // spec: SUSP#barrier-anchoring
        let waiting = BarrierRecord {
            seq,
            required_state: required,
            deadline_secs,
            satisfied: false,
            started_at_secs: Some(now),
        };
        let anchored = anchor.is_some_and(|idx| record_at_anchor(&mut g, idx, waiting));
        // Only a barrier with no entry behind it at all synthesises one.
        if !anchored {
            // No entry behind this barrier: the closure asserted it on a
            // resource set never tracked via rt.start. The language spec
            // does not promise that works, but it is given an entry rather
            // than silently going unrecorded.
            let idx = g.call_index;
            g.call_index += 1;
            g.pending.push(ActionLogEntry {
                call_index: idx,
                call_kind: CallKind::Start,
                resources: self.resources.clone(),
                barriers: vec![BarrierRecord {
                    seq,
                    required_state: required,
                    deadline_secs,
                    satisfied: false,
                    started_at_secs: Some(now),
                }],
                extra: None,
            });
        }
        g.pending_barrier = Some(condition.clone());
        drop(g);
        Err(make_barrier_error(condition))
    }

    // spec: RT#termination
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
            return Termination {
                success: true,
                failed: Vec::new(),
            };
        };
        // spec: IMG#image-discovery
        // Probe mode: pretend every terminated resource succeeded. We
        // accept that this misses error-path image references (code
        // guarded by `termination.ensure_success()` throwing); catching
        // those would require speculatively executing both branches.
        if ctx.lock().probe_mode() {
            return Termination {
                success: true,
                failed: Vec::new(),
            };
        }
        let world = Arc::clone(&ctx.lock().world);
        let failed: Vec<String> = self
            .resources
            .iter()
            .filter(|r| !world.termination_success(r).unwrap_or(false))
            .map(|r| r.display_name.clone())
            .collect();
        Termination {
            success: failed.is_empty(),
            failed,
        }
    }
}

impl CustomType for Started {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Started")
            // spec: RT#barriers
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
                    this.check_barrier(
                        LifecycleState::Scheduled,
                        Some(resolve_deadline(d, DEFAULT_SCHEDULED_DEADLINE_SECS)?),
                    )
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
                    this.check_barrier(
                        LifecycleState::Running,
                        Some(resolve_deadline(d, DEFAULT_RUNNING_DEADLINE_SECS)?),
                    )
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
                    this.check_barrier(
                        LifecycleState::Ready,
                        Some(resolve_deadline(d, DEFAULT_READY_DEADLINE_SECS)?),
                    )
                },
            )
            // spec: RT#barriers
            .with_fn(
                "ready_eventually",
                |this: &mut Self| -> Result<Started, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Ready, None)
                },
            )
            // spec: RT#barriers
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
                    this.check_barrier(
                        LifecycleState::Terminated,
                        Some(resolve_deadline(d, DEFAULT_TERMINATED_DEADLINE_SECS)?),
                    )?;
                    Ok(this.compute_termination())
                },
            )
            // spec: RT#barriers
            .with_fn(
                "terminated_eventually",
                |this: &mut Self| -> Result<Termination, Box<EvalAltResult>> {
                    this.check_barrier(LifecycleState::Terminated, None)?;
                    Ok(this.compute_termination())
                },
            )
            // spec: RT#started
            // Collection's methods on a Started return Starteds over the
            // selected resources. These used to ignore their arguments and
            // return the whole group, so
            // `rt.start(app).except(app.deployment("optional")).ready()`
            // went on waiting for the resource the author had excluded.
            // spec: COL#query-methods
            .with_fn("one", |this: &mut Self| -> Dynamic {
                match this.resources.first().map(|r| r.id) {
                    Some(first) => Dynamic::from(this.narrowed_to(|r| r.id == first)),
                    // `collection.one` is null on an empty collection, and
                    // a Started over no resources is worse than null here:
                    // its barriers would have nothing to wait on.
                    None => Dynamic::UNIT,
                }
            })
            // spec: COL#query-methods
            .with_fn("only", |this: &mut Self, other: Dynamic| -> Self {
                let keys = resource_keys(other);
                this.narrowed_to(|r| keys_hold(&keys, r))
            })
            // spec: COL#query-methods
            .with_fn("except", |this: &mut Self, other: Dynamic| -> Self {
                let keys = resource_keys(other);
                this.narrowed_to(|r| !keys_hold(&keys, r))
            })
            // spec: COL#selection
            .with_fn(
                "select",
                |this: &mut Self, criterion: Map| -> Result<Self, Box<EvalAltResult>> {
                    let selector = Selector::from_map(&criterion)?;
                    Ok(this.narrowed_to(|r| {
                        selector.matches_resource(r.kind, r.name.as_deref().unwrap_or(""))
                    }))
                },
            );
    }
}

// ---------------------------------------------------------------------------
// Termination
// ---------------------------------------------------------------------------

// spec: RT#termination
#[derive(Debug, Clone)]
pub struct Termination {
    pub success: bool,
    /// Display names of the resources that failed (or whose outcome the
    /// world oracle could not determine). Surfaced by `ensure_success`
    /// so the operator can grep `apps logs` / `--instance` for the
    /// specific container without scanning the whole app's stream.
    pub failed: Vec<String>,
}

impl CustomType for Termination {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Termination")
            // spec: RT#termination
            .with_fn(
                "ensure_success",
                |this: &mut Self| -> Result<(), Box<EvalAltResult>> {
                    if this.success {
                        Ok(())
                    } else {
                        let detail = if this.failed.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", this.failed.join(", "))
                        };
                        Err(Box::new(EvalAltResult::ErrorRuntime(
                            format!(
                                "rt.start.terminated: resource did not terminate successfully{detail}"
                            )
                            .into(),
                            rhai::Position::NONE,
                        )))
                    }
                },
            );
    }
}

// ---------------------------------------------------------------------------
// Executed
// ---------------------------------------------------------------------------

// spec: RT#executed
#[derive(Debug, Clone)]
pub struct Executed {
    pub exit_code: i32,
    /// Display name of the container the command ran in. Surfaced by
    /// `ensure_success` so the error names the exact instance an
    /// operator can pull logs for.
    pub target_display: Option<String>,
}

/// Symbolic name for an exit code, when one is widely known. Used in
/// `ensure_success` error messages so an operator can interpret a
/// code like `127` without reaching for documentation. The format
/// mirrors systemctl's `<code>/<NAME>` convention.
///
/// POSIX/bash conventions are listed inline (six entries) and
/// signal-terminated processes (128+N) defer to `nix` so the table
/// stays in lockstep with libc. systemd's LSB codes (200..255) are
/// out of scope here — they are set by exec failure paths in the
/// init manager, not by anything `rt.exec` invokes.
fn exit_code_name(code: i32) -> Option<String> {
    let bash = match code {
        0 => Some("SUCCESS"),
        1 => Some("FAILURE"),
        2 => Some("INVALIDARGUMENT"),
        126 => Some("NOTEXECUTABLE"),
        127 => Some("NOTFOUND"),
        128 => Some("INVALIDEXIT"),
        _ => None,
    };
    if let Some(s) = bash {
        return Some(s.to_owned());
    }

    // Signal-terminated: shell convention is 128 + signal number.
    if (129..=128 + 64).contains(&code)
        && let Ok(sig) = nix::sys::signal::Signal::try_from(code - 128)
    {
        return Some(sig.as_str().to_owned());
    }

    None
}

/// Format an exit code as `<code>` or `<code>/<NAME>` when known.
fn format_exit_code(code: i32) -> String {
    match exit_code_name(code) {
        Some(name) => format!("{code}/{name}"),
        None => code.to_string(),
    }
}

impl CustomType for Executed {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Executed")
            // spec: RT#executed
            .with_fn("exit_code", |this: &mut Self| -> i64 {
                this.exit_code as i64
            })
            // spec: RT#executed
            .with_fn("success", |this: &mut Self| -> bool { this.exit_code == 0 })
            // spec: RT#executed
            .with_fn(
                "ensure_success",
                |this: &mut Self| -> Result<(), Box<EvalAltResult>> {
                    if this.exit_code == 0 {
                        Ok(())
                    } else {
                        let target = this
                            .target_display
                            .as_deref()
                            .map(|n| format!(" in {n}"))
                            .unwrap_or_default();
                        Err(Box::new(EvalAltResult::ErrorRuntime(
                            format!(
                                "rt.exec command failed with exit code {}{target}",
                                format_exit_code(this.exit_code)
                            )
                            .into(),
                            rhai::Position::NONE,
                        )))
                    }
                },
            );
    }
}

#[cfg(test)]
mod exit_code_tests {
    use super::format_exit_code;

    #[test]
    fn well_known_codes_get_a_symbolic_name() {
        assert_eq!(format_exit_code(0), "0/SUCCESS");
        assert_eq!(format_exit_code(1), "1/FAILURE");
        assert_eq!(format_exit_code(127), "127/NOTFOUND");
        assert_eq!(format_exit_code(137), "137/SIGKILL");
        assert_eq!(format_exit_code(143), "143/SIGTERM");
    }

    #[test]
    fn unknown_codes_print_the_bare_number() {
        assert_eq!(format_exit_code(3), "3");
        assert_eq!(format_exit_code(42), "42");
        assert_eq!(format_exit_code(217), "217");
        assert_eq!(format_exit_code(255), "255");
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_READY_DEADLINE_SECS, DEFAULT_TERMINATED_DEADLINE_SECS, resolve_deadline};

    // spec: RT#barriers
    // Zero used to become `Some(0)`, which the `elapsed >= d` check trips on
    // the very next pass — so `ready(0)`, written expecting the documented
    // default, failed about two seconds later instead of waiting 30.
    #[test]
    fn zero_means_the_states_default() {
        assert_eq!(
            resolve_deadline(0, DEFAULT_READY_DEADLINE_SECS).unwrap(),
            DEFAULT_READY_DEADLINE_SECS
        );
        assert_eq!(
            resolve_deadline(0, DEFAULT_TERMINATED_DEADLINE_SECS).unwrap(),
            DEFAULT_TERMINATED_DEADLINE_SECS
        );
    }

    // spec: RT#barriers
    #[test]
    fn a_positive_deadline_is_taken_as_given() {
        assert_eq!(
            resolve_deadline(45, DEFAULT_READY_DEADLINE_SECS).unwrap(),
            45
        );
    }

    // spec: RT#barriers
    // The spec says the argument must be positive; clamping a negative to
    // zero would silently mean "fail immediately".
    #[test]
    fn a_negative_deadline_is_refused() {
        let err = resolve_deadline(-5, DEFAULT_READY_DEADLINE_SECS)
            .expect_err("a negative is not a deadline");
        assert!(err.to_string().contains("-5"), "should name it: {err}");
    }
}
