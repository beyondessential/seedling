use std::sync::Arc;

use seedling_protocol::names::AppName;

use crate::{oi::state::OiState, runtime::AppPhase};

/// Flip the app's phase in memory and persist the new row. Never hold the
/// registry write lock across the db call: the schedule ticker holds db then
/// acquires registry.read(), so registry.write() + db.lock() in either order
/// deadlocks with it.
// spec: EVT#event-types
pub fn set_phase_and_persist(
    state: &OiState,
    app_name: &AppName,
    new_phase: AppPhase,
    actor: Option<Arc<seedling_protocol::actor::Actor>>,
) {
    use crate::oi::handler::apps::{extract_persist_fields, persist_app_fields};
    let phase_str = phase_event_name(&new_phase);
    // spec: FWD#forward-lifetime
    // spec: SHL#session-end
    if !matches!(new_phase, AppPhase::Installed) {
        state.end_sessions_of_app(app_name.as_str());
    }
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(app_name.as_str()) {
            *entry.phase.lock() = new_phase;
        }
    }
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(app_name.as_str()) {
            let (app_n, generation_n, installed, uninstalling) = extract_persist_fields(entry);
            if let Err(e) = state.db.call(move |db| {
                persist_app_fields(db, &app_n, generation_n, installed, uninstalling)
            }) {
                tracing::error!(app = %app_name, "persist phase transition: {e}");
            }
        }
    }
    state.event_tx.app_phase_changed(app_name, phase_str, actor);
    state.tick_notify.notify_one();
}

fn phase_event_name(phase: &AppPhase) -> &'static str {
    match phase {
        AppPhase::NotInstalled => "not_installed",
        AppPhase::Installed => "installed",
        AppPhase::Uninstalling => "uninstalling",
    }
}
