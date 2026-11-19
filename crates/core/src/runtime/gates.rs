//! How the runtime keeps an installed app's gates satisfied: what each gate's
//! state is, whether a run of it is due, what its holds and its dependants'
//! waits come to, and the in-memory state its checks keep between iterations.
//!
//! This is the one place that decides whether a gate runs. The reconciler and
//! the operator interface ask it rather than read gate records themselves.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};

use jiff::Timestamp;
use parking_lot::Mutex;
use seedling_protocol::names::{AppName, GateName};

use crate::{
    defs::{
        app::AppDef,
        gate::{AfterTarget, GateDef, workload_after},
        resource::{Resource, ResourceId, ResourceKind},
    },
    runtime::{
        barrier::{
            CancelToken,
            oracle::{container_run_succeeded, derive_lifecycle_state},
        },
        db::Db,
        history::{instances_for_app, query_observations_for},
        identity::ResourceInstance,
        lifecycle::LifecycleState,
        scaling,
        secrets::{Cipher, KeyDigest},
        stopped::StoppedSet,
    },
};

pub mod drive;
pub mod run;
pub mod store;

use store::{FailedRuns, GateRecord, Holds, Records};

/// How long after its first failed run a gate waits before it runs again.
// spec: GRUN#retries
pub const RETRY_BASE: Duration = Duration::from_secs(30);

/// The longest a gate waits between runs, however long they have been failing.
// spec: GRUN#retries
pub const RETRY_CAP: Duration = Duration::from_secs(3600);

/// The delay before the run that follows `failures` consecutive failed ones.
// spec: GRUN#retries
pub fn retry_delay(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    RETRY_BASE
        .saturating_mul(2u32.saturating_pow(failures.saturating_sub(1)))
        .min(RETRY_CAP)
}

/// Why a gate is neither satisfied nor unsatisfied.
// spec: GRUN#gate-state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Undetermined {
    /// The definition fails to evaluate, or the gate's key cannot be told
    /// from another.
    Definition(String),
    /// The gate's record cannot be read.
    Record(String),
    /// The gate holds a valid success, and its most recent check could not
    /// be run.
    Check(String),
    /// A failed check could not be recorded, so what the record says no
    /// longer stands.
    Unrecorded(String),
}

/// Whether a gate is satisfied, unsatisfied, or neither because the runtime
/// cannot tell. Undetermined is never folded into either of the others.
// spec: GRUN#gate-state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateState {
    Satisfied,
    Unsatisfied,
    Undetermined(Undetermined),
}

impl GateState {
    pub fn is_satisfied(&self) -> bool {
        matches!(self, Self::Satisfied)
    }
}

/// What the runtime knows of one gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateStatus {
    pub state: GateState,
    /// The digest of the gate's key under the definition, or `None` where it
    /// could not be worked out.
    pub key_digest: Option<KeyDigest>,
    /// The gate's record, or `None` where it has none or cannot be read.
    pub record: Option<GateRecord>,
    /// A cancelled run paused the gate for its current key.
    // spec: GRUN#pauses
    pub paused: bool,
    /// Why the gate's record cannot be read, where it cannot, whatever else
    /// keeps the gate's state from being told.
    pub record_error: Option<String>,
}

impl GateStatus {
    /// How the gate's most recent run ended, while it is unsatisfied and that
    /// run did not succeed.
    // spec: GRUN#retries
    pub fn failed_run(&self) -> Option<&str> {
        if self.state != GateState::Unsatisfied {
            return None;
        }
        self.record
            .as_ref()
            .and_then(|record| record.last_failure.as_deref())
    }

    /// Why the gate's most recent check could not be run, if it could not.
    // spec: GRUN#checks
    pub fn check_unavailable(&self) -> Option<&str> {
        self.record
            .as_ref()
            .and_then(|record| record.check_unavailable.as_deref())
    }

    /// Why the gate's record cannot be read, if it cannot.
    // spec: GRUN#gate-state
    pub fn unreadable(&self) -> Option<&str> {
        self.record_error.as_deref()
    }

    /// When the runtime next runs the gate, for an unsatisfied gate whose last
    /// run failed and that is not paused: `None` where there is no such run
    /// to wait out, or where the gate's key has moved since and its run is due
    /// at once.
    // spec: APPS#gates
    pub fn next_run_at(&self) -> Option<Timestamp> {
        if self.paused || self.failed_run().is_none() {
            return None;
        }
        retry_due(self.record.as_ref(), self.key_digest.as_ref())
    }
}

impl GateStatus {
    /// The state reported for the gate: `running` while a run of it is in
    /// progress, `paused` while it is paused, and otherwise the state the
    /// runtime determined.
    // spec: APPS#gates
    pub fn reported(&self, running: bool) -> &'static str {
        if running {
            "running"
        } else if self.paused && self.state == GateState::Unsatisfied {
            "paused"
        } else {
            match self.state {
                GateState::Satisfied => "satisfied",
                GateState::Unsatisfied => "unsatisfied",
                GateState::Undetermined(_) => "undetermined",
            }
        }
    }
}

