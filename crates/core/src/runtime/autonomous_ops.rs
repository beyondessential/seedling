//! Provenance recording for reconciler-driven autonomous operations.
//!
//! Every autonomous action the reconciler takes (start a replacement after
//! a container exit, scale up/down a Deployment, recreate broken networking,
//! etc.) must be logged with its provenance before execution and updated
//! with its outcome afterwards.

use crate::runtime::{
    db::DbHandle,
    history::{self, Provenance},
    identity::ResourceInstance,
};

/// Handle returned by [`record`]; call [`AutonomousOpHandle::complete`] when
/// the operation finishes to attach the outcome.
#[must_use = "autonomous operation outcome must be recorded; call .complete()"]
pub struct AutonomousOpHandle {
    db: DbHandle,
    /// Row id of the inserted autonomous_operations entry, or `None` if
    /// recording failed (in which case completion is a no-op). The
    /// reconciler should not abort the actual operation just because the
    /// log write failed — the system needs to keep converging.
    id: Option<i64>,
}

impl AutonomousOpHandle {
    pub fn complete(self, outcome: &str) {
        let Some(id) = self.id else { return };
        let outcome = outcome.to_owned();
        self.db.call(move |db| {
            if let Err(e) = history::complete_autonomous_operation(db, id, &outcome) {
                tracing::warn!(error = %e, id, "failed to complete autonomous op");
            }
        });
    }
}

/// How many of the instance's most recent observations an operation's
/// provenance names.
const PROVENANCE_OBSERVATIONS: usize = 5;

// spec: AUTO#operation-provenance
// spec: HIST#autonomous-operations
/// Record the start of an autonomous operation. Must be called before the
/// underlying action (start/stop/reconfigure/etc.) is initiated. Returns a
/// handle whose [`complete`](AutonomousOpHandle::complete) method records
/// the outcome, or `None` when the operation could not be recorded, in which
/// case the caller MUST NOT perform it: an operation that is not on record
/// cannot be backed off from, or shown to the operator.
///
/// `operation` is a short identifier of the action (e.g. `"restart"`,
/// `"scale_down_excess"`, `"job_terminal_stop"`). `rule` is a free-form
/// description of why the action was taken — typically a reference to the
/// observation(s) that triggered it together with the policy or invariant
/// that mandates the action. The provenance also names the observations the
/// runtime held of the instance when it acted.
pub fn record(
    db: &DbHandle,
    instance: &ResourceInstance,
    operation: &str,
    rule: &str,
) -> Option<AutonomousOpHandle> {
    let inst_id = instance.id;
    let inst_display = instance.display_name.clone();
    let op = operation.to_owned();
    let rule = rule.to_owned();
    let id = db.call(move |db| {
        let observation_ids =
            history::recent_observation_ids(db, inst_id, PROVENANCE_OBSERVATIONS).unwrap_or_default();
        let prov = Provenance {
            observation_ids,
            rule,
        };
        history::insert_autonomous_operation(db, inst_id, &op, &prov)
            .map_err(|e| {
                tracing::warn!(
                    instance = %inst_display,
                    error = %e,
                    "failed to record autonomous op; not performing it",
                );
                e
            })
            .ok()
    })?;
    Some(AutonomousOpHandle {
        db: db.clone(),
        id: Some(id),
    })
}

