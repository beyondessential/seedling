use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use seedling_protocol::names::AppName;

use super::{AppEntry, AppPhase, AppRegistry, AppStatus};
use crate::{
    defs::resource::{Resource, ResourceKind},
    runtime::{
        barrier::oracle::{derive_lifecycle_state, has_lifecycle},
        db::{Db, DbHandle},
        faults,
        history::{find_instances_for_group, query_observations},
        identity::ResourceInstance,
        lifecycle::LifecycleState,
        scaling,
        scheduler::{ActiveOperation, Scheduler},
        stopped,
    },
};

/// The operation the scheduler is running, taken once so that a status and
/// whatever is reported about its operation describe the same one.
pub fn active_operation(scheduler: &Mutex<Scheduler>) -> Option<ActiveOperation> {
    scheduler.lock().active().cloned()
}

/// The status of every registered app, sorted by name.
pub fn all(
    registry: &RwLock<AppRegistry>,
    scheduler: &Mutex<Scheduler>,
    db: &DbHandle,
) -> Vec<(AppName, AppStatus)> {
    // Taken before the registry: the scheduler lock is never held while
    // waiting on the registry.
    let active = active_operation(scheduler);
    let reg = registry.read();
    reg.names()
        .into_iter()
        .filter_map(|name| {
            let entry = reg.get(name.as_str())?;
            Some((name, of(entry, active.as_ref(), db)))
        })
        .collect()
}

// i[impl app.status] i[impl app.status.priority]
/// The status of one app.
///
/// `active` is the scheduler's active operation, from [`active_operation`].
/// The phase resolves the states that outrank an operation; an operation the
/// scheduler is running for the app is the only thing that makes it
/// Operating, because the scheduler is what knows which action it is.
pub fn of(entry: &AppEntry, active: Option<&ActiveOperation>, db: &DbHandle) -> AppStatus {
    let phase = entry.phase.lock().clone();
    match phase {
        AppPhase::Uninstalling => AppStatus::Uninstalling,
        AppPhase::Installing => AppStatus::Installing,
        AppPhase::NotInstalled => AppStatus::NotInstalled,
        AppPhase::Installed => match active.filter(|a| a.app == entry.name) {
            Some(op) => AppStatus::Operating {
                action_name: op.action.clone(),
            },
            None => steady(entry, db),
        },
    }
}

/// Faulted, Degraded or Running, for an installed app with no operation.
fn steady(entry: &AppEntry, db: &DbHandle) -> AppStatus {
    let app_name = entry.name.clone();

    // Collected with a brief def lock, released before touching the database.
    let resources: Vec<(ResourceKind, Arc<String>, Option<(u16, u16)>)> = {
        let def = entry.app.def.load();
        def.resources
            .iter()
            .map(|(id, resource)| {
                let bounds = match resource {
                    Resource::Deployment(d) => {
                        let scale = &d.def.lock().scale;
                        Some((scale.start, scale.end))
                    }
                    _ => None,
                };
                (id.kind, Arc::clone(&id.name), bounds)
            })
            .collect()
    };

    let result = db.call({
        let app_name = app_name.clone();
        move |db| -> rusqlite::Result<AppStatus> {
            if faults::has_active_faults(db, &app_name)? {
                return Ok(AppStatus::Faulted);
            }
            let stopped = stopped::load_stopped(db, &app_name)?;
            for (kind, name, bounds) in &resources {
                if !has_lifecycle(*kind) {
                    continue;
                }
                // i[impl resource.stop]
                // As the steady desired state has it: a stopped resource, or
                // a deployment scaled to zero, is meant to have nothing Ready.
                let wants_ready = if stopped.contains(&(*kind, name.as_str().to_owned())) {
                    false
                } else if let Some((low, high)) = bounds {
                    scaling::effective_scale(db, &app_name, name, *low, *high)? > 0
                } else {
                    true
                };
                let instances = find_instances_for_group(db, &app_name, *kind, Some(name))?;
                if !at_desired_state(db, &instances, wants_ready)? {
                    return Ok(AppStatus::Degraded);
                }
            }
            Ok(AppStatus::Running)
        }
    });

    result.unwrap_or_else(|e| {
        // Running is a claim that everything was checked. With a read
        // failed nothing was, and Degraded is the status that has an
        // operator look.
        tracing::warn!(app = %app_name, "could not read the app's state to derive its status: {e}");
        AppStatus::Degraded
    })
}

/// Whether a resource's instances are where its desired state puts them.
fn at_desired_state(
    db: &Db,
    instances: &[ResourceInstance],
    wants_ready: bool,
) -> rusqlite::Result<bool> {
    let mut has_ready = false;
    for inst in instances {
        let obs = query_observations(db, inst)?;
        match derive_lifecycle_state(inst, &obs) {
            LifecycleState::Ready if wants_ready => has_ready = true,
            // Instances being torn down are on their way to Unscheduled, which
            // is where a stopped resource belongs and where a replaced
            // instance is going, so they do not drag the app to Degraded.
            LifecycleState::Unscheduled
            | LifecycleState::Terminating
            | LifecycleState::Terminated => {}
            _ => return Ok(false),
        }
    }
    Ok(has_ready || !wants_ready)
}