/// The state of every gate of an app, and what its gates hold.
#[derive(Debug, Clone)]
pub struct AppGates {
    pub gates: BTreeMap<GateName, GateStatus>,
    /// What each gate holds stopped, or why that cannot be read.
    pub holds: Result<Holds, String>,
}

impl AppGates {
    pub fn state(&self, gate: &GateName) -> Option<&GateState> {
        self.gates.get(gate).map(|status| &status.state)
    }

    /// Whether every gate the app declares is satisfied.
    // spec: GRUN#app-setup
    pub fn all_satisfied(&self) -> bool {
        self.gates.values().all(|g| g.state.is_satisfied())
    }
}

/// What the runtime holds in memory of an app's gates between iterations.
///
/// It is read with the app's definition and records to work out its gates'
/// state. None of it outlives the runtime, because each part is something a
/// restart is right to look at again.
#[derive(Debug, Default, Clone)]
pub struct Remembered {
    /// Gates whose last check failed and whose invalidation could not be
    /// written, with why.
    pub unrecorded: BTreeMap<GateName, String>,
    /// Gates a check has cleared for the dependants that start from stopped,
    /// for the key given.
    pub cleared: BTreeMap<GateName, KeyDigest>,
    /// Runs of each gate that did not succeed and are not yet written to its
    /// record.
    pub failed: BTreeMap<GateName, FailedRuns>,
}

/// Work out the state of every gate of `app`, as its definition and records
/// give it.
///
/// `definition_error` is set while the app's definition fails to evaluate, as
/// the registry keeps the last good definition beside the failure.
// spec: GRUN#gate-state
pub fn evaluate(
    db: &Db,
    app: &AppName,
    def: &AppDef,
    cipher: &Cipher,
    definition_error: Option<&str>,
    remembered: &Remembered,
) -> AppGates {
    #[cfg(test)]
    READS.with(|reads| reads.set((reads.get().0 + 1, reads.get().1)));
    let holds = store::load_holds(db, app).map_err(|e| e.to_string());
    if def.gates.is_empty() {
        return AppGates {
            gates: BTreeMap::new(),
            holds,
        };
    }
    let records: Result<Records, String> = store::load_records(db, app).map_err(|e| e.to_string());
    let gates = def
        .gates
        .iter()
        .map(|(name, gate)| {
            let digest = gate.key.digest(cipher);
            let record = match &records {
                Ok(records) => records.get(name).cloned(),
                Err(why) => Some(Err(why.clone())),
            };
            (
                name.clone(),
                status_of(
                    name,
                    gate.check.is_some(),
                    digest.map_err(|e| e.to_string()),
                    record,
                    definition_error,
                    remembered,
                ),
            )
        })
        .collect();
    AppGates { gates, holds }
}

/// The status of one gate, from what could be worked out of its key and what
/// was read of its record, where `None` is no record and `Some(Err)` one that
/// cannot be read.
fn status_of(
    name: &GateName,
    declares_check: bool,
    digest: Result<KeyDigest, String>,
    record: Option<Result<GateRecord, String>>,
    definition_error: Option<&str>,
    remembered: &Remembered,
) -> GateStatus {
    let (key_digest, key_error) = match digest {
        Ok(digest) => (Some(digest), None),
        Err(why) => (
            None,
            Some(format!("the gate's key cannot be digested: {why}")),
        ),
    };
    let (record, unreadable) = match record {
        None => (None, None),
        Some(Ok(record)) => (Some(record), None),
        Some(Err(why)) => (None, Some(why)),
    };
    // spec: GRUN#retries
    // Runs whose failure could not be written count as they will once it is,
    // so that the next run waits them out and the gate is reported failing.
    let record = match remembered.failed.get(name) {
        Some(failed) if unreadable.is_none() => Some(failed.applied_to(record.unwrap_or_default())),
        _ => record,
    };
    // Each reason that the state cannot be told outranks the next, in the
    // order the specification lists them.
    let undetermined = if let Some(why) = definition_error {
        Some(Undetermined::Definition(why.to_owned()))
    } else if let Some(why) = key_error {
        Some(Undetermined::Definition(why))
    } else if let Some(why) = &unreadable {
        Some(Undetermined::Record(why.clone()))
    } else {
        remembered
            .unrecorded
            .get(name)
            .map(|why| Undetermined::Unrecorded(why.clone()))
    };
    let paused = match (&record, &key_digest) {
        (Some(record), Some(digest)) => record.paused_key_digest.as_ref() == Some(digest),
        _ => false,
    };
    let state = match undetermined {
        Some(why) => GateState::Undetermined(why),
        None => match (&record, &key_digest) {
            (Some(record), Some(digest))
                if record.valid && record.key_digest.as_ref() == Some(digest) =>
            {
                match &record.check_unavailable {
                    // spec: GRUN#gate-state
                    Some(why) if declares_check => {
                        GateState::Undetermined(Undetermined::Check(why.clone()))
                    }
                    _ => GateState::Satisfied,
                }
            }
            _ => GateState::Unsatisfied,
        },
    };
    GateStatus {
        state,
        key_digest,
        record,
        paused,
        record_error: unreadable,
    }
}

