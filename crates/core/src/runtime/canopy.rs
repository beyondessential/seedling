//! Reporting Seedling's own health to Canopy.
//!
//! Seedling reports as a source of its own, distinct from any other agent on the
//! host, so its checks and the issues Canopy derives from them are scoped to it
//! alone and neither source can close the other's issues.
//!
//! Nothing here dials out. Reporting happens only while a connected client is
//! offering to carry the requests, which is why a host without one has no
//! reporting task doing anything, nothing retrying, and no fault to explain.

use std::{collections::HashMap, sync::Arc, time::Duration};

use bestool_canopy::CanopyClient;
use jiff::Timestamp;
use seedling_protocol::names::AppName;
use serde_json::{Value, json};

use crate::{
    oi::{canopy::OiCanopyTransport, state::OiState},
    runtime::{apps::AppStatus, faults},
    system::types::ContainerStatus,
};

pub mod store;

pub use store::{CanopySettings, get_settings, set_enabled, set_server_id};

// r[impl canopy.report.schedule]
/// How often a report is sent.
///
/// Matches the cadence Canopy already receives from the other agent on a host,
/// so nothing about its staleness detection has to accommodate a new rate.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(60);

/// The source name Seedling reports under.
const SOURCE: &str = "seedling";

/// Fault kind filed when a report fails while a provider is available.
const FAULT_KIND: &str = "canopy_report_failed";

/// The app a instance-wide fault is attributed to, as used elsewhere for
/// conditions that belong to the daemon rather than to a workload.
fn seedling_app() -> AppName {
    AppName::new_unchecked("seedling")
}

/// The outcome of a report attempt, as surfaced through `/canopy/status`.
#[derive(Debug, Clone)]
pub struct LastReport {
    pub at: Timestamp,
    pub ok: bool,
    pub error: Option<String>,
}

/// What one turn of the reporting loop did.
#[derive(Debug)]
enum Outcome {
    /// Canopy access is off, or nothing is offering to carry the request. Not a
    /// failure: there is nothing to report through and nothing wrong.
    Skipped,
    Reported,
    Failed(String),
}

// r[impl canopy.report.schedule]
/// Run the reporting loop until the process ends.
pub fn spawn(state: Arc<OiState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(REPORT_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let _ = report(&state).await;
        }
    })
}

// r[impl canopy.report.schedule]
// r[impl canopy.report.fault]
/// Attempt one report, filing or clearing the fault to match.
///
/// Returns the error text on failure so an operator-invoked report can report it
/// back rather than only leaving it in a fault.
pub async fn report(state: &Arc<OiState>) -> Result<(), String> {
    match report_once(state).await {
        Outcome::Skipped => {
            // Nothing is expected of an instance with no provider, so a fault
            // left over from when there was one would misdescribe it.
            clear_fault(state);
            Ok(())
        }
        Outcome::Reported => {
            record(state, true, None);
            clear_fault(state);
            Ok(())
        }
        Outcome::Failed(err) => {
            record(state, false, Some(err.clone()));
            // Logging and filing together: an error worth a log line here is
            // one an operator should be able to see in the fault surface.
            tracing::error!("canopy report failed: {err}");
            let message = err.clone();
            state.db.call(move |db| {
                // Replace rather than accumulate: successive failures of the
                // same condition are one fault, with the latest detail.
                let _ = faults::clear_faults_by_kind(db, &seedling_app(), FAULT_KIND);
                let _ =
                    faults::file_fault(db, &seedling_app(), None, None, None, FAULT_KIND, &message);
            });
            Err(err)
        }
    }
}

async fn report_once(state: &Arc<OiState>) -> Outcome {
    if !state.canopy.is_enabled() {
        return Outcome::Skipped;
    }
    if state.canopy.current().is_none() {
        return Outcome::Skipped;
    }

    let client = CanopyClient::with_transport(OiCanopyTransport::new(Arc::clone(&state.canopy)));

    let server_id = match server_id(state, &client).await {
        Ok(id) => id,
        Err(e) => return Outcome::Failed(e),
    };

    let payload = build_payload(state).await;
    match client.status(&server_id, &payload).await {
        // r[impl canopy.report.backup-prompt] — the response carries return-path
        // instructions, including backups to run now. That list is addressed to
        // whichever source owns backups on a host, which is never this one, so it
        // arrives empty and is discarded rather than being read as "nothing to
        // do" and acted on.
        Ok(_) => Outcome::Reported,
        Err(e) => {
            // The cached identity is the most likely thing to be wrong when a
            // report is rejected, so drop it and let the next attempt re-resolve
            // rather than retrying the same bad address forever.
            state.db.call(|db| {
                let _ = store::set_server_id(db, None);
            });
            Outcome::Failed(format!("{e}"))
        }
    }
}

// r[impl canopy.report.identity]
/// The server identifier Canopy knows this instance by, from cache or resolved.
async fn server_id(
    state: &Arc<OiState>,
    client: &CanopyClient<OiCanopyTransport>,
) -> Result<String, String> {
    let cached = state
        .db
        .call(|db| store::get_settings(db).ok().and_then(|s| s.server_id));
    if let Some(id) = cached {
        return Ok(id);
    }

    let resolved = client
        .servers_self()
        .await
        .map_err(|e| format!("cannot resolve which server this instance is: {e}"))?;
    let id = resolved.server_id.to_string();
    let to_store = id.clone();
    state.db.call(move |db| {
        let _ = store::set_server_id(db, Some(&to_store));
    });
    Ok(id)
}

/// A check result as Canopy grades it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Result_ {
    Passed,
    Warning,
    Failed,
    /// The check itself could not be evaluated.
    Broken,
}

