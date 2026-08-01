//! Turning the supervisor's restart counter into seedling's restart records,
//! and the records into a crash-loop verdict.
//!
//! The counter is monotonic while a unit lives and zero on a fresh one, so the
//! reconciler keeps a baseline per instance and records the difference. That
//! is what makes recording independent of the observe interval: a container
//! that goes down and comes back between two ticks moves the counter even
//! though it was never seen down.

use jiff::Timestamp;
use seedling_protocol::names::AppName;
use tracing::warn;

use crate::{
    runtime::{
        db::Db,
        generations,
        identity::ResourceInstance,
        restarts::{
            self, ExitKind, ExitStatus, Initiator, RETAIN_PER_INSTANCE, RestartSettings,
            RestartSubject,
        },
    },
    system::types::{UnitExit, UnitExitKind},
};

use super::pods::{self, CrashLoop, CrashLoopCause};

fn subject(instance: &ResourceInstance, generation: Option<i64>) -> RestartSubject {
    RestartSubject {
        app: instance.app.clone(),
        instance_id: instance.id.to_hex(),
        resource_type: Some(format!("{:?}", instance.kind).to_lowercase()),
        resource_name: instance.name.clone(),
        generation,
    }
}

fn exit_status(exit: UnitExit) -> ExitStatus {
    ExitStatus {
        kind: match exit.kind {
            UnitExitKind::Exited => ExitKind::Exited,
            UnitExitKind::Signalled => ExitKind::Signalled,
            UnitExitKind::Dumped => ExitKind::Dumped,
        },
        code: exit.code,
    }
}

/// Reconcile one app's observed restart counters against the stored
/// baselines, record what moved, and return the instances whose rate has
/// reached the threshold.
///
/// Split from the `Reconciler` method so the counter arithmetic — deltas,
/// resets, and the runtime-initiated exclusion — can be exercised directly
/// against a database without standing up a reconciliation tick.
// r[impl autonomous.restart.record]
// r[impl autonomous.restart.rate]
pub(super) fn reconcile_counters(
    db: &Db,
    app: &AppName,
    counters: &[(ResourceInstance, pods::RestartCounter)],
    started: &[ResourceInstance],
) -> Vec<CrashLoop> {
    let settings = restarts::settings(db).unwrap_or(RestartSettings {
        threshold: 5,
        window_secs: 1800,
    });
    let generation = generations::current(db, app)
        .ok()
        .flatten()
        .map(|g| g as i64);

    // r[impl autonomous.restart.record]
    // A start the reconciler issued for an instance it has already run is a
    // restart it initiated. The fresh transient unit's counter begins at zero,
    // so re-baseline here rather than reading the drop as a reset next tick.
    for instance in started {
        let hex = instance.id.to_hex();
        match restarts::baseline(db, &hex) {
            Ok(Some(_)) => {
                if let Err(e) = restarts::record(
                    db,
                    &subject(instance, generation),
                    Initiator::Runtime,
                    None,
                    Timestamp::now().as_millisecond(),
                ) {
                    warn!(app = %app, instance = %hex, "failed to record runtime restart: {e}");
                }
            }
            // No baseline: the reconciler has never seen this instance's unit,
            // so this is a first start, not a restart.
            Ok(None) => continue,
            Err(e) => {
                warn!(app = %app, instance = %hex, "failed to read restart baseline: {e}");
                continue;
            }
        }
        if let Err(e) = restarts::set_baseline(db, &hex, 0) {
            warn!(app = %app, instance = %hex, "failed to re-baseline restart counter: {e}");
        }
    }

    let mut rate_loops = Vec::new();
    for (instance, counter) in counters {
        let hex = instance.id.to_hex();
        if started.iter().any(|s| s.id == instance.id) {
            // The counter was read before this tick's actuation; the start
            // above already re-baselined it.
            continue;
        }
        let observed = i64::from(counter.count);
        let previous = match restarts::baseline(db, &hex) {
            Ok(v) => v,
            Err(e) => {
                warn!(app = %app, instance = %hex, "failed to read restart baseline: {e}");
                continue;
            }
        };

        let new_restarts = match previous {
            // First sighting of this unit. Adopt the counter as the baseline
            // without recording: the restarts it already holds happened at
            // times seedling cannot know, and inventing timestamps for them
            // would corrupt the rate.
            None => 0,
            // The counter went backwards, so it was reset — the unit was
            // recreated or its failed state cleared. Whatever it holds now
            // accrued after the reset.
            Some(prev) if observed < prev => observed,
            Some(prev) => observed - prev,
        };

        if previous != Some(observed)
            && let Err(e) = restarts::set_baseline(db, &hex, observed)
        {
            warn!(app = %app, instance = %hex, "failed to store restart baseline: {e}");
        }

        if new_restarts <= 0 {
            continue;
        }

        // Only the most recent run's exit is known, so it goes on the last of
        // the batch; earlier ones are recorded without one.
        let now = Timestamp::now().as_millisecond();
        let subject = subject(instance, generation);
        for n in 0..new_restarts {
            let last = n == new_restarts - 1;
            let exit = if last {
                counter.exit.map(exit_status)
            } else {
                None
            };
            // Stamp a burst apart so its ordering survives.
            let at = now - (new_restarts - 1 - n);
            if let Err(e) = restarts::record(db, &subject, Initiator::Supervisor, exit, at) {
                warn!(app = %app, instance = %hex, "failed to record restart: {e}");
            }
        }

        // r[impl autonomous.restart.rate]
        match restarts::recent_supervisor_count(db, &hex, settings.window_secs) {
            Ok(count) if count >= settings.threshold => rate_loops.push(CrashLoop {
                instance: instance.clone(),
                cause: CrashLoopCause::RestartRate {
                    count,
                    window_secs: settings.window_secs,
                },
            }),
            Ok(_) => {}
            Err(e) => {
                warn!(app = %app, instance = %hex, "failed to count recent restarts: {e}");
            }
        }
    }
    rate_loops
}

