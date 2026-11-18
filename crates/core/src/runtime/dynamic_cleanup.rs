//! Tearing down the dynamic resources of a lifecycle operation or shell
//! session that has ended, while the runtime goes on running.
//!
//! The reconciler does the stopping: the cleanup places each resource in the
//! desired state of the progress slot it is given as stopped, and waits for the
//! world to agree. The slot is the operation's, or the session's.

use std::time::Duration;

use parking_lot::RwLock;
use seedling_protocol::names::AppName;
use tokio::sync::Notify;

use crate::{
    runtime::{db::DbHandle, desired::OperationProgress, dynamic_teardown::STALL_AFTER, faults},
    system::System,
};

/// Piecewise backoff: 2s while waited < 2 min, then ramp to 30s at 1 hour,
/// then ramp to 300s at 6 hours, then cap at 300s.
// spec: SUSP#re-evaluation-cadence
pub(crate) fn dynamic_poll_interval(waited_secs: u64) -> Duration {
    const LOW: u64 = 2;
    const MID: u64 = 30;
    const HIGH: u64 = 300;
    const T_LOW_END: u64 = 120; // 2 min
    const T_MID_END: u64 = 3600; // 1 hour
    const T_HIGH_END: u64 = 21600; // 6 hours

    let poll = if waited_secs <= T_LOW_END {
        LOW
    } else if waited_secs <= T_MID_END {
        let ratio = (waited_secs - T_LOW_END) as f64 / (T_MID_END - T_LOW_END) as f64;
        LOW + ((MID - LOW) as f64 * ratio) as u64
    } else if waited_secs <= T_HIGH_END {
        let ratio = (waited_secs - T_MID_END) as f64 / (T_HIGH_END - T_MID_END) as f64;
        MID + ((HIGH - MID) as f64 * ratio) as u64
    } else {
        HIGH
    };
    Duration::from_secs(poll)
}

/// What keeps the end-of-operation cleanup from finishing.
enum CleanupBlocker {
    /// The records of the operation's dynamic resources cannot be read.
    Unreadable(String),
    /// A dynamic resource is not yet torn down.
    NotTornDown(String),
}

/// Pacing for the end-of-operation cleanup's looks at its dynamic resources,
/// and the fault it holds while they are stalled.
struct CleanupPace<'a> {
    db: &'a DbHandle,
    app: &'a AppName,
    owner_id: &'a str,
    started: tokio::time::Instant,
}

impl CleanupPace<'_> {
    /// Hold `operation_cleanup_stalled` while `blocker` stands, if it is one
    /// that stalls the cleanup now.
    // spec: OPS#completion
    // spec: FLT#app-faults
    fn note(&self, blocker: &CleanupBlocker) {
        let why = match blocker {
            CleanupBlocker::Unreadable(why) => why.clone(),
            CleanupBlocker::NotTornDown(why) if self.started.elapsed() >= STALL_AFTER => {
                why.clone()
            }
            CleanupBlocker::NotTornDown(why) => {
                tracing::debug!(operation_id = %self.owner_id, "dynamic cleanup waiting: {why}");
                return;
            }
        };
        let (app, owner_id) = (self.app.clone(), self.owner_id.to_owned());
        self.db.call(move |db| {
            crate::runtime::faults::hold_operation_cleanup_stalled(db, &app, &owner_id, &why);
        });
    }

    /// Hold the fault `blocker` calls for, then wait before the next look.
    ///
    /// The back-off is [`dynamic_poll_interval`] over the time spent so far:
    /// two seconds at first, ramping to its cap. Every failure here, a read
    /// or a probe, is taken as transient: nothing gives the cleanup up.
    async fn wait(&self, blocker: CleanupBlocker) {
        self.note(&blocker);
        tokio::time::sleep(dynamic_poll_interval(self.started.elapsed().as_secs())).await;
    }
}

