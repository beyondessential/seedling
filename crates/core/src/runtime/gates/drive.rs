//! The gate half of each iteration of the loop that keeps the host in line
//! with its apps: bring the gate records, holds, faults and setup record up
//! to date, run the checks that are due, and initiate the runs that are.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use jiff::Timestamp;
use parking_lot::Mutex;
use seedling_protocol::{
    events::OperationTarget,
    names::{AppName, GateName},
};

use super::{
    AppGates, Decision, GateState, Readiness, Remembered, Undetermined, decide, declared_order,
    dependants_down, evaluate, held_resources, read_readiness, retry_delay,
    run::{CheckOutcome, check_retry_is_due, eligible_for_check, run_check},
    store::{self, FailedRuns},
    unready_targets,
};
use crate::{
    defs::{app::AppDef, resource::ResourceId},
    oi::{
        handler::actions::lifecycle::{Admission, admit_and_spawn},
        state::OiState,
    },
    runtime::{
        AppPhase,
        db::Db,
        faults::{self, FaultKey, FaultMeta, FaultScope},
        history::clear_queued_operation,
        secrets::{Cipher, KeyDigest},
        stopped::{self, StoppedSet},
    },
};

/// What the loop reads of an installed app's registry entry.
struct Tracked {
    name: AppName,
    def: Arc<AppDef>,
    /// Why the app's definition fails to evaluate, while it does.
    definition_error: Option<String>,
    generation: u64,
    phase: Arc<Mutex<AppPhase>>,
}

fn tracked(state: &OiState, name: Option<&AppName>) -> Vec<Tracked> {
    let reg = state.registry.read();
    reg.names()
        .into_iter()
        .filter(|n| name.is_none_or(|only| only == n))
        .filter_map(|n| reg.get(n.as_str()))
        .filter(|entry| *entry.phase.lock() == AppPhase::Installed)
        .map(|entry| Tracked {
            name: entry.name.clone(),
            def: entry.app.def.load_full(),
            definition_error: entry.script_error.as_ref().map(|(why, _)| why.clone()),
            generation: entry.current_generation,
            phase: Arc::clone(&entry.phase),
        })
        .collect()
}

/// What one pass over an app's gates found.
struct Synced {
    gates: AppGates,
    /// What the checks and runs decide from, or `None` when what is held or
    /// what is stopped cannot be read, so that no check or run is decided.
    world: Option<World>,
}

/// What an app's resources are, as the checks and runs of its gates read it.
struct World {
    readiness: Readiness,
    held: BTreeSet<ResourceId>,
    stopped: StoppedSet,
}

/// The gate half of one iteration, for every installed app.
// spec: GRUN#initiating-runs
// spec: GRUN#checks
pub async fn drive(state: &Arc<OiState>) {
    for app in tracked(state, None) {
        drive_app(state, &app).await;
    }
}

/// Bring the gate records, holds, faults and setup record of `app` up to date,
/// as they are after a run ends.
pub async fn sync_app(state: &Arc<OiState>, app: &AppName) {
    for app in tracked(state, Some(app)) {
        sync(state, &app).await;
    }
}

/// Clear the hold of `gate` if the gate is satisfied for its current key.
// spec: RCN#gate-holds
pub async fn release_hold_if_satisfied(state: &Arc<OiState>, app: &AppName, gate: &GateName) {
    for app in tracked(state, Some(app)) {
        let (name, gate, def, cipher, error, remembered) = (
            app.name.clone(),
            gate.clone(),
            Arc::clone(&app.def),
            Arc::clone(&state.cipher),
            app.definition_error.clone(),
            state.gate_runtime.remembered(&app.name),
        );
        state.db.call(move |db| {
            let gates = evaluate(db, &name, &def, &cipher, error.as_deref(), &remembered);
            if gates.state(&gate).is_some_and(GateState::is_satisfied)
                && let Err(e) = store::clear_hold(db, &name, &gate)
            {
                tracing::error!(app = %name, %gate, "cannot clear the hold of a satisfied gate: {e}");
            }
        });
    }
}

async fn sync(state: &Arc<OiState>, app: &Tracked) -> Synced {
    let (name, def, cipher, error, remembered) = (
        app.name.clone(),
        Arc::clone(&app.def),
        Arc::clone(&state.cipher),
        app.definition_error.clone(),
        state.gate_runtime.remembered(&app.name),
    );
    state
        .db
        .call(move |db| sync_db(db, &name, &def, &cipher, error.as_deref(), &remembered))
}