impl super::Reconciler {
    // r[impl autonomous.restart.record]
    // r[impl autonomous.restart.rate]
    /// Run the restart bookkeeping for one app's pod update, appending any
    /// rate-derived crash loops to the update's list.
    ///
    /// Runs before the fault filing step so that a rate-derived crash loop and
    /// a start-limit-hit one are filed through the same path.
    pub(super) fn record_restarts(&self, app: &AppName, update: &mut pods::PodActuationUpdate) {
        let app_owned = app.clone();
        let counters = update.restart_counters.clone();
        let started: Vec<ResourceInstance> = update.started_instances.to_vec();
        let rate_loops = self
            .db
            .call(move |db| reconcile_counters(db, &app_owned, &counters, &started));

        for loop_ in rate_loops {
            // A unit that also hit the start limit is already listed; one
            // crash_loop fault per instance is what the operator needs.
            if update
                .crash_loops
                .iter()
                .any(|c| c.instance.id == loop_.instance.id)
            {
                continue;
            }
            update.crash_loops.push(loop_);
        }
    }
}

// r[impl gc.restarts]
/// Drop restart bookkeeping for instances that no longer exist, and re-apply
/// the per-instance cap. Recording already enforces the cap on write; this
/// catches rows left by an older build or a partial write.
pub fn gc(db: &Db) -> rusqlite::Result<usize> {
    let orphaned_records = db.conn.execute(
        "DELETE FROM instance_restarts
         WHERE instance_id NOT IN (SELECT id FROM resource_instances)",
        [],
    )?;
    let orphaned_counters = db.conn.execute(
        "DELETE FROM instance_restart_counters
         WHERE instance_id NOT IN (SELECT id FROM resource_instances)",
        [],
    )?;

    let over_cap: Vec<String> = {
        let mut stmt = db.conn.prepare(
            "SELECT instance_id FROM instance_restarts
             GROUP BY instance_id HAVING COUNT(*) > ?1",
        )?;
        let rows = stmt.query_map([RETAIN_PER_INSTANCE as i64], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut pruned = 0;
    for instance_id in over_cap {
        pruned += restarts::prune_instance(db, &instance_id, RETAIN_PER_INSTANCE)?;
    }

    Ok(orphaned_records + orphaned_counters + pruned)
}

#[cfg(test)]
mod tests;
