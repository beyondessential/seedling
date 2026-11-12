use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rhai::{CustomType, Dynamic, FnPtr, Map, TypeBuilder};
use seedling_protocol::names::{ActionName, AppName, ParamName, ShellName};

use crate::runtime::definition::Bundle;

use super::{
    Holder,
    action::{ActionDef, ShellDef},
    install::{InstallDef, ParamDef},
    resource::{Resource, ResourceId, ResourceKind},
};

mod action;
mod collection;
mod deployment;
mod file;
mod install;
mod job;
mod param;
mod service;
mod shell;
mod volume;

// ---------------------------------------------------------------------------
// Thread-local closure capture buffer
// ---------------------------------------------------------------------------

/// Closures registered by the BSL script during a single re-run in
/// `run_operation`. Never stored persistently — activated on demand, consumed
/// immediately after the re-run, then discarded.
#[derive(Default)]
pub(crate) struct ClosureCapture {
    pub actions: BTreeMap<ActionName, FnPtr>,
    pub shells: BTreeMap<ShellName, FnPtr>,
    pub install: Option<FnPtr>,
    pub param_changes: BTreeMap<ParamName, FnPtr>,
}

thread_local! {
    static CLOSURE_CAPTURE: RefCell<Option<ClosureCapture>> = const { RefCell::new(None) };
}

/// Activate the closure capture buffer on this thread. While active, every
/// `on_start`, `on_action`, `on_shell`, `on_install`, and `param.on_change`
/// call will push its `FnPtr` into the buffer in addition to writing metadata
/// into `AppDef`. Has no effect (and causes no allocation) when not active.
pub(crate) fn begin_closure_capture() {
    CLOSURE_CAPTURE.with(|c| *c.borrow_mut() = Some(ClosureCapture::default()));
}

/// Deactivate the buffer and return whatever was captured. Must be called
/// exactly once after `begin_closure_capture`, even if the script run fails.
pub(crate) fn end_closure_capture() -> ClosureCapture {
    CLOSURE_CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default())
}

/// Called by `param.on_change` — writes the FnPtr into the active buffer if
/// one exists, otherwise silently discards it.
pub(crate) fn capture_param_change(name: ParamName, fnptr: FnPtr) {
    CLOSURE_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.param_changes.insert(name, fnptr);
        }
    });
}

fn capture_action(name: ActionName, fnptr: FnPtr) {
    CLOSURE_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.actions.insert(name, fnptr);
        }
    });
}

fn capture_shell(name: ShellName, fnptr: FnPtr) {
    CLOSURE_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.shells.insert(name, fnptr);
        }
    });
}

fn capture_install(fnptr: FnPtr) {
    CLOSURE_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.install = Some(fnptr);
        }
    });
}

// ---------------------------------------------------------------------------
// Thread-local validator capture
// ---------------------------------------------------------------------------

thread_local! {
    static VALIDATOR_CAPTURE: RefCell<Option<BTreeMap<ParamName, FnPtr>>> = const { RefCell::new(None) };
    static IN_VALIDATOR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Start collecting `param.validate` closures on this thread. Evaluations
/// that will not run validators leave the buffer inactive, and the closures
/// are dropped.
pub(crate) fn begin_validator_capture() {
    VALIDATOR_CAPTURE.with(|c| *c.borrow_mut() = Some(BTreeMap::new()));
}

/// Stop collecting and return what was captured. Must be called once after
/// `begin_validator_capture`, whether or not the script run succeeded.
pub(crate) fn end_validator_capture() -> BTreeMap<ParamName, FnPtr> {
    VALIDATOR_CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default())
}

pub(crate) fn capture_validator(name: ParamName, fnptr: FnPtr) {
    VALIDATOR_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.insert(name, fnptr);
        }
    });
}

/// Marks the thread as running a validator for as long as the guard lives,
/// so that resource definitions throw.
// spec: PARAM#validators
pub(crate) struct ValidatorFrame(());

impl ValidatorFrame {
    pub fn enter() -> Self {
        IN_VALIDATOR.with(|v| v.set(true));
        Self(())
    }
}