impl Result_ {
    fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Warning => "warning",
            Self::Failed => "failed",
            Self::Broken => "broken",
        }
    }
}

fn check(name: &str, result: Result_, extra: Value) -> Value {
    let mut entry = json!({ "check": name, "result": result.as_str() });
    if let (Some(entry), Some(extra)) = (entry.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            entry.insert(k.clone(), v.clone());
        }
    }
    entry
}

// r[impl canopy.report.checks]
// r[impl canopy.report.extra]
async fn build_payload(state: &Arc<OiState>) -> Value {
    let (apps_check, apps_total, apps_by_status) = apps_check(state);
    let faults_check = faults_check(state);
    let proxy = component_check(
        state,
        "health/proxy",
        &["seedling-caddy-blue", "seedling-caddy-green"],
    )
    .await;
    let resolver = component_check(
        state,
        "health/resolver",
        &["seedling-resolver-blue", "seedling-resolver-green"],
    )
    .await;

    let health = vec![apps_check, faults_check, proxy, resolver];
    // Overall health is recorded for display but is not what Canopy grades
    // incidents from — the per-check results are. A broken check is not itself
    // unhealthy: it says the truth is unknown, not that it is bad.
    let healthy = health.iter().all(|c| {
        !matches!(
            c.get("result").and_then(Value::as_str),
            Some("failed") | Some("warning")
        )
    });

    let active_faults = state
        .db
        .call(|db| faults::count_active_faults(db).unwrap_or(0));

    json!({
        "source": SOURCE,
        "healthy": healthy,
        "health": health,
        // r[impl canopy.report.extra] — what describes Seedling, and nothing
        // that describes the host: the hostname, the host's uptime, and any
        // managed application's version belong to the agent that owns them.
        "seedlingVersion": env!("CARGO_PKG_VERSION"),
        "seedlingUptimeSecs": state.start_time.elapsed().as_secs(),
        "appsTotal": apps_total,
        "appsByStatus": apps_by_status,
        "activeOperations": active_operations(state),
        "activeFaults": active_faults,
    })
}

// r[impl canopy.report.checks]
/// `health/apps`, plus the app totals the free-form payload also carries.
fn apps_check(state: &Arc<OiState>) -> (Value, usize, HashMap<&'static str, usize>) {
    let reg = state.registry.read();
    let apps = reg.list();
    let total = apps.len();

    let mut by_status: HashMap<&'static str, usize> = HashMap::new();
    let mut degraded = Vec::new();
    let mut faulted = Vec::new();
    for (name, status) in &apps {
        *by_status.entry(status.name()).or_insert(0) += 1;
        match status {
            AppStatus::Degraded => degraded.push(name.to_string()),
            AppStatus::Faulted => faulted.push(name.to_string()),
            // Transitional states are not a health signal: an app mid-install
            // or mid-operation is not yet supposed to be running.
            _ => {}
        }
    }
    drop(reg);

    let result = if !faulted.is_empty() {
        Result_::Failed
    } else if !degraded.is_empty() {
        Result_::Warning
    } else {
        Result_::Passed
    };

    // Naming the apps responsible means an operator reading the report can act
    // on it without coming back to ask which ones.
    let entry = check(
        "health/apps",
        result,
        json!({ "degraded": degraded, "faulted": faulted, "total": total }),
    );
    (entry, total, by_status)
}

// r[impl canopy.report.checks]
fn faults_check(state: &Arc<OiState>) -> Value {
    let active = state
        .db
        .call(|db| faults::list_active_faults(db, None).unwrap_or_default());
    if active.is_empty() {
        return check("health/faults", Result_::Passed, json!({ "count": 0 }));
    }

    let mut kinds: Vec<String> = active.iter().map(|f| f.kind.clone()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    check(
        "health/faults",
        Result_::Failed,
        json!({ "count": active.len(), "kinds": kinds }),
    )
}

// r[impl canopy.report.checks]
// r[impl canopy.report.checks.undeterminable]
/// Whether one of an infrastructure component's container slots is running.
///
/// A slot the runtime could not ask about is distinguished from one it asked
/// about and found absent: reporting the proxy as stopped when the truth is that
/// the container runtime is unreachable would open an incident against a
/// component that may be perfectly healthy.
async fn component_check(state: &Arc<OiState>, name: &str, containers: &[&str]) -> Value {
    let mut asked = false;
    for container in containers {
        match state.container_runtime.inspect(container).await {
            Ok(Some(s)) => {
                asked = true;
                if matches!(s.status, ContainerStatus::Running) {
                    return check(name, Result_::Passed, json!({ "container": container }));
                }
            }
            Ok(None) => asked = true,
            Err(e) => {
                tracing::debug!(container, "cannot inspect for the canopy report: {e}");
            }
        }
    }

    if asked {
        check(name, Result_::Failed, json!({ "running": false }))
    } else {
        check(
            name,
            Result_::Broken,
            json!({ "reason": "the container runtime could not be queried" }),
        )
    }
}

/// How many lifecycle operations are running. The scheduler runs one at a time,
/// so this is zero or one; it is reported as a count because that is the shape
/// the rest of the status surface uses.
fn active_operations(state: &Arc<OiState>) -> usize {
    usize::from(state.scheduler.lock().active().is_some())
}

fn record(state: &Arc<OiState>, ok: bool, error: Option<String>) {
    state.canopy.set_last_report(LastReport {
        at: Timestamp::now(),
        ok,
        error,
    });
}

fn clear_fault(state: &Arc<OiState>) {
    state.db.call(|db| {
        let _ = faults::clear_faults_by_kind(db, &seedling_app(), FAULT_KIND);
    });
}
