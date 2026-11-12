use std::{collections::HashMap, sync::Arc};

use serde_json::json;

use crate::{
    oi::state::OiState,
    runtime::{apps::status as app_status, faults},
    system::types::ContainerStatus,
};

use super::HandlerResult;

/// Whether any of the component's containers is running, or `None` when a
/// container could not be inspected and none was seen running.
async fn component_status(state: &Arc<OiState>, containers: &[&str]) -> Option<&'static str> {
    let mut undetermined = false;
    for name in containers {
        match state.container_runtime.inspect(name).await {
            Ok(Some(s)) if matches!(s.status, ContainerStatus::Running) => return Some("running"),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(container = %name, error = %e, "could not inspect an infrastructure container");
                undetermined = true;
            }
        }
    }
    if undetermined { None } else { Some("stopped") }
}

/// Liveness probe. Clients wanting to confirm the daemon is reachable and
/// responsive use this instead of /server/status — it never touches the
/// container runtime, never allocates a registry read lock, and returns the
/// same shape regardless of daemon state.
// spec: STS#liveness
pub(crate) fn ping() -> HandlerResult {
    Ok(json!({}))
}

// spec: STS#infrastructure-status
pub(crate) async fn get_infra_status(state: &Arc<OiState>) -> HandlerResult {
    let proxy = component_status(state, &["seedling-caddy-blue", "seedling-caddy-green"]).await;
    let resolver = component_status(
        state,
        &["seedling-resolver-blue", "seedling-resolver-green"],
    )
    .await;
    Ok(json!({ "proxy": proxy, "resolver": resolver }))
}

// spec: STS#server-status
pub(crate) fn get_status(state: &OiState) -> HandlerResult {
    let uptime = state.start_time.elapsed().as_secs();
    let hostname = whoami::devicename()
        .or_else(|_| whoami::hostname())
        .unwrap_or_else(|_| "unknown".into());
    // A status that cannot be derived leaves both counts undetermined, rather
    // than reporting fewer apps than there are.
    let apps = match app_status::all(&state.registry, &state.scheduler, &state.db) {
        Ok(apps) => Some(apps),
        Err(e) => {
            tracing::warn!(error = %e, "could not read the apps' status for the server status");
            None
        }
    };
    let apps_total = apps.as_ref().map(Vec::len);
    let apps_by_status = apps.as_ref().map(|apps| {
        let mut by_status: HashMap<&'static str, usize> = HashMap::new();
        for (_, status) in apps {
            *by_status.entry(status.name()).or_insert(0) += 1;
        }
        by_status
    });

    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),        "hostname": hostname,
        "uptime_secs": uptime,
        "spki_fingerprint": state.spki_fingerprint.get().cloned().unwrap_or_default(),
        "apps_total": apps_total,
        "apps_by_status": apps_by_status,
        // spec: STS#server-status — the literal 0 here reported no operation in
        // progress however busy the runtime was. One operation is active at a
        // time; queued ones are waiting, not in progress, so they are not
        // counted.
        "active_operations": usize::from(state.scheduler.lock().active().is_some()),
        "active_faults": state
            .db
            .call(faults::count_active_faults)
            .map_err(|e| tracing::warn!(error = %e, "could not count the active faults"))
            .ok(),
        "active_shells": state.shells.list(None).len(),
        "active_forwards": state.forwards.lock().count(),
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::oi::test_support::TestOi;

    // spec: STS#server-status
    #[test]
    fn a_member_that_cannot_be_read_is_null_not_zero_or_empty() {
        let oi = TestOi::with_app("demo");
        oi.install("demo");
        oi.state
            .db
            .call(|db| db.conn.execute_batch("DROP TABLE faults;").unwrap());

        let status = oi.call("/server/status", json!({})).unwrap();
        assert_eq!(status["active_faults"], json!(null));
        assert_eq!(status["apps_total"], json!(null));
        assert_eq!(status["apps_by_status"], json!(null));
        assert_eq!(
            status["active_shells"],
            json!(0),
            "a member that can be read is still reported"
        );
    }

    // spec: STS#infrastructure-status
    #[test]
    fn a_component_that_cannot_be_inspected_is_null_not_stopped() {
        let oi = TestOi::with_unreadable_containers();
        let status = oi.block_on(super::get_infra_status(&oi.state)).unwrap();
        assert_eq!(status["proxy"], json!(null));
        assert_eq!(status["resolver"], json!(null));

        let healthy = TestOi::new();
        let status = healthy
            .block_on(super::get_infra_status(&healthy.state))
            .unwrap();
        assert_eq!(status["proxy"], json!("stopped"));
    }
}