/// Bring what is held of an app's gates in the database into line with them.
///
/// Each part is a condition read from the gates as they stand, so that a
/// restart has nothing to reconstruct.
// spec: GRUN#gate-state
// spec: RCN#gate-holds
// spec: GRUN#app-setup
// spec: FLT#filing-and-clearing
fn sync_db(
    db: &Db,
    app: &AppName,
    def: &AppDef,
    cipher: &Cipher,
    definition_error: Option<&str>,
    remembered: &Remembered,
) -> Synced {
    let mut gates = evaluate(db, app, def, cipher, definition_error, remembered);
    if definition_error.is_none() && clear_dropped_checks(db, app, def, &gates) {
        gates = evaluate(db, app, def, cipher, definition_error, remembered);
    }

    if let Ok(holds) = &gates.holds {
        for (gate, covered) in holds {
            match def.gates.get(gate) {
                // spec: RCN#gate-holds
                None if definition_error.is_none() => {
                    if let Err(e) = store::clear_hold(db, app, gate) {
                        tracing::error!(%app, %gate, "cannot clear the hold of a gate the definition no longer declares: {e}");
                    }
                }
                None => {}
                Some(_) if gates.state(gate).is_some_and(GateState::is_satisfied) => {
                    if let Err(e) = store::clear_hold(db, app, gate) {
                        tracing::error!(%app, %gate, "cannot clear the hold of a satisfied gate: {e}");
                    }
                }
                Some(gate_def) if definition_error.is_none() && *covered != gate_def.exclusive => {
                    if let Err(e) = store::set_hold(db, app, gate, &gate_def.exclusive) {
                        tracing::error!(%app, %gate, "cannot bring the hold up to the gate's selection: {e}");
                    }
                }
                Some(_) => {}
            }
        }
    }

    if gates.all_satisfied() {
        match store::is_set_up(db, app) {
            Ok(true) => {}
            Ok(false) => {
                if let Err(e) = store::mark_set_up(db, app) {
                    tracing::error!(%app, "cannot record that the app is set up: {e}");
                }
            }
            Err(e) => tracing::error!(%app, "cannot read whether the app is set up: {e}"),
        }
    }

    sync_gate_faults(db, app, &gates);

    let world = read_world(db, app, def);
    Synced { gates, world }
}

/// What the checks and runs of an app's gates decide from, or `None` when
/// what is held or what is stopped cannot be read: either read as empty would
/// count a held or stopped workload as one that is down.
// spec: GRUN#checks
// spec: GRUN#initiating-runs
fn read_world(db: &Db, app: &AppName, def: &AppDef) -> Option<World> {
    // What is held is read again, as the sync may have released holds.
    let held = match store::load_holds(db, app) {
        Ok(holds) => held_resources(def, &holds).into_keys().collect(),
        Err(e) => {
            tracing::error!(%app, "cannot read what the app's gates hold, so no check or run of them is decided: {e}");
            return None;
        }
    };
    let stopped = match stopped::load_stopped(db, app) {
        Ok(stopped) => stopped,
        Err(e) => {
            tracing::error!(%app, "cannot read the app's stopped resources, so no check or run of its gates is decided: {e}");
            return None;
        }
    };
    let readiness = read_readiness(db, app, def, &held, &stopped);
    Some(World {
        readiness,
        held,
        stopped,
    })
}

/// Clear the check that could not be run from the record of each gate whose
/// definition no longer declares a check, as no check of it will ever run to
/// an outcome. Whether any was cleared.
// spec: GRUN#checks
fn clear_dropped_checks(db: &Db, app: &AppName, def: &AppDef, gates: &AppGates) -> bool {
    let mut cleared = false;
    for (gate, status) in &gates.gates {
        if status.check_unavailable().is_none()
            || def.gates.get(gate).is_none_or(|gate| gate.check.is_some())
        {
            continue;
        }
        match store::clear_check_unavailable(db, app, gate) {
            Ok(()) => cleared = true,
            Err(e) => {
                tracing::error!(%app, %gate, "cannot clear the unavailable check of a gate that no longer declares one: {e}")
            }
        }
    }
    cleared
}

