use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use parking_lot::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, error};

use crate::runtime::db::Db;

pub struct GcConfig {
    pub interval: Duration,
    pub retain_action_log: Duration,
    pub retain_cleared_faults: Duration,
    pub retain_completed_operations: Duration,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60 * 60),
            retain_action_log: Duration::from_secs(24 * 60 * 60),
            retain_cleared_faults: Duration::from_secs(7 * 24 * 60 * 60),
            retain_completed_operations: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }
}

// r[impl gc.background]
pub fn spawn_gc_task(db: Arc<Mutex<Db>>, config: GcConfig) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(config.interval);
        loop {
            ticker.tick().await;
            let db = db.lock();
            run_gc_cycle(&db, &config);
        }
    })
}

fn run_gc_cycle(db: &Db, config: &GcConfig) {
    match gc_action_log(db, config.retain_action_log) {
        Ok(n) if n > 0 => debug!(rows = n, "gc: pruned action_log"),
        Err(e) => error!(error = %e, "gc: action_log cleanup failed"),
        _ => {}
    }
    match gc_cleared_faults(db, config.retain_cleared_faults) {
        Ok(n) if n > 0 => debug!(rows = n, "gc: pruned cleared faults"),
        Err(e) => error!(error = %e, "gc: cleared faults cleanup failed"),
        _ => {}
    }
    match gc_orphaned_observations(db) {
        Ok(n) if n > 0 => debug!(rows = n, "gc: pruned orphaned observations"),
        Err(e) => error!(error = %e, "gc: orphaned observations cleanup failed"),
        _ => {}
    }
    match gc_completed_operations(db, config.retain_completed_operations) {
        Ok(n) if n > 0 => debug!(rows = n, "gc: pruned completed operations"),
        Err(e) => error!(error = %e, "gc: completed operations cleanup failed"),
        _ => {}
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// r[impl gc.action-log]
fn gc_action_log(db: &Db, retain: Duration) -> rusqlite::Result<usize> {
    let cutoff = now_ms() - retain.as_millis() as i64;
    db.conn.execute(
        "DELETE FROM action_log
         WHERE operation_id NOT IN (SELECT operation_id FROM current_operation)
           AND recorded_at < ?1",
        rusqlite::params![cutoff],
    )
}

// r[impl gc.faults]
fn gc_cleared_faults(db: &Db, retain: Duration) -> rusqlite::Result<usize> {
    let cutoff = Timestamp::now()
        .checked_sub(jiff::SignedDuration::from_secs(retain.as_secs() as i64))
        .expect("timestamp subtraction should not overflow");
    let cutoff_str = cutoff.to_string();
    db.conn.execute(
        "DELETE FROM faults WHERE cleared_at IS NOT NULL AND cleared_at < ?1",
        rusqlite::params![cutoff_str],
    )
}

// r[impl gc.observations]
fn gc_orphaned_observations(db: &Db) -> rusqlite::Result<usize> {
    db.conn.execute(
        "DELETE FROM world_observations WHERE instance_id NOT IN (SELECT id FROM resource_instances)",
        [],
    )
}

// r[impl gc.autonomous-operations]
fn gc_completed_operations(db: &Db, retain: Duration) -> rusqlite::Result<usize> {
    let cutoff = now_ms() - retain.as_millis() as i64;
    db.conn.execute(
        "DELETE FROM autonomous_operations WHERE completed_at IS NOT NULL AND completed_at < ?1",
        rusqlite::params![cutoff],
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Once;

    use rusqlite::params;

    use super::*;
    use crate::defs::resource::ResourceKind;
    use crate::runtime::barrier::{ActionLogEntry, CallKind, OperationId};
    use crate::runtime::history::{self, CurrentOperation, Provenance};
    use crate::runtime::identity::ResourceInstance;

    fn dep(app: &str, name: &str) -> ResourceInstance {
        ResourceInstance::new_singleton(app, ResourceKind::Deployment, name)
    }

    fn make_entry(call_index: usize) -> ActionLogEntry {
        ActionLogEntry {
            call_index,
            call_kind: CallKind::Start,
            resources: vec![dep("app", "web")],
            barrier: None,
        }
    }

    fn ensure_faults_init() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::runtime::faults::init(tokio::sync::broadcast::channel(16).0);
            }));
        });
    }

    // r[verify gc.action-log]
    #[test]
    fn gc_action_log_removes_old_entries() {
        let db = Db::open_in_memory().unwrap();

        let current_op = OperationId("current-op".into());
        let old_op = OperationId("old-op".into());

        for i in 0..3 {
            history::insert_action_log_entry(&db, &current_op, "app", "start", &make_entry(i))
                .unwrap();
        }
        for i in 0..2 {
            history::insert_action_log_entry(&db, &old_op, "app", "start", &make_entry(i)).unwrap();
        }

        history::save_current_operation(
            &db,
            &CurrentOperation {
                operation_id: current_op.clone(),
                app: "app".into(),
                action_name: "start".into(),
            },
        )
        .unwrap();

        let old_ts = now_ms() - 48 * 60 * 60 * 1000;
        db.conn
            .execute(
                "UPDATE action_log SET recorded_at = ?1 WHERE operation_id = ?2",
                params![old_ts, old_op.0],
            )
            .unwrap();

        let deleted = gc_action_log(&db, Duration::from_secs(3600)).unwrap();
        assert_eq!(deleted, 2);

        let remaining: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM action_log WHERE operation_id = ?1",
                params![current_op.0],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 3);

        let old_remaining: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM action_log WHERE operation_id = ?1",
                params![old_op.0],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_remaining, 0);
    }

    #[test]
    fn gc_action_log_preserves_recent() {
        let db = Db::open_in_memory().unwrap();

        let op = OperationId("recent-op".into());
        for i in 0..3 {
            history::insert_action_log_entry(&db, &op, "app", "start", &make_entry(i)).unwrap();
        }

        let deleted = gc_action_log(&db, Duration::from_secs(24 * 60 * 60)).unwrap();
        assert_eq!(deleted, 0);

        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM action_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 3);
    }

    // r[verify gc.faults]
    #[test]
    fn gc_cleared_faults_removes_old() {
        ensure_faults_init();
        let db = Db::open_in_memory().unwrap();

        let id1 = crate::runtime::faults::file_fault(
            &db,
            "app",
            Some("Deployment"),
            Some("web"),
            None,
            "health",
            "unhealthy",
        )
        .unwrap();
        let id2 = crate::runtime::faults::file_fault(
            &db,
            "app",
            Some("Deployment"),
            Some("api"),
            None,
            "health",
            "unhealthy",
        )
        .unwrap();

        crate::runtime::faults::clear_fault(&db, &id1, "app").unwrap();

        let old_ts = Timestamp::now()
            .checked_sub(jiff::SignedDuration::from_hours(30 * 24))
            .unwrap();
        db.conn
            .execute(
                "UPDATE faults SET cleared_at = ?1 WHERE id = ?2",
                params![old_ts.to_string(), id1],
            )
            .unwrap();

        let deleted = gc_cleared_faults(&db, Duration::from_secs(7 * 24 * 60 * 60)).unwrap();
        assert_eq!(deleted, 1);

        let active = crate::runtime::faults::list_active_faults(&db, None).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id2);

        let total: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM faults", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 1);
    }

    // r[verify gc.observations]
    #[test]
    fn gc_orphaned_observations_removes_unreferenced() {
        let db = Db::open_in_memory().unwrap();
        let resource = dep("app", "web");

        history::insert_observation(
            &db,
            &resource,
            "container_running",
            &serde_json::json!({"status": "running"}),
        )
        .unwrap();

        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM world_observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        db.conn
            .execute(
                "DELETE FROM resource_instances WHERE id = ?1",
                params![resource.id.to_hex()],
            )
            .unwrap();

        let deleted = gc_orphaned_observations(&db).unwrap();
        assert_eq!(deleted, 1);

        let remaining: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM world_observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    // r[verify gc.autonomous-operations]
    #[test]
    fn gc_completed_operations_removes_old() {
        let db = Db::open_in_memory().unwrap();
        let resource = dep("app", "web");
        let prov = Provenance {
            observation_ids: vec![],
            rule: "test".into(),
        };

        let op_id =
            history::insert_autonomous_operation(&db, resource.id, "restart", &prov).unwrap();
        history::complete_autonomous_operation(&db, op_id, "success").unwrap();

        let old_ts = now_ms() - 30 * 24 * 60 * 60 * 1000;
        db.conn
            .execute(
                "UPDATE autonomous_operations SET completed_at = ?1 WHERE id = ?2",
                params![old_ts, op_id],
            )
            .unwrap();

        let deleted = gc_completed_operations(&db, Duration::from_secs(7 * 24 * 60 * 60)).unwrap();
        assert_eq!(deleted, 1);

        let remaining: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM autonomous_operations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
