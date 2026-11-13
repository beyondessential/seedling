pub mod handler;
pub mod registry;
pub mod session;
pub mod target;

pub use registry::{
    Delivery, ForwardEntry, ForwardProto, ForwardRecord, ForwardRegistry, ForwardStatus,
};

use crate::oi::state::OiState;

// spec: FWD#forward-lifetime
/// End every forward of `app` whose service, or port for its proto, the app's
/// definition no longer declares, and leave the others running.
///
/// Called wherever a definition takes effect for an app: an update, and a
/// parameter change, which re-evaluates it.
pub(crate) fn end_undeclared_forwards(state: &OiState, app: &str) {
    // Collected before the forwards lock is taken: that lock is on the
    // per-datagram path, and the walk takes a lock per pod that a
    // concurrent script evaluation may hold.
    let declared = state
        .registry
        .read()
        .get(app)
        .map(|e| target::declared_ports(&e.app.def.load()));
    let stale = state.forwards.lock().remove_stale_for_app(app, |fwd| {
        declared.as_ref().is_none_or(|declared| {
            !declared.contains(&(fwd.service.clone(), fwd.port, fwd.proto.clone()))
        })
    });
    for entry in stale {
        let _ = entry.stop_tx.send(true);
    }
}

// spec: FWD#forward-lifetime
/// End every forward of `app`, as when it leaves `Installed`.
pub(crate) fn end_forwards_of_app(state: &OiState, app: &str) {
    let all = state.forwards.lock().remove_stale_for_app(app, |_| true);
    for entry in all {
        let _ = entry.stop_tx.send(true);
    }
}

#[cfg(test)]
mod tests;
