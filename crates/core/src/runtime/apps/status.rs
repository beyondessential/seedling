use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use seedling_protocol::names::AppName;

use super::{AppEntry, AppPhase, AppRegistry, AppStatus};
use crate::{
    defs::resource::{Resource, ResourceKind},
    runtime::{
        barrier::oracle::{container_run_succeeded, derive_lifecycle_state, has_lifecycle},
        db::{Db, DbHandle},
        faults,
        history::{find_instances_for_group, query_observations_for},
        identity::ResourceInstance,
        lifecycle::LifecycleState,
        scaling,
        scheduler::{ActiveOperation, Scheduler},
        stopped::{self, StoppedSet},
    },
};

/// The operation the scheduler is running, taken once so that a status and
/// whatever is reported about its operation describe the same one.
pub fn active_operation(scheduler: &Mutex<Scheduler>) -> Option<ActiveOperation> {
    scheduler.lock().active().cloned()
}

/// The status of every registered app, sorted by name.
///
/// The registry is read only to take a snapshot, and released before every
/// app is read from the database in one pass.
pub fn all(
    registry: &RwLock<AppRegistry>,
    scheduler: &Mutex<Scheduler>,
    db: &DbHandle,
) -> Vec<(AppName, AppStatus)> {
    let (active, snapshots) = snapshot_all(registry, scheduler, |_| ());
    db.call(move |db| {
        snapshots
            .into_iter()
            .map(|(snapshot, ())| {
                let status = status_in(db, &snapshot, active.as_ref());
                (snapshot.name, status)
            })
            .collect()
    })
}

/// An app's status alongside what a listing reports of its standing.
pub struct Standing {
    pub status: AppStatus,
    pub has_stopped: bool,
    pub fault_count: i64,
}

/// The standing of every registered app, sorted by name, each with what
/// `extra` takes from its registry entry while the snapshot is taken.
///
/// Read in one pass, like [`all`], but a failed read fails the whole call: a
/// listing that defaulted it would report no faults and nothing stopped.
pub fn standings<T: Send + 'static>(
    registry: &RwLock<AppRegistry>,
    scheduler: &Mutex<Scheduler>,
    db: &DbHandle,
    extra: impl Fn(&AppEntry) -> T,
) -> rusqlite::Result<Vec<(AppName, Standing, T)>> {
    let (active, snapshots) = snapshot_all(registry, scheduler, extra);
    db.call(move |db| {
        snapshots
            .into_iter()
            .map(|(snapshot, extra)| {
                let fault_count = faults::count_active_faults_for_app(db, &snapshot.name)?;
                let stopped = stopped::load_stopped(db, &snapshot.name)?;
                let status = match settled(&snapshot, active.as_ref()) {
                    Some(status) => status,
                    None => steady(db, &snapshot, fault_count > 0, &stopped)?,
                };
                let standing = Standing {
                    status,
                    has_stopped: !stopped.is_empty(),
                    fault_count,
                };
                Ok((snapshot.name, standing, extra))
            })
            .collect()
    })
}

// spec: APPS#status-derivation
/// The status of one app.
///
/// `active` is the scheduler's active operation, from [`active_operation`].
/// The phase resolves the states that outrank an operation; an operation the
/// scheduler is running for the app is the only thing that makes it
/// Operating, because the scheduler is what knows which action it is.
pub fn of(entry: &AppEntry, active: Option<&ActiveOperation>, db: &DbHandle) -> AppStatus {
    let snapshot = Snapshot::of(entry);
    let active = active.cloned();
    db.call(move |db| status_in(db, &snapshot, active.as_ref()))
}

/// What the rollup reads of one app from the registry, so that the database
/// work runs with the registry released.
struct Snapshot {
    name: AppName,
    phase: AppPhase,
    resources: Vec<Declared>,
}

impl Snapshot {
    fn of(entry: &AppEntry) -> Self {
        let phase = entry.phase.lock().clone();
        // Only an installed app's resources are judged.
        let resources = if matches!(phase, AppPhase::Installed) {
            declared(entry)
        } else {
            Vec::new()
        };
        Self {
            name: entry.name.clone(),
            phase,
            resources,
        }
    }
}

/// The scheduler's active operation and a snapshot of every registered app,
/// sorted by name, each with what `extra` takes from its entry.
fn snapshot_all<T>(
    registry: &RwLock<AppRegistry>,
    scheduler: &Mutex<Scheduler>,
    extra: impl Fn(&AppEntry) -> T,
) -> (Option<ActiveOperation>, Vec<(Snapshot, T)>) {
    // Taken before the registry: the scheduler lock is never held while
    // waiting on the registry.
    let active = active_operation(scheduler);
    let reg = registry.read();
    let snapshots = reg
        .names()
        .into_iter()
        .filter_map(|name| reg.get(name.as_str()))
        .map(|entry| (Snapshot::of(entry), extra(entry)))
        .collect();
    (active, snapshots)
}

