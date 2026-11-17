//! Tearing down the dynamic resources a previous run of the runtime recorded,
//! from their records alone, and going on tearing down those that do not go
//! at once.
//!
//! The reconciler does not do this: these instances are absent from every
//! desired state, so it ignores them rather than stopping them.

use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
    time::Duration,
};

use parking_lot::Mutex;
use seedling_protocol::names::AppName;

use crate::{
    runtime::{
        db::DbHandle,
        desired::{DynamicResourceRecord, list_dynamic_resources},
        faults,
        identity::InstanceId,
        retry::RetryGate,
        scheduler::Scheduler,
    },
    system::{System, actuator},
};

/// How long the resources may take to go before `operation_cleanup_stalled`
/// is held for their operations.
// spec: OPS#completion
pub const STALL_AFTER: Duration = Duration::from_secs(60);

/// The first delay between attempts, doubled after each that leaves anything.
const RETRY_BASE: Duration = Duration::from_secs(2);

/// The longest delay between attempts.
const RETRY_CAP: Duration = Duration::from_secs(300);

/// Tear one dynamic resource down in the world from its record: stop and
/// reset its unit, remove its pod network, force-remove a container that
/// outlived it, and remove the anonymous volumes recorded for it.
///
/// Returns whether the world gave the resource up. A teardown that failed, or
/// one whose outcome could not be established, is not a teardown.
// spec: SUSP#abandoned-replay
async fn tear_down_one(driver: &System, db: &DbHandle, record: &DynamicResourceRecord) -> bool {
    tracing::info!(
        instance = %record.display_name,
        kind = %record.kind,
        operation_id = %record.operation_id,
        "tearing down dynamic resource"
    );
    let unit = format!("seedling-{}.service", record.display_name);
    let net_name = format!("seedling-{}", record.display_name);
    let container_name = &record.display_name;
    let mut torn_down = true;
    match driver.process.unit_state(&unit).await {
        Ok(Some(_)) => {
            if let Err(e) = driver.process.stop_unit(&unit).await {
                tracing::warn!(unit = %unit, "could not stop the unit: {e}");
                torn_down = false;
            } else if let Err(e) = driver.process.reset_failed_unit(&unit).await {
                tracing::warn!(unit = %unit, "could not reset the unit: {e}");
                torn_down = false;
            }
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(unit = %unit, "could not tell whether the unit is still loaded: {e}");
            torn_down = false;
        }
    }
    match driver.container.network_exists(&net_name).await {
        Ok(true) => {
            if let Err(e) = driver.container.remove_network(&net_name).await {
                tracing::warn!(network = %net_name, "could not remove the network: {e}");
                torn_down = false;
            }
        }
        Ok(false) => {}
        Err(e) => {
            tracing::warn!(
                network = %net_name,
                "could not tell whether the network is still there: {e}"
            );
            torn_down = false;
        }
    }
    // Asked for first, rather than removing and discarding the error: the
    // runtime reports a container that is not there as an error, and a
    // dynamic resource's container has routinely already gone, removed on
    // exit. Discarding it also discards the failure to remove one that is
    // still there, against a runtime answering perfectly well.
    match driver.container.inspect(container_name).await {
        Ok(None) => {}
        Ok(Some(_)) => {
            if let Err(e) = driver
                .container
                .remove_container(container_name, true)
                .await
            {
                tracing::warn!(container = %container_name, "could not remove the container: {e}");
                torn_down = false;
            }
        }
        Err(e) => {
            tracing::warn!(
                container = %container_name,
                "could not tell whether the container is still there: {e}"
            );
            torn_down = false;
        }
    }
    // spec: ACTU#deployment-and-job-instances
    // The anonymous volumes created for the instance go with it, found by the
    // identity they were recorded against.
    match InstanceId::from_hex(&record.instance_id) {
        Some(id) => {
            if let Err(e) = actuator::remove_anonymous_volumes(driver, db, id, &[]).await {
                tracing::warn!(
                    instance = %record.display_name,
                    "could not remove the anonymous volumes: {e}"
                );
                torn_down = false;
            }
        }
        None => {
            tracing::warn!(
                instance_id = %record.instance_id,
                "the recorded instance id does not parse, so its anonymous volumes cannot be found"
            );
            torn_down = false;
        }
    }
    torn_down
}

/// The operations of `records`, each with its app.
fn operations_of(records: &[DynamicResourceRecord]) -> BTreeSet<(AppName, String)> {
    records
        .iter()
        .map(|r| (r.app.clone(), r.operation_id.clone()))
        .collect()
}