/// File and clear the gate faults of `app` so that they are what its gates
/// are now.
// spec: FLT#app-faults
fn sync_gate_faults(db: &Db, app: &AppName, gates: &AppGates) {
    let mut failed = BTreeMap::new();
    let mut unavailable = BTreeMap::new();
    let mut unreadable = BTreeMap::new();
    for (gate, status) in &gates.gates {
        let key = |kind: &str| FaultKey::new(app, kind, gate.as_str());
        if let Some(how) = status.failed_run() {
            failed.insert(
                key("gate_failed"),
                (
                    FaultMeta::default(),
                    format!(
                        "gate {gate} is not satisfied, and its last run did not succeed: {how}"
                    ),
                ),
            );
        }
        if let Some(why) = status.check_unavailable() {
            unavailable.insert(
                key("gate_check_unavailable"),
                (
                    FaultMeta::default(),
                    format!("the check of gate {gate} could not be run: {why}"),
                ),
            );
        }
        if let Some(why) = status.unreadable() {
            unreadable.insert(
                key("gate_state_unreadable"),
                (
                    FaultMeta::default(),
                    format!("the record of gate {gate} cannot be read: {why}"),
                ),
            );
        }
    }
    for (kind, mut current) in [
        ("gate_failed", failed),
        ("gate_check_unavailable", unavailable),
        ("gate_state_unreadable", unreadable),
    ] {
        let scope = FaultScope::AppKind(app.clone(), kind.to_owned());
        keep_what_cannot_be_told(db, &scope, gates, kind, &mut current);
        if let Err(e) = faults::sync_faults(db, &scope, &current) {
            tracing::error!(%app, kind, "cannot bring the gate faults up to date: {e}");
        }
    }
}

/// Carry over the active faults of gates whose state cannot be told.
///
/// A gate whose definition fails to evaluate, or whose record cannot be read,
/// is neither failed nor not failed: the absence of the fault from what was
/// worked out is not evidence that the fault has gone. The record that says a
/// check could not be run is only unreadable along with the whole record.
// spec: GRUN#gate-state
// spec: FLT#filing-and-clearing
fn keep_what_cannot_be_told(
    db: &Db,
    scope: &FaultScope,
    gates: &AppGates,
    kind: &str,
    current: &mut BTreeMap<FaultKey, (FaultMeta, String)>,
) {
    let unknown: BTreeSet<&GateName> = gates
        .gates
        .iter()
        .filter(|(_, status)| match &status.state {
            GateState::Undetermined(Undetermined::Definition(_)) => kind == "gate_failed",
            GateState::Undetermined(Undetermined::Record(_)) => kind != "gate_state_unreadable",
            _ => false,
        })
        .map(|(name, _)| name)
        .collect();
    if unknown.is_empty() {
        return;
    }
    let active = match faults::list_active_faults_in_scope(db, scope) {
        Ok(active) => active,
        Err(e) => {
            tracing::error!(kind, "cannot read the active gate faults: {e}");
            return;
        }
    };
    for record in active {
        if unknown.iter().any(|gate| gate.as_str() == record.subject) {
            current
                .entry(FaultKey::new(&record.app, record.kind, record.subject))
                .or_insert((FaultMeta::default(), record.description));
        }
    }
}

async fn drive_app(state: &Arc<OiState>, app: &Tracked) {
    let synced = sync(state, app).await;
    let active = state.scheduler.lock().active().cloned();
    let running = |gate: &GateName| {
        active
            .as_ref()
            .is_some_and(|a| a.app == app.name && a.target.gate() == Some(gate))
    };

    // spec: EVT#app-events
    for (name, status) in &synced.gates.gates {
        let reported = status.reported(running(name));
        if let Some(previous) = state.gate_runtime.note_state(&app.name, name, reported) {
            state
                .event_tx
                .gate_state_changed(&app.name, name, reported, previous, None);
        }
    }

    settle_unrecorded(state, app).await;
    withdraw_undeclared(state, app).await;
    if let Some(world) = &synced.world {
        start_checks(state, app, &synced.gates, world, &running).await;
        initiate_runs(state, app, &synced.gates, world).await;
    }
}

/// A failed check whose invalidation could not be written, and a failed run
/// that could not be written, are written as soon as they can be.
// spec: GRUN#checks
// spec: GRUN#retries
async fn settle_unrecorded(state: &Arc<OiState>, app: &Tracked) {
    let remembered = state.gate_runtime.remembered(&app.name);
    for (gate, failed) in &remembered.failed {
        if let Err(e) = write_failed_runs(state, &app.name, gate, failed) {
            tracing::warn!(app = %app.name, %gate, "still cannot record that a run of the gate did not succeed: {e}");
        }
    }
    for gate in remembered.unrecorded.into_keys() {
        let (name, g) = (app.name.clone(), gate.clone());
        match state.db.call(move |db| store::invalidate(db, &name, &g)) {
            Ok(()) => state.gate_runtime.settle_unrecorded(&app.name, &gate),
            Err(e) => {
                tracing::warn!(app = %app.name, %gate, "still cannot record that the check failed: {e}")
            }
        }
    }
}