impl Drop for ValidatorFrame {
    fn drop(&mut self) {
        IN_VALIDATOR.with(|v| v.set(false));
    }
}

/// Refuse a resource definition made from within a validator.
// spec: PARAM#validators
pub(crate) fn ensure_not_validating() -> Result<(), Box<rhai::EvalAltResult>> {
    if IN_VALIDATOR.with(|v| v.get()) {
        return Err("a validator must not define resources".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Static resource references
// ---------------------------------------------------------------------------

/// A resource type that `app.<kind>(name)` hands back as a frozen reference in
/// the action context.
///
/// One implementation per named resource type, so [`static_ref`] can do the
/// lookup for all of them rather than each method repeating it.
pub(crate) trait StaticResource: Clone {
    /// The kind this type is filed under in [`AppDef::resources`].
    const KIND: ResourceKind;

    /// The resource as this type, when the entry holds this variant.
    fn from_resource(resource: &Resource) -> Option<&Self>;

    /// Mark the handle frozen, which every static reference in the action
    /// context is.
    // spec: APP#static-resource-immutability
    fn freeze(&mut self);
}

/// The static resource `name` of type `T` in `app`, as a frozen reference.
///
/// Every named-resource method resolves the same way in the action context, so
/// the lookup lives here rather than in each of them: one diagnosis for a name
/// that is not there, and nothing for the next resource type to copy. It reads
/// the definition of the `App` it was called on, so `old` in a change handler
/// answers from the previous generation rather than the current one.
// spec: APP#named-resources
// spec: PARAM#change-handlers
pub(crate) fn static_ref<T: StaticResource>(
    app: &App,
    name: &str,
) -> Result<T, Box<rhai::EvalAltResult>> {
    let def = app.def.load();
    let id = ResourceId {
        kind: T::KIND,
        name: Arc::new(name.to_owned()),
    };
    let Some(resource) = def.resources.get(&id) else {
        return Err(no_static_resource(&def, T::KIND, name));
    };
    let mut handle = T::from_resource(resource)
        .ok_or_else(|| -> Box<rhai::EvalAltResult> {
            format!(
                "internal: '{name}' is filed as {} but holds a {}",
                noun(T::KIND),
                noun(resource.kind())
            )
            .into()
        })?
        .clone();
    handle.freeze();
    Ok(handle)
}

/// The error for a name that is not a static resource of `kind`.
///
/// `resources` is keyed by kind and name together, so a name declared as
/// another kind misses the lookup exactly as one never declared does. Saying
/// which kind holds it is the difference between a one-line fix and a hunt for
/// a typo that was never made.
fn no_static_resource(def: &AppDef, kind: ResourceKind, name: &str) -> Box<rhai::EvalAltResult> {
    let held_by = def
        .resources
        .keys()
        .find(|id| id.name.as_str() == name)
        .map(|id| id.kind);
    match held_by {
        Some(other) => format!(
            "no static {} named '{name}': it is a {}",
            noun(kind),
            noun(other)
        ),
        None => format!("no static {} named '{name}'", noun(kind)),
    }
    .into()
}

/// How a resource kind is named to a script author.
///
/// Separate from the kind slugs the runtime persists and parses, which are
/// written for machines and must not change under them.
fn noun(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Parameter => "parameter",
        ResourceKind::Service => "service",
        ResourceKind::Ingress => "ingress",
        ResourceKind::Deployment => "deployment",
        ResourceKind::Job => "job",
        ResourceKind::Volume => "volume",
        ResourceKind::ExternalVolume => "external volume",
        ResourceKind::ExternalService => "external service",
        ResourceKind::Action => "action",
    }
}

// ---------------------------------------------------------------------------
// Thread-local action-call table
// ---------------------------------------------------------------------------

// spec: ACN#action-handles
// spec: ACN#action-invocation
// spec: OPS#action-composition
/// Per-operation lookup table for `Action.call`. Populated by `replay.rs`
/// around the call to `engine.eval_ast_with_scope(...)` so that
/// `app.action(name)` and `Action.call(...)` can find the captured FnPtrs
/// for actions defined in the script. The stack supports the no-recursion
/// guarantee: each `.call()` pushes its action name on entry and pops on
/// exit (via [`SubActionFrame`] for exception safety), so a cycle is
/// rejected before the closure runs.
pub(crate) struct ActionCallTable {
    pub actions: BTreeMap<ActionName, FnPtr>,
    pub stack: Vec<ActionName>,
}

thread_local! {
    static ACTION_CALL_TABLE: RefCell<Option<ActionCallTable>> = const { RefCell::new(None) };
}

/// Install an `ActionCallTable` for the duration of the closure passed to
/// `f`. The outer action's name seeds the call stack so `.call(<self>)`
/// inside it is rejected as recursion.
pub(crate) fn with_action_call_table<R>(
    actions: BTreeMap<ActionName, FnPtr>,
    outer: ActionName,
    f: impl FnOnce() -> R,
) -> R {
    let table = ActionCallTable {
        actions,
        stack: vec![outer],
    };
    ACTION_CALL_TABLE.with(|t| *t.borrow_mut() = Some(table));
    let result = f();
    ACTION_CALL_TABLE.with(|t| t.borrow_mut().take());
    result
}

/// Look up the FnPtr for `name` in the active call table. Returns
/// `Ok(None)` when no table is active (which is itself a script error
/// that the caller surfaces as "must be called inside an action").
pub(crate) fn action_call_lookup(name: &ActionName) -> Result<Option<FnPtr>, &'static str> {
    ACTION_CALL_TABLE.with(|t| match t.borrow().as_ref() {
        None => Err("no active action call table"),
        Some(table) => Ok(table.actions.get(name).cloned()),
    })
}

/// Snapshot the active call stack for cycle reporting.
pub(crate) fn action_call_stack() -> Option<Vec<ActionName>> {
    ACTION_CALL_TABLE.with(|t| t.borrow().as_ref().map(|table| table.stack.clone()))
}

/// RAII guard that pushes an action name on the active call stack and
/// pops it when dropped, even if the body panics or the closure
/// propagates an exception. Returns an error if the table is not
/// active (caller must already have rejected this case).
pub(crate) struct SubActionFrame {
    name: ActionName,
}

impl SubActionFrame {
    pub fn enter(name: ActionName) -> Self {
        ACTION_CALL_TABLE.with(|t| {
            if let Some(table) = t.borrow_mut().as_mut() {
                table.stack.push(name.clone());
            }
        });
        Self { name }
    }
}

impl Drop for SubActionFrame {
    fn drop(&mut self) {
        ACTION_CALL_TABLE.with(|t| {
            if let Some(table) = t.borrow_mut().as_mut() {
                if let Some(top) = table.stack.last() {
                    debug_assert_eq!(
                        top, &self.name,
                        "action call stack imbalance: expected {:?} on top, found {:?}",
                        self.name, top
                    );
                }
                table.stack.pop();
            }
        });
    }
}

thread_local! {
    static APPDEF_HOLDER: RefCell<Option<Arc<arc_swap::ArcSwap<AppDef>>>> = const { RefCell::new(None) };
}

pub(crate) fn set_appdef_holder(holder: &Arc<arc_swap::ArcSwap<AppDef>>) {
    APPDEF_HOLDER.with(|h| *h.borrow_mut() = Some(holder.clone()));
}

pub(crate) fn clear_appdef_holder() {
    APPDEF_HOLDER.with(|h| *h.borrow_mut() = None);
}

// spec: ACN#scheduled-actions
pub(crate) fn append_action_schedule(action_name: &ActionName, expr: &str) {
    APPDEF_HOLDER.with(|h| {
        if let Some(ref holder) = *h.borrow() {
            holder.rcu(|d| {
                let mut d = (**d).clone();
                if let Some(action_def) = d.actions.get_mut(action_name.as_str()) {
                    action_def.schedules.push(expr.to_owned());
                }
                d
            });
        }
    });
}

// ---------------------------------------------------------------------------
// AppDef — Send, shared with the Reconciler
// ---------------------------------------------------------------------------

// spec: APP#named-resources
#[derive(Debug, Default, Clone)]
pub struct AppDef {
    pub name: AppName,
    /// Free-form description of the application, set via `app.description()`.
    // spec: APP#app-description
    pub description: Option<String>,
    /// Canopy application type, set via `app.canopy_type()`.
    pub canopy_type: Option<String>,
    /// Parameters declared by the BSL script via `app.param()`, with optional schema metadata.
    pub params: BTreeMap<ParamName, ParamDef>,
    pub resources: BTreeMap<ResourceId, Resource>,
    /// Action metadata (name, description). No FnPtrs — closures are
    /// recovered on demand via the thread-local capture buffer.
    pub actions: BTreeMap<ActionName, ActionDef>,
    pub shells: BTreeMap<ShellName, ShellDef>,
    pub install: Option<InstallDef>,
    /// Names of parameters that have an `on_change` handler registered.
    pub param_changes: BTreeSet<ParamName>,
    /// Names of parameters that have a validator attached.
    // spec: PARAM#validators
    pub validators: BTreeSet<ParamName>,
}

fn extract_description(options: &Map) -> Result<Option<String>, Box<rhai::EvalAltResult>> {
    options
        .get("description")
        .cloned()
        .map(|v| crate::defs::take::take_string("description", v))
        .transpose()
}

/// Whether `value` is a type Canopy accepts: lowercase letters and digits in
/// hyphen-separated segments, beginning with a letter.
fn is_valid_canopy_type(value: &str) -> bool {
    value.split('-').all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    }) && value.starts_with(|c: char| c.is_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// App — the BSL-facing handle; !Send (Rc inside), stays on the BSL thread
// ---------------------------------------------------------------------------

// spec: APP#the-app-variable
#[derive(Clone)]
pub struct App {
    pub def: Arc<arc_swap::ArcSwap<AppDef>>,
    /// Operator-provided parameter values, pre-populated from the database before
    /// script evaluation. Not BSL-driven — the script cannot modify this directly.
    pub stored: Holder<BTreeMap<String, String>>,
    /// The definition bundle this app was evaluated from, read by
    /// `app.file` and `app.dir`.
    // spec: BND#bundle-files
    pub bundle: Arc<Bundle>,
}

impl App {
    pub fn with_bundle(bundle: Arc<Bundle>) -> Self {
        Self {
            def: Arc::new(arc_swap::ArcSwap::new(Arc::new(AppDef::default()))),
            stored: Arc::new(parking_lot::Mutex::new(BTreeMap::new())),
            bundle,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::with_bundle(Arc::new(Bundle::empty()))
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("def", &self.def)
            .field("stored", &self.stored)
            .field("bundle", &self.bundle.hash())
            .finish_non_exhaustive()
    }
}

// spec: APP#the-app-variable
impl CustomType for App {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("App");

        // spec: APP#app-description
        builder.with_fn("description", |this: &mut App, desc: &str| -> App {
            let desc_owned = desc.to_owned();
            this.def.rcu(|d| {
                let mut d = (**d).clone();
                d.description = Some(desc_owned.clone());
                d
            });
            this.clone()
        });

        // spec: APP#canopy-type
        builder.with_fn(
            "canopy_type",
            |this: &mut App, canopy_type: Dynamic| -> Result<App, Box<rhai::EvalAltResult>> {
                if crate::runtime::barrier::runtime::is_in_action_closure() {
                    return Err("canopy_type cannot be called from within an action closure".into());
                }
                let canopy_type = crate::defs::take::take_string("canopy_type", canopy_type)?;
                if !is_valid_canopy_type(&canopy_type) {
                    return Err(format!(
                        "canopy_type {canopy_type:?} must be lowercase letters and digits in \
                         hyphen-separated segments, beginning with a letter"
                    )
                    .into());
                }
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.canopy_type = Some(canopy_type.clone());
                    d
                });
                Ok(this.clone())
            },
        );

        param::on_app(&mut builder);
        file::on_app(&mut builder);
        service::on_app(&mut builder);
        deployment::on_app(&mut builder);
        job::on_app(&mut builder);
        volume::on_app(&mut builder);
        collection::on_app(&mut builder);
        action::on_app(&mut builder);
        shell::on_app(&mut builder);
        install::on_app(&mut builder);
    }
}
