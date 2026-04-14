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
