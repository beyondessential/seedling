use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use seedling_protocol::names::AppName;

use super::{AppEntry, AppPhase, AppRegistry, AppStatus};
use crate::{
    defs::resource::ResourceKind,
    runtime::{
        barrier::oracle::derive_lifecycle_state,
        db::DbHandle,
        faults,
        history::{find_instances_for_group, query_observations},
        lifecycle::LifecycleState,
        scheduler::{ActiveOperation, Scheduler},
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

    // Collect resource IDs with a brief def lock, then release before touching db.
    let resource_ids: Vec<(ResourceKind, Arc<String>)> = {
        let def = entry.app.def.load();
        def.resources
            .keys()
            .map(|id| (id.kind, Arc::clone(&id.name)))
            .collect()
    };

    db.call(move |db| {
        let has_faults = faults::has_active_faults(db, &app_name).unwrap_or(false);

        let all_ready = resource_ids.iter().all(|(kind, name)| {
            let instances = find_instances_for_group(db, &app_name, *kind, Some(name.as_str()))
                .unwrap_or_default();
            if instances.is_empty() {
                return false;
            }
            let mut has_ready = false;
            for inst in &instances {
                let obs = query_observations(db, inst).unwrap_or_default();
                let state = derive_lifecycle_state(inst, &obs);
                if state == LifecycleState::Ready {
                    has_ready = true;
                } else if matches!(
                    state,
                    LifecycleState::Unscheduled
                        | LifecycleState::Terminating
                        | LifecycleState::Terminated
                ) {
                    // Instances being torn down must not drag the app to Degraded.
                } else {
                    return false;
                }
            }
            has_ready
        });

        if has_faults {
            AppStatus::Faulted
        } else if all_ready {
            AppStatus::Running
        } else {
            AppStatus::Degraded
        }
    })
}