/// Tear down the dynamic resources recorded against `owner_id`, the id of an
/// operation or shell session that has ended, and forget their records once
/// they are gone.
///
/// The records are the only thing that knows the resources exist, so neither
/// a record that cannot be read nor a teardown that is taking long ends the
/// cleanup: it keeps looking at a capped back-off until they are gone. A
/// record naming no instance that can be torn down is kept, as the startup
/// teardown keeps it.
// spec: APP#dynamic-resource-lifetime
// spec: OPS#cancellation
pub async fn cleanup_dynamic_resources(
    db: &DbHandle,
    driver: &System,
    app_name: &AppName,
    owner_id: &str,
    active_progress: &RwLock<Option<OperationProgress>>,
    tick_notify: &Notify,
) {
    use crate::defs::resource::ResourceKind;
    use crate::runtime::LifecycleState;
    use crate::runtime::desired::{
        delete_dynamic_resource, list_dynamic_resources_for_operation, undeclared_pod,
    };
    use crate::runtime::history::read_lifecycle;
    use crate::runtime::identity::{InstanceId, InstanceVariant, ResourceInstance};

    let pace = CleanupPace {
        db,
        app: app_name,
        owner_id,
        started: tokio::time::Instant::now(),
    };

    let dynamic_records = loop {
        let owner = owner_id.to_owned();
        match db.call(move |db| list_dynamic_resources_for_operation(db, &owner)) {
            Ok(records) => break records,
            Err(e) => {
                tracing::warn!(
                    operation_id = %owner_id,
                    "could not read the dynamic resources: {e}"
                );
                pace.wait(CleanupBlocker::Unreadable(format!(
                    "the records cannot be read: {e}"
                )))
                .await;
            }
        }
    };

    let mut cleanup = OperationProgress::new();
    // The records whose resources this cleanup tears down, or that have
    // nothing to tear down.
    let mut handled: Vec<(AppName, String)> = Vec::new();
    // A record whose instance cannot be read is kept, and its resource cannot
    // be torn down from it.
    let mut unreadable: Option<String> = None;
    for record in &dynamic_records {
        let kind = match record.kind.as_str() {
            "Deployment" => ResourceKind::Deployment,
            "Job" => ResourceKind::Job,
            // Not a pod: it stops being served when the operation's desired
            // state no longer holds it.
            _ => {
                handled.push((record.app.clone(), record.instance_id.clone()));
                continue;
            }
        };
        let Some(id) = InstanceId::from_hex(&record.instance_id) else {
            tracing::error!(
                instance_id = %record.instance_id,
                operation_id = %owner_id,
                "dynamic cleanup: the recorded instance id does not parse; keeping its record"
            );
            unreadable = Some(format!(
                "the recorded instance id {:?} does not parse",
                record.instance_id
            ));
            continue;
        };
        let instance = ResourceInstance {
            id,
            app: record.app.clone(),
            kind,
            name: None,
            variant: InstanceVariant::Singleton,
            display_name: record.display_name.clone(),
        };
        let Some(minimal) = undeclared_pod(&instance) else {
            continue;
        };
        cleanup.stopped(instance.clone());
        cleanup.dynamic_defs.insert(instance, minimal);
        handled.push((record.app.clone(), record.instance_id.clone()));
    }

    if !cleanup.is_empty() {
        let cleanup_instances: Vec<ResourceInstance> =
            cleanup.dynamic_defs.keys().cloned().collect();
        *active_progress.write() = Some(cleanup);
        tick_notify.notify_one();

        loop {
            let mut waiting_on = None;
            for inst in &cleanup_instances {
                let lifecycle = {
                    let inst = inst.clone();
                    db.call(move |db| read_lifecycle(db, &inst))
                };
                match lifecycle {
                    Ok((s, _)) if s.has_reached(LifecycleState::Terminated) => {}
                    Ok((s, _)) => {
                        waiting_on = Some(format!("{} is {s:?}", inst.display_name));
                        break;
                    }
                    Err(e) => {
                        waiting_on = Some(format!(
                            "the lifecycle of {} cannot be read: {e}",
                            inst.display_name
                        ));
                        break;
                    }
                }
                // A container with `podman --rm` can reach the Unscheduled
                // lifecycle state via `container_removed` before the
                // reconciler has had a chance to remove the pod network.
                // Ending the cleanup at that point leaks the network and the
                // next Job on the same app trips over "subnet already used".
                // Require every pod network to also be gone.
                let net_name = format!("seedling-{}", inst.display_name);
                match driver.container.network_exists(&net_name).await {
                    Ok(false) => {}
                    Ok(true) => {
                        waiting_on = Some(format!("network {net_name} still exists"));
                        break;
                    }
                    Err(e) => {
                        waiting_on = Some(format!(
                            "whether network {net_name} still exists cannot be told: {e}"
                        ));
                        break;
                    }
                }
            }
            let Some(why) = waiting_on else {
                break;
            };
            pace.wait(CleanupBlocker::NotTornDown(why)).await;
            tick_notify.notify_one();
        }
    }

    // spec: FLT#workload-faults
    // Clear per-instance faults for every dynamic resource that we just
    // tore down. Without this, faults filed against short-lived Job
    // instances (image_pull_failed, container_start_failed, …) persist
    // forever because the instance id they reference has gone away.
    // spec: OPS#completion
    // spec: FLT#app-faults
    match unreadable {
        Some(why) => pace.note(&CleanupBlocker::Unreadable(why)),
        None => {
            let (app, operation_id) = (app_name.clone(), owner_id.to_owned());
            db.call(move |db| {
                faults::clear_operation_cleanup_stalled(db, &app, &operation_id);
            });
        }
    }
    db.call(move |db| {
        for (app, instance_id) in &handled {
            if let Err(e) = faults::clear_faults_for_instance(db, app, instance_id) {
                tracing::warn!(
                    app = %app,
                    instance_id = %instance_id,
                    "failed to clear per-instance faults during dynamic cleanup: {e}"
                );
            }
            if let Err(e) = delete_dynamic_resource(db, instance_id) {
                tracing::error!(
                    instance_id = %instance_id,
                    "failed to delete dynamic resource record: {e}"
                );
            }
        }
    });
}