/// Write the failed runs of `gate` that are held to be written, and stop
/// holding them once they are.
// spec: GRUN#retries
pub(super) fn write_failed_runs(
    state: &OiState,
    app: &AppName,
    gate: &GateName,
    failed: &FailedRuns,
) -> rusqlite::Result<()> {
    let (a, g, f) = (app.clone(), gate.clone(), failed.clone());
    state
        .db
        .call(move |db| store::record_failure(db, &a, &g, &f))?;
    state.gate_runtime.settle_failed_runs(app, gate, failed);
    Ok(())
}

/// Withdraw the queued runs of gates the definition no longer declares.
// spec: GRUN#gates-no-longer-declared
async fn withdraw_undeclared(state: &Arc<OiState>, app: &Tracked) {
    if app.definition_error.is_some() {
        return;
    }
    let withdrawn: Vec<_> = {
        let mut scheduler = state.scheduler.lock();
        let stale: Vec<_> = scheduler
            .queue_iter()
            .filter(|queued| queued.app == app.name)
            .filter(|queued| {
                queued
                    .target
                    .gate()
                    .is_some_and(|gate| !app.def.gates.contains_key(gate))
            })
            .map(|queued| queued.operation_id.clone())
            .collect();
        stale
            .into_iter()
            .filter(|id| scheduler.unqueue(id).is_some())
            .collect()
    };
    for id in withdrawn {
        tracing::info!(app = %app.name, operation_id = %id.0, "withdrawing the queued run of a gate the definition no longer declares");
        state.db.call(move |db| {
            if let Err(e) = clear_queued_operation(db, &id) {
                tracing::error!("cannot clear the withdrawn queued run: {e}");
            }
        });
    }
}

/// Run the checks that are due.
// spec: GRUN#checks
async fn start_checks(
    state: &Arc<OiState>,
    app: &Tracked,
    gates: &AppGates,
    world: &World,
    running: &impl Fn(&GateName) -> bool,
) {
    let remembered = state.gate_runtime.remembered(&app.name);
    let queued: BTreeSet<GateName> = state
        .scheduler
        .lock()
        .queue_iter()
        .filter(|q| q.app == app.name)
        .filter_map(|q| q.target.gate().cloned())
        .collect();
    let now = Timestamp::now();
    for (name, gate_def) in &app.def.gates {
        let Some(check) = &gate_def.check else {
            continue;
        };
        let Some(status) = gates.gates.get(name) else {
            continue;
        };
        let down = dependants_down(
            &app.def,
            name,
            &world.readiness,
            &world.held,
            &world.stopped,
        );
        if !down {
            // The dependants are up, so the next one to start from stopped
            // is a new occasion for a check.
            state.gate_runtime.unclear(&app.name, name);
        }
        if !eligible_for_check(status) || running(name) || queued.contains(name) {
            continue;
        }
        let Some(digest) = status.key_digest else {
            continue;
        };
        if state.gate_runtime.is_checking(&app.name, name)
            || state.gate_runtime.check_deferred(&app.name, name)
        {
            continue;
        }
        let due = match &status.state {
            GateState::Undetermined(Undetermined::Check(_)) => {
                check_retry_is_due(status.record.as_ref(), now)
            }
            _ => {
                let cleared = remembered.cleared.get(name) == Some(&digest);
                let interval_due = check.interval.is_some_and(|interval| {
                    match state.gate_runtime.last_checked(&app.name, name) {
                        Some(at) => at.elapsed() >= interval,
                        None => {
                            state.gate_runtime.note_checked(&app.name, name);
                            false
                        }
                    }
                });
                (down && !cleared) || interval_due
            }
        };
        if !due {
            continue;
        }
        let Some(job) = app.def.resources.get(&check.job).cloned() else {
            continue;
        };
        let Some(claim) = state.gate_runtime.begin_check(&app.name, name) else {
            continue;
        };
        let (state, app_name, gate) = (Arc::clone(state), app.name.clone(), name.clone());
        tokio::spawn(async move {
            let outcome = run_check(&state, &app_name, &gate, job, Some(claim.cancel())).await;
            apply_check_outcome(&state, &app_name, &gate, digest, outcome).await;
            drop(claim);
        });
    }
}