/// Why a gate is not run now.
// spec: GRUN#initiating-runs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotDue {
    Satisfied,
    Undetermined,
    Paused,
    /// A target of the gate is not ready.
    WaitingOn(Vec<AfterTarget>),
    /// A failed run is being waited out, until the time given.
    BackOff(Timestamp),
}

/// The decision whether to run a gate.
// spec: GRUN#initiating-runs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Run,
    Hold(NotDue),
}

/// Decide whether a run of a gate is due: it is unsatisfied, not paused,
/// every target is ready and its retry is due.
///
/// Nothing else decides this. An undetermined gate is never run, and a gate
/// whose targets are not all ready is waited on rather than failed.
// spec: GRUN#initiating-runs
pub fn decide(status: &GateStatus, unready: Vec<AfterTarget>, now: Timestamp) -> Decision {
    match &status.state {
        GateState::Satisfied => return Decision::Hold(NotDue::Satisfied),
        GateState::Undetermined(_) => return Decision::Hold(NotDue::Undetermined),
        GateState::Unsatisfied => {}
    }
    if status.paused {
        return Decision::Hold(NotDue::Paused);
    }
    if !unready.is_empty() {
        return Decision::Hold(NotDue::WaitingOn(unready));
    }
    if let Some(due) = retry_due(status.record.as_ref(), status.key_digest.as_ref())
        && now < due
    {
        return Decision::Hold(NotDue::BackOff(due));
    }
    Decision::Run
}

/// When the next run of a gate is due after a failed one, or `None` where it
/// is due at once: no run has failed, or the key moved since.
// spec: GRUN#retries
fn retry_due(record: Option<&GateRecord>, key: Option<&KeyDigest>) -> Option<Timestamp> {
    let record = record?;
    let failed_at = record.last_failure_at?;
    if record.failed_key_digest.as_ref() != key {
        return None;
    }
    let delay = retry_delay(record.consecutive_failures);
    Timestamp::from_second(failed_at.as_second().saturating_add(delay.as_secs() as i64)).ok()
}

/// When the next check of a gate is due after one that could not be run, or
/// `None` where it is due at once.
// spec: GRUN#checks
pub fn check_retry_due(record: &GateRecord) -> Option<Timestamp> {
    let tried_at = record.check_unavailable_at?;
    let delay = retry_delay(record.check_failures);
    Timestamp::from_second(tried_at.as_second().saturating_add(delay.as_secs() as i64)).ok()
}

// ---------------------------------------------------------------------------
// Start order
// ---------------------------------------------------------------------------

/// What a workload is ordered directly after, or nothing for a resource that
/// is not a Deployment or a Job.
fn workload_targets(def: &AppDef, id: &ResourceId) -> BTreeSet<AfterTarget> {
    def.resources
        .get(id)
        .and_then(workload_after)
        .unwrap_or_default()
}

/// The workloads of `def`: its named Deployments and Jobs.
fn workloads(def: &AppDef) -> impl Iterator<Item = &ResourceId> {
    def.resources
        .keys()
        .filter(|id| matches!(id.kind, ResourceKind::Deployment | ResourceKind::Job))
}

/// Every gate `workload` is ordered after, directly or through other targets.
// spec: GATE#start-order
pub fn gates_behind(def: &AppDef, workload: &ResourceId) -> BTreeSet<GateName> {
    fn walk(
        def: &AppDef,
        targets: &BTreeSet<AfterTarget>,
        seen_workloads: &mut BTreeSet<ResourceId>,
        gates: &mut BTreeSet<GateName>,
    ) {
        for target in targets {
            match target {
                AfterTarget::Gate(name) => {
                    if gates.insert(name.clone())
                        && let Some(gate) = def.gates.get(name)
                    {
                        walk(def, &gate.after, seen_workloads, gates);
                    }
                }
                AfterTarget::Workload(id) => {
                    if seen_workloads.insert(id.clone()) {
                        walk(def, &workload_targets(def, id), seen_workloads, gates);
                    }
                }
            }
        }
    }
    let mut gates = BTreeSet::new();
    walk(
        def,
        &workload_targets(def, workload),
        &mut BTreeSet::from([workload.clone()]),
        &mut gates,
    );
    gates
}

/// What a gate holds stopped, as its record and the definition's current
/// selection together say.
///
/// A resource a change removed from the selection stays held until the
/// record drops it, and one a change added is held at once.
// spec: RCN#gate-holds
pub fn held_resources(def: &AppDef, holds: &Holds) -> BTreeMap<ResourceId, BTreeSet<GateName>> {
    let mut held: BTreeMap<ResourceId, BTreeSet<GateName>> = BTreeMap::new();
    for (gate, covered) in holds {
        let current = def
            .gates
            .get(gate)
            .map(|g| g.exclusive.clone())
            .unwrap_or_default();
        for id in covered.iter().chain(&current) {
            held.entry(id.clone()).or_default().insert(gate.clone());
        }
    }
    held
}

