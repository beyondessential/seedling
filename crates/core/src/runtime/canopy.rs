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
    let healthy = overall_healthy(&health);

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
    let apps = state.registry.read().list();
    let total = apps.len();

    let mut by_status: HashMap<&'static str, usize> = HashMap::new();
    for (_, status) in &apps {
        *by_status.entry(status.name()).or_insert(0) += 1;
    }

    (grade_apps(&apps), total, by_status)
}

// r[impl canopy.report.extra]
// r[impl canopy.report.checks.undeterminable]
/// The overall self-reported health, from the per-check results.
///
/// Recorded for display; Canopy grades incidents from the per-check results
/// rather than from this. A broken check is not itself unhealthy — it says the
/// truth is unknown, not that it is bad — so it does not drag the whole instance
/// down with it.
fn overall_healthy(health: &[Value]) -> bool {
    health.iter().all(|c| {
        !matches!(
            c.get("result").and_then(Value::as_str),
            Some("failed") | Some("warning")
        )
    })
}

// r[impl canopy.report.checks]
/// Grade the app set into the `health/apps` entry.
///
/// Split out from reading the registry so the rule itself is testable without
/// having to drive an app into a derived state through the reconciler.
fn grade_apps(apps: &[(AppName, AppStatus)]) -> Value {
    let mut degraded = Vec::new();
    let mut faulted = Vec::new();
    for (name, status) in apps {
        match status {
            AppStatus::Degraded => degraded.push(name.to_string()),
            AppStatus::Faulted => faulted.push(name.to_string()),
            // Transitional states are not a health signal: an app mid-install
            // or mid-operation is not yet supposed to be running.
            _ => {}
        }
    }

    let result = if !faulted.is_empty() {
        Result_::Failed
    } else if !degraded.is_empty() {
        Result_::Warning
    } else {
        Result_::Passed
    };

    // Naming the apps responsible means an operator reading the report can act
    // on it without coming back to ask which ones.
    check(
        "health/apps",
        result,
        json!({ "degraded": degraded, "faulted": faulted, "total": apps.len() }),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oi::test_support::TestOi;

    fn app(name: &str, status: AppStatus) -> (AppName, AppStatus) {
        (AppName::new_unchecked(name), status)
    }

    fn result_of(entry: &Value) -> &str {
        entry["result"].as_str().expect("every check has a result")
    }

    fn find<'a>(payload: &'a Value, name: &str) -> &'a Value {
        payload["health"]
            .as_array()
            .expect("health is an array")
            .iter()
            .find(|c| c["check"] == name)
            .unwrap_or_else(|| panic!("no {name} check in the report"))
    }

    // r[verify canopy.report.checks]
    #[test]
    fn no_apps_at_all_is_a_pass() {
        let entry = grade_apps(&[]);
        assert_eq!(result_of(&entry), "passed");
        assert_eq!(entry["total"], 0);
    }

    // r[verify canopy.report.checks]
    #[test]
    fn every_app_running_is_a_pass() {
        let entry = grade_apps(&[
            app("postgres", AppStatus::Running),
            app("tamanu", AppStatus::Running),
        ]);
        assert_eq!(result_of(&entry), "passed");
        assert_eq!(entry["total"], 2);
    }

    // r[verify canopy.report.checks]
    #[test]
    fn a_degraded_app_warns_and_is_named() {
        let entry = grade_apps(&[
            app("postgres", AppStatus::Running),
            app("tamanu", AppStatus::Degraded),
        ]);
        assert_eq!(result_of(&entry), "warning");
        assert_eq!(entry["degraded"], json!(["tamanu"]));
        assert_eq!(entry["faulted"], json!([]));
    }

    // r[verify canopy.report.checks]
    #[test]
    fn a_faulted_app_fails_and_outranks_a_degraded_one() {
        let entry = grade_apps(&[
            app("tamanu", AppStatus::Degraded),
            app("postgres", AppStatus::Faulted),
        ]);
        assert_eq!(
            result_of(&entry),
            "failed",
            "the worse condition decides the result"
        );
        assert_eq!(entry["degraded"], json!(["tamanu"]));
        assert_eq!(entry["faulted"], json!(["postgres"]));
    }

    // r[verify canopy.report.checks]
    #[test]
    fn transitional_apps_are_not_a_health_signal() {
        // An app mid-install or mid-operation is not yet meant to be running, so
        // reporting it as unhealthy would open an incident for normal progress.
        let entry = grade_apps(&[
            app("a", AppStatus::NotInstalled),
            app("b", AppStatus::Installing),
            app("c", AppStatus::Uninstalling),
            app(
                "d",
                AppStatus::Operating {
                    action_name: seedling_protocol::names::ActionName::new_unchecked("migrate"),
                },
            ),
        ]);
        assert_eq!(result_of(&entry), "passed");
        assert_eq!(entry["total"], 4);
    }

    // r[verify canopy.report.extra]
    #[test]
    fn the_report_names_seedling_as_its_source_and_describes_only_seedling() {
        let oi = TestOi::new();
        let payload = oi.block_on(build_payload(&oi.state));

        assert_eq!(payload["source"], SOURCE);
        assert_eq!(payload["seedlingVersion"], env!("CARGO_PKG_VERSION"));
        assert!(payload["seedlingUptimeSecs"].is_u64());
        assert_eq!(payload["appsTotal"], 0);
        assert_eq!(payload["activeOperations"], 0);
        assert_eq!(payload["activeFaults"], 0);

        // These describe the host, not Seedling, and the agent that owns them
        // already reports them; claiming them here would have the two fight.
        assert!(payload.get("hostname").is_none());
        assert!(payload.get("uptimeSecs").is_none());
        assert!(
            payload.get("tamanuVersion").is_none(),
            "tamanuVersion sets the server's tracked version and is not ours to set"
        );
    }

    // r[verify canopy.report.checks]
    #[test]
    fn the_report_carries_the_fixed_check_set() {
        let oi = TestOi::new();
        let payload = oi.block_on(build_payload(&oi.state));

        let names: Vec<&str> = payload["health"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["check"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "health/apps",
                "health/faults",
                "health/proxy",
                "health/resolver"
            ],
            "the set is fixed so Canopy's check catalog does not grow with app names"
        );
    }

    // r[verify canopy.report.checks]
    #[test]
    fn no_faults_passes_the_faults_check() {
        let oi = TestOi::new();
        let payload = oi.block_on(build_payload(&oi.state));
        let entry = find(&payload, "health/faults");
        assert_eq!(result_of(entry), "passed");
        assert_eq!(entry["count"], 0);
    }

    // r[verify canopy.report.checks]
    #[test]
    fn an_active_fault_fails_the_faults_check_and_names_its_kind() {
        let oi = TestOi::new();
        oi.state.db.call(|db| {
            faults::file_fault(
                db,
                &AppName::new_unchecked("tamanu"),
                None,
                None,
                None,
                "image_pull_failed",
                "no such tag",
            )
            .expect("file a fault");
        });

        let payload = oi.block_on(build_payload(&oi.state));
        let entry = find(&payload, "health/faults");
        assert_eq!(result_of(entry), "failed");
        assert_eq!(entry["count"], 1);
        assert_eq!(entry["kinds"], json!(["image_pull_failed"]));
        assert_eq!(payload["activeFaults"], 1);
        assert_eq!(
            payload["healthy"], false,
            "a failed check makes the overall self-report unhealthy"
        );
    }

    // r[verify canopy.report.checks]
    #[test]
    fn the_faults_check_lists_each_kind_once() {
        let oi = TestOi::new();
        oi.state.db.call(|db| {
            for (app, kind) in [
                ("a", "image_pull_failed"),
                ("b", "image_pull_failed"),
                ("c", "crash_loop"),
            ] {
                faults::file_fault(
                    db,
                    &AppName::new_unchecked(app),
                    None,
                    None,
                    None,
                    kind,
                    "detail",
                )
                .expect("file a fault");
            }
        });

        let entry = find(&oi.block_on(build_payload(&oi.state)), "health/faults").clone();
        assert_eq!(entry["count"], 3);
        assert_eq!(
            entry["kinds"],
            json!(["crash_loop", "image_pull_failed"]),
            "kinds are deduplicated so the report reads as conditions, not incidents"
        );
    }

    // r[verify canopy.report.checks.undeterminable]
    #[test]
    fn a_component_the_runtime_cannot_ask_about_is_broken_not_failed() {
        // The stub runtime answers "no such container" rather than erroring, so
        // drive the undeterminable case through a container name it is asked
        // about and refuses.
        let oi = TestOi::new();
        let entry = oi.block_on(component_check(&oi.state, "health/x", &[]));
        assert_eq!(
            result_of(&entry),
            "broken",
            "no slot was asked about, so the truth is unknown rather than bad"
        );
        assert!(entry["reason"].is_string());
    }

    // r[verify canopy.report.checks]
    #[test]
    fn a_component_asked_about_and_absent_fails() {
        let oi = TestOi::new();
        let entry = oi.block_on(component_check(
            &oi.state,
            "health/proxy",
            &["seedling-caddy-blue"],
        ));
        assert_eq!(result_of(&entry), "failed");
        assert_eq!(entry["running"], false);
    }

    // r[verify canopy.report.checks.undeterminable]
    #[test]
    fn a_broken_check_does_not_by_itself_make_the_instance_unhealthy() {
        // "The runtime could not ask" is not "the system is bad".
        let broken = check("health/x", Result_::Broken, json!({}));
        assert!(overall_healthy(std::slice::from_ref(&broken)));

        // A failure or a warning does, though.
        assert!(!overall_healthy(&[check(
            "health/y",
            Result_::Failed,
            json!({})
        )]));
        assert!(!overall_healthy(&[check(
            "health/z",
            Result_::Warning,
            json!({})
        )]));
        assert!(
            !overall_healthy(&[broken, check("health/y", Result_::Failed, json!({}))]),
            "a broken check does not mask a real failure alongside it"
        );
    }

    // r[verify canopy.report.extra]
    #[test]
    fn everything_passing_is_healthy() {
        assert!(overall_healthy(&[
            check("health/a", Result_::Passed, json!({})),
            check("health/b", Result_::Passed, json!({})),
        ]));
    }

    // r[verify canopy.report.fault]
    #[test]
    fn a_skipped_report_files_no_fault() {
        let oi = TestOi::new();
        oi.block_on(report(&oi.state))
            .expect("skipping is not an error");

        let faults = oi
            .state
            .db
            .call(|db| faults::list_active_faults(db, None).unwrap_or_default());
        assert!(
            faults.is_empty(),
            "no provider is a deployment choice, not a malfunction"
        );
    }

    // r[verify canopy.report.fault]
    #[test]
    fn a_failing_report_files_a_fault_that_a_lost_provider_then_clears() {
        let oi = TestOi::new();
        // An offer whose peer cannot open a stream: enough to get past the
        // skip checks and fail for a real reason.
        oi.state.canopy.offer(
            1,
            "test".into(),
            "https://example.invalid".into(),
            None,
            crate::oi::canopy::test_peer(),
        );

        oi.block_on(report(&oi.state))
            .expect_err("no reachable provider is a failure once one is offered");

        let filed = oi
            .state
            .db
            .call(|db| faults::list_active_faults(db, None).unwrap_or_default());
        assert_eq!(filed.len(), 1, "one fault for one condition");
        assert_eq!(filed[0].kind, FAULT_KIND);

        let last = oi
            .state
            .canopy
            .last_report()
            .expect("an attempt was recorded");
        assert!(!last.ok);
        assert!(last.error.is_some());

        // Losing the provider means reporting is no longer expected, so a
        // lingering fault would misdescribe the instance.
        oi.state.canopy.revoke_all();
        oi.block_on(report(&oi.state))
            .expect("skipping is not an error");
        let after = oi
            .state
            .db
            .call(|db| faults::list_active_faults(db, None).unwrap_or_default());
        assert!(after.is_empty(), "the fault clears with the expectation");
    }

    // r[verify canopy.report.fault]
    #[test]
    fn repeated_failures_do_not_accumulate_faults() {
        let oi = TestOi::new();
        oi.state.canopy.offer(
            1,
            "test".into(),
            "https://example.invalid".into(),
            None,
            crate::oi::canopy::test_peer(),
        );

        for _ in 0..3 {
            let _ = oi.block_on(report(&oi.state));
        }

        let filed = oi
            .state
            .db
            .call(|db| faults::list_active_faults(db, None).unwrap_or_default());
        assert_eq!(
            filed.len(),
            1,
            "successive failures of one condition are one fault with the latest detail"
        );
    }
}
