use std::collections::HashMap;

use serde_json::json;

use crate::{oi::state::OiState, runtime::faults};

use super::HandlerResult;

// i[status.get]
pub(crate) fn get_status(state: &OiState) -> HandlerResult {
    let uptime = state.start_time.elapsed().as_secs();
    let hostname = whoami::devicename()
        .or_else(|_| whoami::hostname())
        .unwrap_or_else(|_| "unknown".into());
    let reg = state.registry.read();
    let apps = reg.list();
    let apps_total = apps.len();
    let mut apps_by_status: HashMap<&'static str, usize> = HashMap::new();
    for (_, status) in &apps {
        *apps_by_status.entry(status.name()).or_insert(0) += 1;
    }

    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "hostname": hostname,
        "uptime_secs": uptime,
        "spki_fingerprint": state.spki_fingerprint.get().cloned().unwrap_or_default(),
        "apps_total": apps_total,
        "apps_by_status": apps_by_status,
        "active_operations": 0,
        "active_faults": state.db.call(|db| faults::count_active_faults(db).unwrap_or(0)),
        "active_shells": state.shells.list(None).len(),
        "active_forwards": state.forwards.lock().count(),
    }))
}
