//! Periodic re-resolution of the tags fetched definitions came from.
//!
//! Back-off source: a [`RetryGates`] keyed per app, capped exponential from
//! [`RETRY_BASE`] to [`RETRY_CAP`]. Classification: every failure is
//! transient; there is no fatal class, since a registry that is down today
//! may be up tomorrow and the allowlist may change. Exit reporting: the task
//! runs for the life of the daemon; each failed re-check is logged, and a
//! failed re-check deliberately changes no fault.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use parking_lot::RwLock;
use seedling_protocol::names::AppName;
use semver::Version;

use super::{
    Source,
    faults::{self, Moved},
    fetch::{self, DefinitionRef, Registry},
};
use crate::runtime::{apps::AppRegistry, db::DbHandle, retry::RetryGates};

/// How long a successful re-check stands before the tag is resolved again.
/// A tag moves rarely, and acting on a move is the operator's decision.
// r[impl definition.recheck.cadence]
pub const RECHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// How often the task looks for apps that are due.
pub const TICK: Duration = Duration::from_secs(5 * 60);
// r[impl definition.recheck.backoff]
pub const RETRY_BASE: Duration = Duration::from_secs(15 * 60);
// r[impl definition.recheck.backoff]
pub const RETRY_CAP: Duration = Duration::from_secs(24 * 60 * 60);
/// How many apps a pass asks about at once. A registry that is down answers
/// only when its connect and read timeouts expire, and a pass that waited
/// for each in turn would run past the tick that starts the next one.
const PASS_CONCURRENCY: usize = 8;

/// A spread of up to a tenth of the interval, so a fleet started together
/// does not re-resolve against a registry at the same moment.
// r[impl definition.recheck.cadence]
fn jitter(interval: Duration) -> Duration {
    use rand::Rng;
    let max = interval.as_secs() / 10;
    Duration::from_secs(rand::rng().next_u64() % (max + 1))
}

/// What a re-check compares against: the reference and the digest the app
/// is running.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    reference: String,
    digest: String,
}

fn target_of(source: &Source) -> Option<Target> {
    let reference = source.recheckable_reference()?;
    let Source::Fetched { digest, .. } = source else {
        return None;
    };
    Some(Target {
        reference: reference.to_owned(),
        digest: digest.clone(),
    })
}

/// When each app is next due, and how its re-checks have been failing.
pub struct Rechecker {
    interval: Duration,
    next_due: HashMap<AppName, Instant>,
    gates: RetryGates<AppName>,
}

impl Default for Rechecker {
    fn default() -> Self {
        Self::new(RECHECK_INTERVAL, RETRY_BASE, RETRY_CAP)
    }
}

impl Rechecker {
    pub fn new(interval: Duration, retry_base: Duration, retry_cap: Duration) -> Self {
        Self {
            interval,
            next_due: HashMap::new(),
            gates: RetryGates::new(retry_base, retry_cap),
        }
    }

    /// Re-check every app that is due at `now`.
    ///
    /// The due apps are asked concurrently and each app's schedule and
    /// back-off are its own: one registry that hangs until its timeouts
    /// expire never pushes back another app's re-check.
    // r[impl definition.recheck]
    // r[impl definition.recheck.backoff]
    // r[impl fault.definition-source-moved]
    pub async fn pass(
        &mut self,
        apps: &RwLock<AppRegistry>,
        db: &DbHandle,
        client: &dyn Registry,
        running: &Version,
        now: Instant,
    ) {
        let targets: Vec<(AppName, Target)> = apps
            .read()
            .iter()
            .filter_map(|e| target_of(&e.source).map(|t| (e.name.clone(), t)))
            .collect();
        self.next_due
            .retain(|app, _| targets.iter().any(|(a, _)| a == app));

        let mut due_now: Vec<(AppName, Target)> = Vec::new();
        for (app, target) in targets {
            let interval = self.interval;
            let due = *self
                .next_due
                .entry(app.clone())
                .or_insert_with(|| now + jitter(interval));
            let failing = self.gates.failures(&app) > 0;
            // A failing app is paced by its back-off rather than the
            // interval; a healthy one by the interval alone.
            let ready = if failing {
                self.gates.should_attempt(&app, now)
            } else {
                now >= due
            };
            if ready {
                due_now.push((app, target));
            }
        }
        if due_now.is_empty() {
            return;
        }

        // The allowlist governs the whole pass, and reading it is the one
        // thing here that blocks on the database.
        let allowed = match db.call(crate::runtime::registries::list_allowed_registries) {
            Ok(a) => a,
            Err(e) => {
                // r[impl fault.definition-source-moved] — with no allowlist
                // there is no re-check, and so nothing to file or clear.
                tracing::warn!(
                    "definition re-check pass skipped: could not read the allowlist: {e}"
                );
                return;
            }
        };

        let results: Vec<(AppName, Target, Result<String, fetch::FetchError>)> =
            futures_util::stream::iter(due_now.into_iter().map(|(app, target)| {
                let allowed = &allowed;
                async move {
                    let outcome = check(client, allowed, &target, running).await;
                    (app, target, outcome)
                }
            }))
            .buffer_unordered(PASS_CONCURRENCY)
            .collect()
            .await;

        for (app, target, outcome) in results {
            let interval = self.interval;
            match outcome {
                Ok(selected) => {
                    self.gates.record_success(&app);
                    self.next_due
                        .insert(app.clone(), now + interval + jitter(interval));
                    // The definition may have been replaced while the
                    // registry was being asked; its replacement already
                    // cleared the fault, and this result says nothing
                    // about the new one.
                    let still_running = apps
                        .read()
                        .get(app.as_str())
                        .and_then(|e| target_of(&e.source))
                        .is_some_and(|t| t == target);
                    if !still_running {
                        continue;
                    }
                    let moved = (selected != target.digest).then(|| Moved {
                        reference: target.reference.clone(),
                        running: target.digest.clone(),
                        selected,
                    });
                    let app_owned = app.clone();
                    db.call(move |db| faults::sync_source_moved(db, &app_owned, moved));
                }
                Err(e) => {
                    // r[impl fault.definition-source-moved] — a failed
                    // re-check neither files nor clears the fault.
                    let failures = self.gates.record_failure(app.clone(), now);
                    tracing::warn!(
                        app = %app,
                        reference = %target.reference,
                        failures,
                        "definition re-check failed: {e}"
                    );
                }
            }
        }
    }
}

/// Resolve the digest a reference selects now, honouring the allowlist.
async fn check(
    client: &dyn Registry,
    allowed: &[String],
    target: &Target,
    running: &Version,
) -> Result<String, fetch::FetchError> {
    let reference = DefinitionRef::parse(&target.reference)?;
    fetch::check_allowed(allowed, &reference)?;
    fetch::resolve_digest(client, &reference, running).await
}

/// Run re-checks for the life of the daemon.
// r[impl definition.recheck]
pub fn spawn(
    apps: Arc<RwLock<AppRegistry>>,
    db: DbHandle,
    client: fetch::SharedRegistry,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let running = super::version::running();
        let mut rechecker = Rechecker::default();
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            rechecker
                .pass(&apps, &db, client.as_ref(), &running, Instant::now())
                .await;
        }
    })
}

#[cfg(test)]
mod tests;
