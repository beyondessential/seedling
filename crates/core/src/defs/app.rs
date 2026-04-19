use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use rhai::{CustomType, FnPtr, Map, TypeBuilder};

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
    pub actions: BTreeMap<String, FnPtr>,
    pub shells: BTreeMap<String, FnPtr>,
    pub install: Option<FnPtr>,
    pub param_changes: BTreeMap<String, FnPtr>,
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
pub(crate) fn capture_param_change(name: String, fnptr: FnPtr) {
    CLOSURE_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.param_changes.insert(name, fnptr);
        }
    });
}

fn capture_action(name: String, fnptr: FnPtr) {
    CLOSURE_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.actions.insert(name, fnptr);
        }
    });
}

fn capture_shell(name: String, fnptr: FnPtr) {
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

thread_local! {
    static APPDEF_HOLDER: RefCell<Option<Holder<AppDef>>> = const { RefCell::new(None) };
}

pub(crate) fn set_appdef_holder(holder: &Holder<AppDef>) {
    APPDEF_HOLDER.with(|h| *h.borrow_mut() = Some(holder.clone()));
}

pub(crate) fn clear_appdef_holder() {
    APPDEF_HOLDER.with(|h| *h.borrow_mut() = None);
}

// l[impl action.schedule]
pub(crate) fn append_action_schedule(action_name: &str, expr: &str) {
    APPDEF_HOLDER.with(|h| {
        if let Some(ref holder) = *h.borrow() {
            let mut def = holder.lock();
            if let Some(action_def) = def.actions.get_mut(action_name) {
                action_def.schedules.push(expr.to_owned());
            }
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
    pub name: String,
    /// Parameters declared by the BSL script via `app.param()`, with optional schema metadata.
    pub params: BTreeMap<String, ParamDef>,
    pub resources: BTreeMap<ResourceId, Resource>,
    /// Action metadata (name, description). No FnPtrs — closures are
    /// recovered on demand via the thread-local capture buffer.
    pub actions: BTreeMap<String, ActionDef>,
    pub shells: BTreeMap<String, ShellDef>,
    pub install: Option<InstallDef>,
    /// Names of parameters that have an `on_change` handler registered.
    pub param_changes: BTreeSet<String>,
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
#[derive(Clone, Default)]
pub struct App {
    pub def: Holder<AppDef>,
    /// Operator-provided parameter values, pre-populated from the database before
    /// script evaluation. Not BSL-driven — the script cannot modify this directly.
    pub stored: Holder<BTreeMap<String, String>>,
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