/// The status the phase or the scheduler settles without the database, or
/// `None` for an installed app with no operation, whose status is its health.
fn settled(snapshot: &Snapshot, active: Option<&ActiveOperation>) -> Option<AppStatus> {
    match snapshot.phase {
        AppPhase::Uninstalling => Some(AppStatus::Uninstalling),
        AppPhase::Installing => Some(AppStatus::Installing),
        AppPhase::NotInstalled => Some(AppStatus::NotInstalled),
        AppPhase::Installed => {
            active
                .filter(|a| a.app == snapshot.name)
                .map(|op| AppStatus::Operating {
                    action_name: op.action.clone(),
                })
        }
    }
}

fn status_in(db: &Db, snapshot: &Snapshot, active: Option<&ActiveOperation>) -> AppStatus {
    settled(snapshot, active).unwrap_or_else(|| {
        let read = faults::has_active_faults(db, &snapshot.name).and_then(|faulted| {
            let stopped = stopped::load_stopped(db, &snapshot.name)?;
            steady(db, snapshot, faulted, &stopped)
        });
        read.unwrap_or_else(|e| {
            // Running is a claim that everything was checked. With a read
            // failed nothing was, and Degraded is the status that has an
            // operator look.
            tracing::warn!(app = %snapshot.name, "could not read the app's state to derive its status: {e}");
            AppStatus::Degraded
        })
    })
}

/// What the status rollup reads of one declared resource.
struct Declared {
    kind: ResourceKind,
    name: Arc<String>,
    /// A deployment's scale bounds; `None` for every other kind.
    scale_bounds: Option<(u16, u16)>,
}

fn declared(entry: &AppEntry) -> Vec<Declared> {
    let def = entry.app.def.load();
    def.resources
        .iter()
        .filter(|(id, _)| has_lifecycle(id.kind))
        .map(|(id, resource)| {
            let scale_bounds = match resource {
                Resource::Deployment(d) => {
                    let scale = &d.def.lock().scale;
                    Some((scale.start, scale.end))
                }
                _ => None,
            };
            Declared {
                kind: id.kind,
                name: Arc::clone(&id.name),
                scale_bounds,
            }
        })
        .collect()
}

/// Faulted, Degraded or Running, for an installed app with no operation.
fn steady(
    db: &Db,
    snapshot: &Snapshot,
    faulted: bool,
    stopped: &StoppedSet,
) -> rusqlite::Result<AppStatus> {
    if faulted {
        return Ok(AppStatus::Faulted);
    }
    let app_name = &snapshot.name;
    for Declared {
        kind,
        name,
        scale_bounds,
    } in &snapshot.resources
    {
        // spec: CTL#resource-stop
        // As the steady desired state has it: a stopped resource, or a
        // deployment scaled to zero, is meant to have nothing Ready.
        let desired = if stopped.contains(&(*kind, name.as_str().to_owned())) {
            Desired::Unscheduled
        } else if let Some((low, high)) = scale_bounds {
            if scaling::effective_scale(db, app_name, name, *low, *high)? > 0 {
                Desired::Ready
            } else {
                Desired::Unscheduled
            }
        } else if *kind == ResourceKind::Job {
            Desired::Run
        } else {
            Desired::Ready
        };
        let instances = find_instances_for_group(db, app_name, *kind, Some(name))?;
        if !at_desired_state(db, &instances, desired)? {
            return Ok(AppStatus::Degraded);
        }
    }
    Ok(AppStatus::Running)
}

/// Where a resource's desired state puts its instances.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Desired {
    Ready,
    Unscheduled,
    /// A Job, which runs once: Ready while it runs, then done once its run
    /// has succeeded.
    Run,
}

/// Whether a resource's instances are where its desired state puts them.
fn at_desired_state(
    db: &Db,
    instances: &[ResourceInstance],
    desired: Desired,
) -> rusqlite::Result<bool> {
    let mut reached = false;
    let histories = query_observations_for(db, instances)?;
    for inst in instances {
        // Every instance asked for has an entry, empty when it has no history.
        let obs = histories
            .get(&inst.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        match (desired, derive_lifecycle_state(inst, obs)) {
            (Desired::Ready | Desired::Run, LifecycleState::Ready) => reached = true,
            // A Job that has run rests here, removed or not.
            (Desired::Run, LifecycleState::Terminated | LifecycleState::Unscheduled) => {
                match container_run_succeeded(obs) {
                    Some(true) => reached = true,
                    Some(false) => return Ok(false),
                    None => {}
                }
            }
            // Instances being torn down are on their way to Unscheduled, which
            // is where a stopped resource belongs and where a replaced
            // instance is going, so they do not drag the app to Degraded.
            (
                _,
                LifecycleState::Unscheduled
                | LifecycleState::Terminating
                | LifecycleState::Terminated,
            ) => {}
            _ => return Ok(false),
        }
    }
    Ok(reached || desired == Desired::Unscheduled)
}