/// The resources a runtime handle method leaves out on account of gates other
/// than `running`: what they hold, and the workloads ordered after them while
/// they are not satisfied.
///
/// `Err` where what a gate holds cannot be read.
// spec: RT#resources-a-gate-holds
pub fn rt_exclusions(
    def: &AppDef,
    gates: &AppGates,
    running: Option<&GateName>,
) -> Result<BTreeSet<ResourceId>, String> {
    let holds = gates
        .holds
        .as_ref()
        .map_err(|why| format!("what the app's gates hold cannot be read: {why}"))?;
    let mut excluded: BTreeSet<ResourceId> = held_resources(def, holds)
        .into_iter()
        .filter(|(_, by)| by.iter().any(|gate| Some(gate) != running))
        .map(|(id, _)| id)
        .collect();
    for id in workloads(def) {
        let waits = gates_behind(def, id)
            .into_iter()
            .filter(|gate| Some(gate) != running)
            .any(|gate| gates.state(&gate).is_none_or(|state| !state.is_satisfied()));
        if waits {
            excluded.insert(id.clone());
        }
    }
    Ok(excluded)
}

/// What a runtime handle leaves out of the resources it acts on, as a
/// closure's pass was given it.
// spec: RT#resources-a-gate-holds
#[derive(Debug, Clone, Default)]
pub struct Exclusions {
    /// The Jobs of the gates' checks.
    checks: BTreeSet<ResourceId>,
    gated: Gated,
}

/// What the gates keep from a closure.
#[derive(Debug, Clone, Default)]
enum Gated {
    /// Nothing: the closure runs outside an installed app, as in a
    /// language-only test.
    #[default]
    None,
    Known(BTreeSet<ResourceId>),
    /// What gates hold cannot be read, so a call that would act on a workload
    /// fails rather than act on one that may be held.
    Unreadable(String),
}

impl Exclusions {
    /// What is left out for a closure of `def` running for gate `running`, or
    /// for no gate.
    pub fn of(def: &AppDef, gates: &AppGates, running: Option<&GateName>) -> Self {
        Self {
            // spec: GATE#check
            checks: check_jobs(def),
            gated: match rt_exclusions(def, gates, running) {
                Ok(ids) => Gated::Known(ids),
                Err(why) => Gated::Unreadable(why),
            },
        }
    }

    /// What is left out for a closure of `app`, as its gates stand now.
    ///
    /// The gates are evaluated here, as the reconciler evaluates them, so a
    /// closure is kept from what the reconciler keeps from the app.
    // spec: RT#resources-a-gate-holds
    pub fn for_closure(
        db: &Db,
        app: &AppName,
        def: &AppDef,
        cipher: &Cipher,
        remembered: &Remembered,
        running: Option<&GateName>,
    ) -> Self {
        let gates = evaluate(db, app, def, cipher, None, remembered);
        Self::of(def, &gates, running)
    }

    /// Whether the named workload is left out.
    pub fn leaves_out(&self, kind: ResourceKind, name: &str) -> Result<bool, String> {
        let named = |ids: &BTreeSet<ResourceId>| {
            ids.iter()
                .any(|id| id.kind == kind && id.name.as_str() == name)
        };
        // spec: GATE#check
        if named(&self.checks) {
            return Ok(true);
        }
        match &self.gated {
            Gated::None => Ok(false),
            Gated::Known(ids) => Ok(named(ids)),
            Gated::Unreadable(why) => Err(why.clone()),
        }
    }
}

/// Which of a gate's or workload's targets are ready, as readiness reads them.
#[derive(Debug, Default, Clone)]
pub struct Readiness {
    /// The workloads of the app that are ready.
    pub ready: BTreeSet<ResourceId>,
    /// The workloads with an instance that is started and not yet ended, for
    /// the check that stands before a workload starts from stopped.
    pub live: BTreeSet<ResourceId>,
    /// Why readiness could not be read, where it could not: every workload
    /// then reads as not ready.
    pub unreadable: Option<String>,
}

