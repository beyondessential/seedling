use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rhai::{CustomType, FnPtr, Map, TypeBuilder};
use seedling_protocol::names::{ActionName, AppName, ParamName, ShellName};

use super::{
    Holder,
    action::{ActionDef, ShellDef},
    install::{InstallDef, ParamDef},
    resource::{Resource, ResourceId},
};

mod action;
mod collection;
mod deployment;
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
// Thread-local action-call table
// ---------------------------------------------------------------------------

// l[impl action.lookup]
// l[impl action.call]
// r[impl operation.composition]
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

// l[impl action.schedule]
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

// l[impl app.resources]
// l[impl app.resources.names]
#[derive(Debug, Default, Clone)]
pub struct AppDef {
    pub name: AppName,
    /// Free-form description of the application, set via `app.description()`.
    // l[impl app.description]
    pub description: Option<String>,
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
}

fn extract_description(options: &Map) -> Option<String> {
    options
        .get("description")
        .and_then(|v| v.clone().into_string().ok())
}

// ---------------------------------------------------------------------------
// App — the BSL-facing handle; !Send (Rc inside), stays on the BSL thread
// ---------------------------------------------------------------------------

// l[impl app.type]
// l[impl app.constructor]
#[derive(Clone)]
pub struct App {
    pub def: Arc<arc_swap::ArcSwap<AppDef>>,
    /// Operator-provided parameter values, pre-populated from the database before
    /// script evaluation. Not BSL-driven — the script cannot modify this directly.
    pub stored: Holder<BTreeMap<String, String>>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            def: Arc::new(arc_swap::ArcSwap::new(Arc::new(AppDef::default()))),
            stored: Arc::new(parking_lot::Mutex::new(BTreeMap::new())),
        }
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("def", &self.def)
            .field("stored", &self.stored)
            .finish_non_exhaustive()
    }
}

// l[impl app.methods]
impl CustomType for App {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("App");

        // l[impl app.description]
        builder.with_fn("description", |this: &mut App, desc: &str| -> App {
            let desc_owned = desc.to_owned();
            this.def.rcu(|d| {
                let mut d = (**d).clone();
                d.description = Some(desc_owned.clone());
                d
            });
            this.clone()
        });

        param::on_app(&mut builder);
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