/// Record how a check that no run is waiting on came out.
// spec: GRUN#checks
async fn apply_check_outcome(
    state: &Arc<OiState>,
    app: &AppName,
    gate: &GateName,
    digest: KeyDigest,
    outcome: CheckOutcome,
) {
    let runtime = &state.gate_runtime;
    // spec: GRUN#uninstall
    // A check that ends after the app has left the installed phase records
    // nothing: its gate's state is being deleted, or is gone.
    if tracked(state, Some(app)).is_empty() {
        return;
    }
    match outcome {
        CheckOutcome::Passed => {
            let (a, g) = (app.clone(), gate.clone());
            let cleared = state
                .db
                .call(move |db| store::clear_check_unavailable(db, &a, &g));
            if let Err(e) = cleared {
                tracing::error!(%app, %gate, "cannot record that the check ran to an outcome: {e}");
            }
            runtime.note_checked(app, gate);
            runtime.clear_for_start(app, gate, digest);
        }
        CheckOutcome::Failed(why) => {
            tracing::warn!(%app, %gate, "the check failed, so the gate is no longer satisfied: {why}");
            let (a, g) = (app.clone(), gate.clone());
            let cleared = state.db.call(move |db| {
                store::clear_check_unavailable(db, &a, &g)?;
                store::invalidate(db, &a, &g)
            });
            runtime.note_checked(app, gate);
            runtime.unclear(app, gate);
            if let Err(e) = cleared {
                tracing::error!(%app, %gate, "cannot record that the check failed: {e}");
                runtime.note_unrecorded(app, gate, e.to_string());
            }
        }
        CheckOutcome::Unavailable(why) => {
            tracing::warn!(%app, %gate, "the check could not be run: {why}");
            let (a, g, w) = (app.clone(), gate.clone(), why.clone());
            let recorded = state
                .db
                .call(move |db| store::record_check_unavailable(db, &a, &g, Timestamp::now(), &w));
            // Whether or not it was written, the check is not tried again
            // until a delay that grows with each time it could not be run.
            let failures = match &recorded {
                Ok(()) => state.db.call({
                    let (a, g) = (app.clone(), gate.clone());
                    move |db| {
                        store::load_records(db, &a)
                            .ok()
                            .and_then(|mut records| records.remove(&g))
                            .and_then(Result::ok)
                            .map_or(1, |record| record.check_failures)
                    }
                }),
                Err(e) => {
                    tracing::error!(%app, %gate, "cannot record that the check could not be run: {e}");
                    1
                }
            };
            runtime.defer_check(app, gate, retry_delay(failures));
        }
        CheckOutcome::Cancelled => {}
    }
    sync_app(state, app).await;
    state.tick_notify.notify_one();
}

/// Initiate the run of the first gate that is due, in the order the script
/// declared them.
///
/// A request the concurrency rules do not accept is not a failure of the
/// gate: nothing is recorded, and the next iteration asks again.
// spec: GRUN#initiating-runs
async fn initiate_runs(state: &Arc<OiState>, app: &Tracked, gates: &AppGates, world: &World) {
    if state.scheduler.lock().blocker_for(&app.name).is_some() {
        return;
    }
    let now = Timestamp::now();
    for (name, gate_def) in declared_order(&app.def) {
        let Some(status) = gates.gates.get(name) else {
            continue;
        };
        let unready = unready_targets(&gate_def.after, gates, &world.readiness);
        if decide(status, unready, now) != Decision::Run {
            continue;
        }
        let admission = admit_and_spawn(
            state,
            &app.name,
            OperationTarget::Gate(name.clone()),
            serde_json::Map::new(),
            app.generation,
            "gate",
            &app.phase,
            None,
        );
        match admission {
            Admission::Started(id) | Admission::Queued(id) => {
                tracing::info!(app = %app.name, gate = %name, operation_id = %id.0, "initiated a run of the gate");
                return;
            }
            Admission::Rejected(reason) => {
                tracing::debug!(app = %app.name, gate = %name, ?reason, "a run of the gate is not admitted yet");
                return;
            }
            Admission::CannotPersist(e) => {
                tracing::error!(app = %app.name, gate = %name, "cannot queue a run of the gate: {}", e.message);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
