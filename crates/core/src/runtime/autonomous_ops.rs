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

// r[impl autonomous.provenance-required]
/// Record the start of an autonomous operation. Must be called before the
/// underlying action (start/stop/reconfigure/etc.) is initiated. Returns a
/// handle whose [`complete`](AutonomousOpHandle::complete) method records
/// the outcome.
///
/// `operation` is a short identifier of the action (e.g. `"restart"`,
/// `"scale_down_excess"`, `"job_terminal_stop"`). `rule` is a free-form
/// description of why the action was taken — typically a reference to the
/// observation(s) that triggered it together with the policy or invariant
/// that mandates the action.
pub fn record(
    db: &DbHandle,
    instance: &ResourceInstance,
    operation: &str,
    rule: &str,
) -> AutonomousOpHandle {
    let inst_id = instance.id;
    let inst_display = instance.display_name.clone();
    let op = operation.to_owned();
    let prov = Provenance {
        observation_ids: Vec::new(),
        rule: rule.to_owned(),
    };
    let id = db.call(move |db| {
        history::insert_autonomous_operation(db, inst_id, &op, &prov)
            .map_err(|e| {
                tracing::warn!(
                    instance = %inst_display,
                    error = %e,
                    "failed to record autonomous op",
                );
                e
            })
            .ok()
    });
    AutonomousOpHandle {
        db: db.clone(),
        id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::resource::ResourceKind;
    use crate::runtime::db::Db;
    use crate::runtime::history::query_autonomous_operations;
    use crate::runtime::identity::ResourceInstance;

    fn dep(app: &str, name: &str) -> ResourceInstance {
        ResourceInstance::new_singleton(app, ResourceKind::Deployment, name)
    }

    // r[verify autonomous.provenance-required]
    #[test]
    fn record_inserts_entry_before_returning_handle() {
        let db = DbHandle::from_db(Db::open_in_memory().unwrap());
        let instance = dep("app", "web");

        let handle = record(
            &db,
            &instance,
            "restart",
            "Container exited; OnExit policy is Restart",
        );

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

    // r[verify autonomous.provenance-required]
    #[test]
    fn complete_attaches_outcome() {
        let db = DbHandle::from_db(Db::open_in_memory().unwrap());
        let instance = dep("app", "web");

        let handle = record(&db, &instance, "scale_down_stop", "scale=2 observed=3");
        handle.complete("ok");

        let entries = db
            .call(move |db| query_autonomous_operations(db, instance.id))
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].outcome.as_deref(), Some("ok"));
        assert!(entries[0].completed_at.is_some());
    }
}