/// Which workloads are ready, for every one that anything is ordered after.
///
/// A Deployment is ready when at least one of its instances is `Ready` and it
/// is neither stopped, nor held, nor at an effective scale of zero. A Job is
/// ready when its most recent instance ran to completion and exited with
/// status 0. Both are read from the instances' history rather than from what
/// the runtime has remembered since it started.
// spec: GATE#start-order
pub fn read_readiness(
    db: &Db,
    app: &AppName,
    def: &AppDef,
    held: &BTreeSet<ResourceId>,
    stopped: &StoppedSet,
) -> Readiness {
    #[cfg(test)]
    READS.with(|reads| reads.set((reads.get().0, reads.get().1 + 1)));
    let mut wanted: BTreeSet<&ResourceId> = BTreeSet::new();
    for gate in def.gates.values() {
        for target in &gate.after {
            if let AfterTarget::Workload(id) = target {
                wanted.insert(id);
            }
        }
        if gate.check.is_some() {
            wanted.extend(workloads(def).filter(|id| {
                workload_targets(def, id)
                    .iter()
                    .any(|t| matches!(t, AfterTarget::Gate(_)))
            }));
        }
    }
    for id in workloads(def) {
        for target in workload_targets(def, id) {
            if let AfterTarget::Workload(target) = target
                && let Some((key, _)) = def.resources.get_key_value(&target)
            {
                wanted.insert(key);
            }
        }
    }
    match read_wanted(db, app, def, &wanted, held, stopped) {
        Ok(readiness) => readiness,
        Err(e) => Readiness {
            unreadable: Some(e),
            ..Readiness::default()
        },
    }
}