/// How long `instance` must wait before `operation` is repeated, from the
/// autonomous operations log, or `None` if it may proceed now.
///
/// Repeats of one operation on one instance are spaced with a delay that grows
/// with each, to a bound, so a container that exits at once is not restarted
/// at full speed.
// spec: HIST#autonomous-operations
pub fn back_off(
    db: &DbHandle,
    instance: &ResourceInstance,
    operation: &str,
) -> Option<std::time::Duration> {
    let inst_id = instance.id;
    let recent = db
        .call(move |db| history::query_autonomous_operations(db, inst_id))
        .ok()?;
    crate::runtime::scheduler::should_back_off(
        inst_id,
        operation,
        &recent,
        std::time::SystemTime::now(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::resource::ResourceKind;
    use crate::runtime::db::Db;
    use crate::runtime::history::query_autonomous_operations;
    use crate::runtime::identity::ResourceInstance;

    fn dep(app: &str, name: &str) -> ResourceInstance {
        ResourceInstance::new_singleton(
            seedling_protocol::names::AppName::new(app).unwrap(),
            ResourceKind::Deployment,
            name,
        )
    }

    // spec: AUTO#operation-provenance
    #[test]
    fn record_inserts_entry_before_returning_handle() {
        let db = DbHandle::from_db(Db::open_in_memory().unwrap());
        let instance = dep("app", "web");

        let handle = record(
            &db,
            &instance,
            "restart",
            "Container exited; OnExit policy is Restart",
        )
        .expect("the operation is recorded");

        // Before complete() is called, the row exists with no outcome —
        // proving that recording happens BEFORE execution as the spec
        // requires.
        let entries = db
            .call(move |db| query_autonomous_operations(db, instance.id))
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].operation, "restart");
        assert!(entries[0].outcome.is_none());
        assert_eq!(
            entries[0].provenance["rule"].as_str(),
            Some("Container exited; OnExit policy is Restart"),
        );

        handle.complete("ok");
    }

    // spec: AUTO#operation-provenance
    #[test]
    fn complete_attaches_outcome() {
        let db = DbHandle::from_db(Db::open_in_memory().unwrap());
        let instance = dep("app", "web");

        let handle = record(&db, &instance, "scale_down_stop", "scale=2 observed=3")
            .expect("the operation is recorded");
        handle.complete("ok");

        let entries = db
            .call(move |db| query_autonomous_operations(db, instance.id))
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].outcome.as_deref(), Some("ok"));
        assert!(entries[0].completed_at.is_some());
    }

    // spec: HIST#operation-provenance
    #[test]
    fn provenance_names_the_observations_held_of_the_instance() {
        let db = DbHandle::from_db(Db::open_in_memory().unwrap());
        let instance = dep("app", "web");
        for kind in ["container_running", "container_exited"] {
            let instance = instance.clone();
            db.call(move |db| {
                history::insert_observation(db, &instance, kind, &serde_json::json!({})).unwrap();
            });
        }

        record(&db, &instance, "start", "exited").unwrap().complete("ok");

        let entries = db
            .call(move |db| query_autonomous_operations(db, instance.id))
            .unwrap();
        assert_eq!(entries[0].provenance["observations"].as_array().unwrap().len(), 2);
    }

    // spec: HIST#autonomous-operations
    #[test]
    fn an_operation_that_cannot_be_recorded_is_not_to_be_performed() {
        let db = DbHandle::from_db(Db::open_in_memory().unwrap());
        db.call(|db| db.conn.execute_batch("DROP TABLE autonomous_operations;").unwrap());
        assert!(record(&db, &dep("app", "web"), "start", "rule").is_none());
    }

    // spec: HIST#autonomous-operations
    #[test]
    fn a_repeated_operation_on_one_instance_is_backed_off_with_a_growing_delay() {
        let db = DbHandle::from_db(Db::open_in_memory().unwrap());
        let instance = dep("app", "web");
        assert!(back_off(&db, &instance, "start").is_none(), "the first goes ahead");

        record(&db, &instance, "start", "r").unwrap().complete("ok");
        let first = back_off(&db, &instance, "start").expect("the repeat waits");
        record(&db, &instance, "start", "r").unwrap().complete("ok");
        let second = back_off(&db, &instance, "start").expect("and waits longer");
        assert!(second > first, "{second:?} > {first:?}");
        assert!(
            back_off(&db, &dep("app", "other"), "start").is_none(),
            "another instance is not held back"
        );
        assert!(back_off(&db, &instance, "stop").is_none(), "nor is another operation");
    }
}