/// Tear `records` down in the world and forget those torn down, returning
/// the ones that are not.
///
/// Only records actually torn down are deleted, with the faults a previous
/// run filed against their instances; one whose teardown failed stays on the
/// books rather than being written off as a teardown that did not happen.
/// `operation_cleanup_stalled` is cleared for each operation none of whose
/// records is left.
// spec: SUSP#abandoned-replay
// spec: OPS#completion
// spec: FLT#workload-faults
pub async fn tear_down(
    driver: &System,
    db: &DbHandle,
    records: Vec<DynamicResourceRecord>,
) -> Vec<DynamicResourceRecord> {
    let operations = operations_of(&records);
    let mut handled: Vec<(AppName, String)> = Vec::new();
    let mut retained = Vec::new();
    for record in records {
        if tear_down_one(driver, db, &record).await {
            handled.push((record.app.clone(), record.instance_id.clone()));
        } else {
            tracing::warn!(
                instance = %record.display_name,
                operation_id = %record.operation_id,
                "dynamic resource not torn down; keeping its record"
            );
            retained.push(record);
        }
    }
    let finished: Vec<(AppName, String)> = operations
        .difference(&operations_of(&retained))
        .cloned()
        .collect();
    db.call(move |db| {
        for (app, instance_id) in &handled {
            if let Err(e) = faults::clear_faults_for_instance(db, app, instance_id) {
                tracing::warn!(
                    %app,
                    instance_id = %instance_id,
                    "failed to clear per-instance faults while tearing down dynamic resources: {e}"
                );
            }
            if let Err(e) = crate::runtime::desired::delete_dynamic_resource(db, instance_id) {
                tracing::warn!(
                    instance_id = %instance_id,
                    "failed to delete dynamic resource record: {e}"
                );
            }
        }
        for (app, operation_id) in &finished {
            faults::clear_operation_cleanup_stalled(db, app, operation_id);
        }
    });
    retained
}

/// Clear `operation_cleanup_stalled` for every operation that has no dynamic
/// resource record left, given `left`, every record there is but those of the
/// operations in `live`, whose resources are not to be torn down.
///
/// A fault that outlived its operation's last record, as across a restart
/// between forgetting the record and clearing the fault, is cleared here.
// spec: OPS#completion
// spec: FLT#app-faults
pub fn clear_finished_operations(
    db: &DbHandle,
    left: &[DynamicResourceRecord],
    live: &HashSet<String>,
) {
    let left = operations_of(left);
    let live = live.clone();
    db.call(move |db| {
        let active = match faults::list_active_faults(db, None) {
            Ok(active) => active,
            Err(e) => {
                tracing::warn!(
                    "could not read the faults to clear {}: {e}",
                    faults::OPERATION_CLEANUP_STALLED
                );
                return;
            }
        };
        for fault in active {
            if fault.kind != faults::OPERATION_CLEANUP_STALLED
                || live.contains(&fault.subject)
                || left.contains(&(fault.app.clone(), fault.subject.clone()))
            {
                continue;
            }
            faults::clear_operation_cleanup_stalled(db, &fault.app, &fault.subject);
        }
    });
}

/// The operations whose dynamic resources are not to be torn down here: the
/// one running, and those queued, which includes an operation being replayed.
fn live_operations(scheduler: &Mutex<Scheduler>) -> HashSet<String> {
    let scheduler = scheduler.lock();
    scheduler
        .active()
        .map(|op| op.operation_id.0.clone())
        .into_iter()
        .chain(scheduler.queue_iter().map(|op| op.operation_id.0.clone()))
        .collect()
}

/// Keep tearing down dynamic resources a first teardown left, until none is
/// left.
///
/// `left` holds the records that were not torn down, or is `None` where the
/// records could not be read, in which case every record of an operation that
/// is neither running nor queued is read again and torn down.
///
/// Back-off: a [`RetryGate`] doubling from two seconds to five minutes.
/// Every failure, of a read or of a teardown, is taken as transient, and
/// nothing gives the teardown up. Once [`STALL_AFTER`] has passed with any
/// left, `operation_cleanup_stalled` is held for each operation still with
/// records, and cleared as the last of each goes.
// spec: OPS#completion
// spec: SUSP#abandoned-replay
pub async fn keep_tearing_down(
    driver: Arc<System>,
    db: DbHandle,
    scheduler: Arc<Mutex<Scheduler>>,
    mut left: Option<Vec<DynamicResourceRecord>>,
) {
    let started = tokio::time::Instant::now();
    let mut gate = RetryGate::new(RETRY_BASE, RETRY_CAP);
    loop {
        match &left {
            Some(records) if records.is_empty() => return,
            Some(records) if started.elapsed() >= STALL_AFTER => {
                let operations = operations_of(records);
                db.call(move |db| {
                    for (app, operation_id) in &operations {
                        // spec: FLT#app-faults
                        faults::hold_operation_cleanup_stalled(
                            db,
                            app,
                            operation_id,
                            "its dynamic resources could not be torn down",
                        );
                    }
                });
            }
            Some(_) => {}
            None if started.elapsed() >= STALL_AFTER => {
                tracing::error!(
                    "the records of the dynamic resources to tear down still cannot be read; \
                     still trying"
                );
            }
            None => {}
        }
        gate.record_failure(std::time::Instant::now());
        tokio::time::sleep(gate.delay()).await;

        let records = match left.take() {
            Some(records) => records,
            None => match db.call(list_dynamic_resources) {
                Ok(all) => {
                    let live = live_operations(&scheduler);
                    all.into_iter()
                        .filter(|r| !live.contains(&r.operation_id))
                        .collect()
                }
                Err(e) => {
                    tracing::warn!("could not read the dynamic resources to tear down: {e}");
                    continue;
                }
            },
        };
        left = Some(tear_down(&driver, &db, records).await);
    }
}

#[cfg(test)]
mod tests;