/// Readiness of the `wanted` workloads, from one read of the app's instances,
/// one of their observations and one of its scaling decisions.
fn read_wanted(
    db: &Db,
    app: &AppName,
    def: &AppDef,
    wanted: &BTreeSet<&ResourceId>,
    held: &BTreeSet<ResourceId>,
    stopped: &StoppedSet,
) -> Result<Readiness, String> {
    let mut readiness = Readiness::default();
    if wanted.is_empty() {
        return Ok(readiness);
    }
    let mut instances: BTreeMap<&ResourceId, Vec<ResourceInstance>> = BTreeMap::new();
    for instance in instances_for_app(db, app).map_err(|e| e.to_string())? {
        let id = wanted.iter().copied().find(|id| {
            id.kind == instance.kind && instance.name.as_deref() == Some(id.name.as_str())
        });
        if let Some(id) = id {
            instances.entry(id).or_default().push(instance);
        }
    }
    let all: Vec<ResourceInstance> = instances.values().flatten().cloned().collect();
    let histories = query_observations_for(db, &all).map_err(|e| e.to_string())?;
    let history = |instance: &ResourceInstance| {
        histories
            .get(&instance.id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    let decisions = if wanted.iter().any(|id| id.kind == ResourceKind::Deployment) {
        scaling::load_scaling_decisions_for_app(db, app).map_err(|e| e.to_string())?
    } else {
        Default::default()
    };
    for id in wanted.iter().copied() {
        let instances = instances.get(id).map(Vec::as_slice).unwrap_or_default();
        let live = instances.iter().any(|instance| {
            matches!(
                derive_lifecycle_state(instance, history(instance)),
                LifecycleState::Scheduled | LifecycleState::Running | LifecycleState::Ready
            )
        });
        if live {
            readiness.live.insert(id.clone());
        }
        if held.contains(id) || stopped.contains(&(id.kind, id.name.as_str().to_owned())) {
            continue;
        }
        let ready = match id.kind {
            ResourceKind::Deployment => {
                let at_zero = match def.resources.get(id) {
                    Some(Resource::Deployment(d)) => {
                        let scale = &d.def.lock().scale;
                        scaling::effective_scale_from(
                            decisions.get(id.name.as_str()).copied(),
                            *scale.start(),
                            *scale.end(),
                        ) == 0
                    }
                    _ => false,
                };
                !at_zero
                    && instances.iter().any(|instance| {
                        derive_lifecycle_state(instance, history(instance)) == LifecycleState::Ready
                    })
            }
            ResourceKind::Job => instances
                .last()
                .is_some_and(|instance| container_run_succeeded(history(instance)) == Some(true)),
            _ => false,
        };
        if ready {
            readiness.ready.insert(id.clone());
        }
    }
    Ok(readiness)
}

/// The targets in `targets` that are not ready.
///
/// A gate is ready when it is satisfied. A workload is ready as `readiness`
/// says. A target the definition does not declare is not ready.
// spec: GATE#start-order
pub fn unready_targets(
    targets: &BTreeSet<AfterTarget>,
    gates: &AppGates,
    readiness: &Readiness,
) -> Vec<AfterTarget> {
    targets
        .iter()
        .filter(|target| match target {
            AfterTarget::Gate(name) => !gates.state(name).is_some_and(GateState::is_satisfied),
            AfterTarget::Workload(id) => !readiness.ready.contains(id),
        })
        .cloned()
        .collect()
}

/// The gates of `def` in the order its script declared them.
pub fn declared_order(def: &AppDef) -> Vec<(&GateName, &GateDef)> {
    let mut gates: Vec<_> = def.gates.iter().collect();
    gates.sort_by_key(|(_, gate)| gate.declaration);
    gates
}

/// What the reconciler needs of an app's gates in an iteration.
#[derive(Debug, Default, Clone)]
pub struct GateView {
    /// Resources held `Unscheduled` whatever their effective scale, with the
    /// gates holding them.
    // spec: RCN#gate-holds
    pub held: BTreeMap<ResourceId, BTreeSet<GateName>>,
    /// Workloads that start no new instance, and whose stale instances are not
    /// stopped, with the targets each waits on.
    // spec: GATE#start-order
    pub waiting: BTreeMap<ResourceId, Vec<AfterTarget>>,
    /// Why readiness could not be read, where it could not: every workload
    /// then waits on whatever it is ordered after.
    pub readiness_unreadable: Option<String>,
}

impl GateView {
    pub fn held_ids(&self) -> BTreeSet<ResourceId> {
        self.held.keys().cloned().collect()
    }

    /// `stopped`, the resources an operator stopped, with every resource a gate
    /// holds added: each is kept at `Unscheduled` whatever its effective scale.
    // spec: RCN#gate-holds
    pub fn keep_down(&self, stopped: &StoppedSet) -> StoppedSet {
        stopped
            .iter()
            .cloned()
            .chain(
                self.held
                    .keys()
                    .map(|id| (id.kind, id.name.as_str().to_owned())),
            )
            .collect()
    }

    /// Whether the named workload of the given kind waits on a target.
    pub fn waits_for(&self, kind: &ResourceKind, name: Option<&str>) -> bool {
        name.is_some_and(|name| {
            self.waiting
                .keys()
                .any(|id| id.kind == *kind && id.name.as_str() == name)
        })
    }
}

/// A gate whose run is in progress, and the declared resources its body has
/// started.
#[derive(Debug, Clone)]
pub struct BodyStarts {
    pub gate: GateName,
    pub started: BTreeSet<ResourceId>,
}

/// The view of an app's gates for an iteration of the reconciliation loop.
///
/// Where what a gate holds cannot be read, the resources of every gate's
/// exclusive selection wait instead: none is started, and none is stopped
/// because of what could not be read.
///
/// `body` is the gate whose run is in progress with what its body has started:
/// none of those waits on that gate, though each still waits on its other
/// targets.
// spec: RCN#gate-holds
// spec: GRUN#gate-state
pub fn view(
    db: &Db,
    app: &AppName,
    def: &AppDef,
    gates: &AppGates,
    stopped: &StoppedSet,
    remembered: &Remembered,
    body: Option<&BodyStarts>,
) -> GateView {
    let held = match &gates.holds {
        Ok(holds) => held_resources(def, holds).into_keys().collect(),
        Err(_) => BTreeSet::new(),
    };
    let readiness = read_readiness(db, app, def, &held, stopped);
    view_from(app, def, gates, &readiness, remembered, body)
}

/// The view of an app's gates, as [`view`] gives it, from readiness already
/// read against what the gates hold and what is stopped.
// spec: RCN#gate-holds
// spec: GRUN#gate-state
pub fn view_from(
    app: &AppName,
    def: &AppDef,
    gates: &AppGates,
    readiness: &Readiness,
    remembered: &Remembered,
    body: Option<&BodyStarts>,
) -> GateView {
    let mut view = GateView::default();
    let mut unsure: BTreeSet<ResourceId> = BTreeSet::new();
    match &gates.holds {
        Ok(holds) => view.held = held_resources(def, holds),
        Err(why) => {
            tracing::error!(%app, "cannot read what the app's gates hold; starting none of the selections: {why}");
            unsure.extend(def.gates.values().flat_map(|g| g.exclusive.iter().cloned()));
        }
    }
    if let Some(why) = &readiness.unreadable {
        tracing::error!(%app, "cannot read whether the app's workloads are ready; starting none that wait on one: {why}");
    }
    view.readiness_unreadable = readiness.unreadable.clone();
    for id in workloads(def) {
        let mut waiting_on = unready_targets(&workload_targets(def, id), gates, readiness);
        // A gate that declares a check clears its dependants to start from
        // stopped only once a check has passed.
        for target in workload_targets(def, id) {
            let AfterTarget::Gate(name) = &target else {
                continue;
            };
            if waiting_on.contains(&target) {
                continue;
            }
            let Some(gate) = def.gates.get(name) else {
                continue;
            };
            if gate.check.is_none() || readiness.live.contains(id) {
                continue;
            }
            let cleared = gates
                .gates
                .get(name)
                .and_then(|g| g.key_digest.as_ref())
                .is_some_and(|digest| remembered.cleared.get(name) == Some(digest));
            if !cleared {
                waiting_on.push(target);
            }
        }
        // spec: GATE#body
        if let Some(body) = body
            && body.started.contains(id)
        {
            waiting_on
                .retain(|target| !matches!(target, AfterTarget::Gate(name) if *name == body.gate));
        }
        if !waiting_on.is_empty() || unsure.contains(id) {
            view.waiting.insert(id.clone(), waiting_on);
        }
    }
    for id in unsure {
        view.waiting.entry(id).or_default();
    }
    view
}

/// The gates of `def` that have a check, and the Job each runs.
pub fn check_jobs(def: &AppDef) -> BTreeSet<ResourceId> {
    def.gates
        .values()
        .filter_map(|gate| gate.check.as_ref().map(|check| check.job.clone()))
        .collect()
}

/// Whether the dependants of `gate` that want to start have a workload down,
/// so that a check must clear them before they start.
// spec: GRUN#checks
pub fn dependants_down(
    def: &AppDef,
    gate: &GateName,
    readiness: &Readiness,
    held: &BTreeSet<ResourceId>,
    stopped: &StoppedSet,
) -> bool {
    workloads(def)
        .filter(|id| {
            workload_targets(def, id)
                .iter()
                .any(|t| matches!(t, AfterTarget::Gate(g) if g == gate))
        })
        .filter(|id| {
            !held.contains(*id) && !stopped.contains(&(id.kind, id.name.as_str().to_owned()))
        })
        .any(|id| !readiness.live.contains(id))
}

/// What an iteration worked out of an app's gates, and what it worked it out
/// from, so that the reconciler acts in that iteration on the gate state the
/// gates were driven on rather than reading it a second time.
#[derive(Debug, Clone)]
pub struct GateSnapshot {
    pub def: Arc<AppDef>,
    pub definition_error: Option<String>,
    pub stopped: StoppedSet,
    /// The gates' state, with what they hold as it stands after the
    /// iteration's own changes to it.
    pub gates: AppGates,
    pub readiness: Readiness,
}

// ---------------------------------------------------------------------------
// What is kept in memory
// ---------------------------------------------------------------------------

/// The state the runtime keeps in memory for gate checks, shared by the
/// reconciler and the code that drives gates.
#[derive(Debug, Default)]
pub struct GateRuntime {
    inner: Mutex<Memory>,
}

#[derive(Debug, Default)]
struct Memory {
    remembered: BTreeMap<AppName, Remembered>,
    reported: BTreeMap<(AppName, GateName), &'static str>,
    checking: BTreeMap<(AppName, GateName), Arc<CancelToken>>,
    last_checked: BTreeMap<(AppName, GateName), Instant>,
    not_before: BTreeMap<(AppName, GateName), Instant>,
    snapshots: BTreeMap<AppName, GateSnapshot>,
}

impl GateRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// What is remembered of `app`'s gates.
    pub fn remembered(&self, app: &AppName) -> Remembered {
        self.inner
            .lock()
            .remembered
            .get(app)
            .cloned()
            .unwrap_or_default()
    }

    /// A check cleared `gate` for the dependants that start from stopped.
    pub fn clear_for_start(&self, app: &AppName, gate: &GateName, key: KeyDigest) {
        self.inner
            .lock()
            .remembered
            .entry(app.clone())
            .or_default()
            .cleared
            .insert(gate.clone(), key);
    }

    /// No check has cleared `gate` for the dependants that start from stopped.
    pub fn unclear(&self, app: &AppName, gate: &GateName) {
        if let Some(remembered) = self.inner.lock().remembered.get_mut(app) {
            remembered.cleared.remove(gate);
        }
    }

    /// A failed check of `gate` could not be recorded.
    pub fn note_unrecorded(&self, app: &AppName, gate: &GateName, why: String) {
        self.inner
            .lock()
            .remembered
            .entry(app.clone())
            .or_default()
            .unrecorded
            .insert(gate.clone(), why);
    }

    pub fn settle_unrecorded(&self, app: &AppName, gate: &GateName) {
        if let Some(remembered) = self.inner.lock().remembered.get_mut(app) {
            remembered.unrecorded.remove(gate);
        }
    }

    /// Runs of `gate` did not succeed, after any already held that are not yet
    /// written, returning everything now held to be written.
    // spec: GRUN#retries
    pub fn note_failed_runs(
        &self,
        app: &AppName,
        gate: &GateName,
        failed: FailedRuns,
    ) -> FailedRuns {
        let mut inner = self.inner.lock();
        let held = inner.remembered.entry(app.clone()).or_default();
        let failed = match held.failed.remove(gate) {
            Some(earlier) => earlier.followed_by(failed),
            None => failed,
        };
        held.failed.insert(gate.clone(), failed.clone());
        failed
    }

    /// `written`, the failed runs of `gate` held to be written, are on its
    /// record. Runs noted since are still held.
    pub fn settle_failed_runs(&self, app: &AppName, gate: &GateName, written: &FailedRuns) {
        if let Some(remembered) = self.inner.lock().remembered.get_mut(app)
            && remembered.failed.get(gate) == Some(written)
        {
            remembered.failed.remove(gate);
        }
    }

    /// A run of `gate` succeeded, and its success is on record.
    // spec: GRUN#retries
    pub fn forget_failed_runs(&self, app: &AppName, gate: &GateName) {
        if let Some(remembered) = self.inner.lock().remembered.get_mut(app) {
            remembered.failed.remove(gate);
        }
    }

    /// Note the state reported for `gate`, returning the one reported before
    /// it where that differs.
    // spec: EVT#app-events
    pub fn note_state(
        &self,
        app: &AppName,
        gate: &GateName,
        state: &'static str,
    ) -> Option<&'static str> {
        let previous = self
            .inner
            .lock()
            .reported
            .insert((app.clone(), gate.clone()), state);
        previous.filter(|previous| *previous != state)
    }

    /// Forget everything held for `app`.
    pub fn forget_app(&self, app: &AppName) {
        let mut inner = self.inner.lock();
        inner.remembered.remove(app);
        inner.reported.retain(|(a, _), _| a != app);
        inner.checking.retain(|(a, _), _| a != app);
        inner.last_checked.retain(|(a, _), _| a != app);
        inner.not_before.retain(|(a, _), _| a != app);
        inner.snapshots.remove(app);
    }

    /// Keep what this iteration worked out of `app`'s gates for the
    /// reconciler, in place of anything kept before.
    pub fn publish(&self, app: &AppName, snapshot: GateSnapshot) {
        self.inner.lock().snapshots.insert(app.clone(), snapshot);
    }

    /// Take what was kept of `app`'s gates, where it was worked out from the
    /// same definition, definition error and stopped resources as given.
    pub fn take_snapshot(
        &self,
        app: &AppName,
        def: &Arc<AppDef>,
        definition_error: Option<&str>,
        stopped: &StoppedSet,
    ) -> Option<GateSnapshot> {
        self.inner.lock().snapshots.remove(app).filter(|snapshot| {
            Arc::ptr_eq(&snapshot.def, def)
                && snapshot.definition_error.as_deref() == definition_error
                && snapshot.stopped == *stopped
        })
    }

    /// Claim the right to check `gate`, unless a check of it is in flight.
    pub fn begin_check(self: &Arc<Self>, app: &AppName, gate: &GateName) -> Option<CheckClaim> {
        let key = (app.clone(), gate.clone());
        let mut inner = self.inner.lock();
        if inner.checking.contains_key(&key) {
            return None;
        }
        let cancel = Arc::new(CancelToken::new());
        inner.checking.insert(key.clone(), Arc::clone(&cancel));
        Some(CheckClaim {
            runtime: Arc::clone(self),
            key,
            cancel,
        })
    }

    /// Claim the right to check `gate` for a run of it, cancelling the check
    /// of it in flight and waiting for that check to end.
    ///
    /// The wait is as long as the check in flight takes to see its cancel and
    /// record what it reached, which it does without retrying anything.
    // spec: GRUN#checks
    pub async fn claim_for_run(self: &Arc<Self>, app: &AppName, gate: &GateName) -> CheckClaim {
        loop {
            if let Some(claim) = self.begin_check(app, gate) {
                return claim;
            }
            if let Some(cancel) = self.inner.lock().checking.get(&(app.clone(), gate.clone())) {
                cancel.request();
            }
            tokio::time::sleep(CLAIM_POLL).await;
        }
    }

    pub fn is_checking(&self, app: &AppName, gate: &GateName) -> bool {
        self.inner
            .lock()
            .checking
            .contains_key(&(app.clone(), gate.clone()))
    }

    /// A check of `gate` ran to an outcome, now.
    pub fn note_checked(&self, app: &AppName, gate: &GateName) {
        self.inner
            .lock()
            .last_checked
            .insert((app.clone(), gate.clone()), Instant::now());
    }

    pub fn last_checked(&self, app: &AppName, gate: &GateName) -> Option<Instant> {
        self.inner
            .lock()
            .last_checked
            .get(&(app.clone(), gate.clone()))
            .copied()
    }

    /// Hold off checking `gate` until `delay` from now.
    pub fn defer_check(&self, app: &AppName, gate: &GateName, delay: Duration) {
        self.inner
            .lock()
            .not_before
            .insert((app.clone(), gate.clone()), Instant::now() + delay);
    }

    pub fn check_deferred(&self, app: &AppName, gate: &GateName) -> bool {
        self.inner
            .lock()
            .not_before
            .get(&(app.clone(), gate.clone()))
            .is_some_and(|until| Instant::now() < *until)
    }
}

/// How long a run waits between looks at whether the check it cancelled has
/// ended.
const CLAIM_POLL: Duration = Duration::from_millis(50);

/// The right to check one gate, given back when it is dropped.
pub struct CheckClaim {
    runtime: Arc<GateRuntime>,
    key: (AppName, GateName),
    cancel: Arc<CancelToken>,
}

impl CheckClaim {
    /// Requested when a run of the gate needs the claim.
    pub fn cancel(&self) -> &Arc<CancelToken> {
        &self.cancel
    }
}

impl Drop for CheckClaim {
    fn drop(&mut self) {
        let mut inner = self.runtime.inner.lock();
        if inner
            .checking
            .get(&self.key)
            .is_some_and(|held| Arc::ptr_eq(held, &self.cancel))
        {
            inner.checking.remove(&self.key);
        }
    }
}

#[cfg(test)]
thread_local! {
    static READS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// How many times gate state and readiness have been read on this thread,
/// which for a database's own thread is how many times they were read of it.
#[cfg(test)]
pub(crate) fn reads() -> (usize, usize) {
    READS.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests;
