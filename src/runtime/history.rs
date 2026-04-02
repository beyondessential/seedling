use rusqlite::params;
use serde_json;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::runtime::barrier::{ActionLogEntry, BarrierRecord, CallKind, OperationId};
use crate::runtime::db::Db;
use crate::runtime::identity::ResourceInstance;
use crate::runtime::lifecycle::LifecycleState;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ---------------------------------------------------------------------------
// World observations
// ---------------------------------------------------------------------------

// r[impl history.world]
pub struct WorldObservation {
    pub id: i64,
    pub recorded_at: i64,
    pub resource: ResourceInstance,
    pub obs_kind: String,
    pub payload: serde_json::Value,
}

// r[impl history.world.entries]
pub fn insert_observation(
    db: &Db,
    resource: &ResourceInstance,
    obs_kind: &str,
    payload: &serde_json::Value,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO world_observations
             (recorded_at, app, kind, name, ordinal, obs_kind, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            now_ms(),
            resource.app,
            format!("{:?}", resource.kind),
            resource.name,
            resource.ordinal,
            obs_kind,
            serde_json::to_string(payload).unwrap_or_default(),
        ],
    )?;
    Ok(())
}

// r[impl history.world.state-derivation]
pub fn query_observations(
    db: &Db,
    resource: &ResourceInstance,
) -> rusqlite::Result<Vec<WorldObservation>> {
    let mut stmt = db.conn.prepare(
        "SELECT id, recorded_at, obs_kind, payload
         FROM world_observations
         WHERE app = ?1 AND kind = ?2 AND name IS ?3 AND ordinal = ?4
         ORDER BY recorded_at ASC",
    )?;
    let rows = stmt.query_map(
        params![
            resource.app,
            format!("{:?}", resource.kind),
            resource.name,
            resource.ordinal,
        ],
        |row| {
            Ok(WorldObservation {
                id: row.get(0)?,
                recorded_at: row.get(1)?,
                resource: resource.clone(),
                obs_kind: row.get(2)?,
                payload: serde_json::from_str(&row.get::<_, String>(3)?)
                    .unwrap_or(serde_json::Value::Null),
            })
        },
    )?;
    rows.collect()
}

// ---------------------------------------------------------------------------
// Autonomous operations
// ---------------------------------------------------------------------------

// r[impl history.operations]
pub struct AutonomousOperation {
    pub id: i64,
    pub recorded_at: i64,
    pub resource: ResourceInstance,
    pub operation: String,
    pub provenance: serde_json::Value,
    pub outcome: Option<String>,
    pub completed_at: Option<i64>,
}

// r[impl history.operations.provenance]
pub struct Provenance {
    pub observation_ids: Vec<i64>,
    pub rule: String,
}

// r[impl history.operations.entries]
pub fn insert_autonomous_operation(
    db: &Db,
    resource: &ResourceInstance,
    operation: &str,
    provenance: &Provenance,
) -> rusqlite::Result<i64> {
    let prov_json = serde_json::json!({
        "observations": provenance.observation_ids,
        "rule": provenance.rule,
    });
    db.conn.execute(
        "INSERT INTO autonomous_operations
             (recorded_at, app, kind, name, ordinal, operation, provenance)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            now_ms(),
            resource.app,
            format!("{:?}", resource.kind),
            resource.name,
            resource.ordinal,
            operation,
            serde_json::to_string(&prov_json).unwrap_or_default(),
        ],
    )?;
    Ok(db.conn.last_insert_rowid())
}

pub fn complete_autonomous_operation(db: &Db, id: i64, outcome: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE autonomous_operations SET outcome = ?1, completed_at = ?2 WHERE id = ?3",
        params![outcome, now_ms(), id],
    )?;
    Ok(())
}

pub fn query_autonomous_operations(
    db: &Db,
    resource: &ResourceInstance,
) -> rusqlite::Result<Vec<AutonomousOperation>> {
    let mut stmt = db.conn.prepare(
        "SELECT id, recorded_at, operation, provenance, outcome, completed_at
         FROM autonomous_operations
         WHERE app = ?1 AND kind = ?2 AND name IS ?3 AND ordinal = ?4
         ORDER BY recorded_at ASC",
    )?;
    let rows = stmt.query_map(
        params![
            resource.app,
            format!("{:?}", resource.kind),
            resource.name,
            resource.ordinal,
        ],
        |row| {
            Ok(AutonomousOperation {
                id: row.get(0)?,
                recorded_at: row.get(1)?,
                resource: resource.clone(),
                operation: row.get(2)?,
                provenance: serde_json::from_str(&row.get::<_, String>(3)?)
                    .unwrap_or(serde_json::Value::Null),
                outcome: row.get(4)?,
                completed_at: row.get(5)?,
            })
        },
    )?;
    rows.collect()
}

// ---------------------------------------------------------------------------
// Action log
// ---------------------------------------------------------------------------

// r[impl history.action-log]
// r[impl history.action-log.entries]
pub fn insert_action_log_entry(
    db: &Db,
    operation_id: &OperationId,
    app: &str,
    action_name: &str,
    entry: &ActionLogEntry,
) -> rusqlite::Result<()> {
    let resources_json = serde_json::to_string(&entry.resources).unwrap_or_default();
    let (barrier_state, barrier_deadline, barrier_satisfied, barrier_started_at) =
        match &entry.barrier {
            Some(b) => (
                Some(format!("{:?}", b.required_state)),
                Some(b.deadline_secs as i64),
                Some(b.satisfied as i32),
                b.started_at_secs.map(|s| s as i64),
            ),
            None => (None, None, None, None),
        };

    db.conn.execute(
        "INSERT OR REPLACE INTO action_log
             (recorded_at, operation_id, app, action_name, call_index, call_kind,
              resources, barrier_state, barrier_deadline, barrier_satisfied,
              barrier_started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            now_ms(),
            operation_id.0,
            app,
            action_name,
            entry.call_index as i64,
            format!("{:?}", entry.call_kind),
            resources_json,
            barrier_state,
            barrier_deadline,
            barrier_satisfied,
            barrier_started_at,
        ],
    )?;
    Ok(())
}

// r[impl history.action-log.replay]
pub fn load_action_log(
    db: &Db,
    operation_id: &OperationId,
) -> rusqlite::Result<Vec<ActionLogEntry>> {
    let mut stmt = db.conn.prepare(
        "SELECT call_index, call_kind, resources, barrier_state, barrier_deadline,
                barrier_satisfied, barrier_started_at
         FROM action_log
         WHERE operation_id = ?1
         ORDER BY call_index ASC",
    )?;

    let rows = stmt.query_map(params![operation_id.0], |row| {
        let call_index: i64 = row.get(0)?;
        let call_kind_str: String = row.get(1)?;
        let resources_str: String = row.get(2)?;
        let barrier_state: Option<String> = row.get(3)?;
        let barrier_deadline: Option<i64> = row.get(4)?;
        let barrier_satisfied: Option<i32> = row.get(5)?;
        let barrier_started_at: Option<i64> = row.get(6)?;

        let call_kind = match call_kind_str.as_str() {
            "Start" => CallKind::Start,
            "Stop" => CallKind::Stop,
            "Reconcile" => CallKind::Reconcile,
            "Query" => CallKind::Query,
            _ => CallKind::Start,
        };

        let resources: Vec<ResourceInstance> =
            serde_json::from_str(&resources_str).unwrap_or_default();

        let barrier = match (barrier_state, barrier_deadline, barrier_satisfied) {
            (Some(state_str), Some(deadline), Some(satisfied)) => {
                let required_state = parse_lifecycle_state(&state_str);
                Some(BarrierRecord {
                    required_state,
                    deadline_secs: deadline as u64,
                    satisfied: satisfied != 0,
                    started_at_secs: barrier_started_at.map(|s| s as u64),
                })
            }
            _ => None,
        };

        Ok(ActionLogEntry {
            call_index: call_index as usize,
            call_kind,
            resources,
            barrier,
        })
    })?;

    rows.collect()
}

// ---------------------------------------------------------------------------
// Current operation tracking
// ---------------------------------------------------------------------------

/// Persists the identity of the one in-progress lifecycle operation so that
/// a runtime restart can detect it and replay rather than starting fresh.
// r[impl operation.lifecycle.events]
// r[impl barrier.replay]
pub struct CurrentOperation {
    pub operation_id: OperationId,
    pub app: String,
    pub action_name: String,
}

/// Record (or overwrite) the current in-progress operation.
// r[impl operation.lifecycle.events]
// r[impl barrier.replay]
pub fn save_current_operation(db: &Db, op: &CurrentOperation) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR REPLACE INTO current_operation (singleton, operation_id, app, action_name)
         VALUES (1, ?1, ?2, ?3)",
        params![op.operation_id.0, op.app, op.action_name],
    )?;
    Ok(())
}

/// Return the recorded in-progress operation, if any.
// r[impl barrier.replay]
pub fn load_current_operation(db: &Db) -> rusqlite::Result<Option<CurrentOperation>> {
    let mut stmt = db.conn.prepare(
        "SELECT operation_id, app, action_name FROM current_operation WHERE singleton = 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok(CurrentOperation {
            operation_id: OperationId(row.get(0)?),
            app: row.get(1)?,
            action_name: row.get(2)?,
        })
    })?;
    rows.next().transpose()
}

/// Clear the current operation record once the operation has completed.
// r[impl operation.lifecycle.completion]
pub fn clear_current_operation(db: &Db) -> rusqlite::Result<()> {
    db.conn.execute("DELETE FROM current_operation", [])?;
    Ok(())
}

fn parse_lifecycle_state(s: &str) -> LifecycleState {
    match s {
        "Pending" => LifecycleState::Pending,
        "Scheduled" => LifecycleState::Scheduled,
        "Running" => LifecycleState::Running,
        "Ready" => LifecycleState::Ready,
        "Terminating" => LifecycleState::Terminating,
        "Terminated" => LifecycleState::Terminated,
        "Unscheduled" => LifecycleState::Unscheduled,
        _ => LifecycleState::Pending,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // r[verify operation.lifecycle.events]
    // r[verify barrier.replay]
    #[test]
    fn save_and_load_current_operation() {
        let db = Db::open_in_memory().expect("open");
        let op = CurrentOperation {
            operation_id: crate::runtime::barrier::OperationId("test-op-id".into()),
            app: "myapp".into(),
            action_name: "start".into(),
        };
        save_current_operation(&db, &op).expect("save");
        let loaded = load_current_operation(&db)
            .expect("load")
            .expect("should exist");
        assert_eq!(loaded.operation_id.0, "test-op-id");
        assert_eq!(loaded.app, "myapp");
        assert_eq!(loaded.action_name, "start");
    }

    // r[verify operation.lifecycle.events]
    #[test]
    fn load_current_operation_returns_none_when_empty() {
        let db = Db::open_in_memory().expect("open");
        let result = load_current_operation(&db).expect("load");
        assert!(result.is_none());
    }

    // r[verify operation.lifecycle.completion]
    #[test]
    fn clear_current_operation_removes_record() {
        let db = Db::open_in_memory().expect("open");
        let op = CurrentOperation {
            operation_id: crate::runtime::barrier::OperationId("op".into()),
            app: "app".into(),
            action_name: "start".into(),
        };
        save_current_operation(&db, &op).expect("save");
        clear_current_operation(&db).expect("clear");
        let result = load_current_operation(&db).expect("load");
        assert!(result.is_none());
    }

    // r[verify operation.lifecycle.events]
    // r[verify barrier.replay]
    #[test]
    fn save_overwrites_previous_current_operation() {
        let db = Db::open_in_memory().expect("open");
        let op1 = CurrentOperation {
            operation_id: crate::runtime::barrier::OperationId("op1".into()),
            app: "app".into(),
            action_name: "start".into(),
        };
        let op2 = CurrentOperation {
            operation_id: crate::runtime::barrier::OperationId("op2".into()),
            app: "app".into(),
            action_name: "deploy".into(),
        };
        save_current_operation(&db, &op1).expect("save op1");
        save_current_operation(&db, &op2).expect("save op2");
        let loaded = load_current_operation(&db)
            .expect("load")
            .expect("should exist");
        assert_eq!(loaded.operation_id.0, "op2");
        assert_eq!(loaded.action_name, "deploy");
    }

    use crate::defs::resource::ResourceKind;
    use crate::runtime::db::Db;

    fn dep(app: &str, name: &str) -> ResourceInstance {
        ResourceInstance::named(app, ResourceKind::Deployment, name)
    }

    // --- World observations ---

    // r[verify history.world]
    // r[verify history.world.entries]
    // r[verify history.world.state-derivation]
    #[test]
    fn insert_and_retrieve_observation() {
        let db = Db::open_in_memory().expect("open");
        let resource = dep("myapp", "web");
        let payload = serde_json::json!({"status": "created"});

        insert_observation(&db, &resource, "container_created", &payload)
            .expect("insert observation");

        let obs = query_observations(&db, &resource).expect("query observations");
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].obs_kind, "container_created");
        assert_eq!(obs[0].payload, payload);
        assert_eq!(obs[0].resource, resource);
    }

    // r[verify history.world.state-derivation]
    #[test]
    fn observations_ordered_by_recorded_at() {
        let db = Db::open_in_memory().expect("open");
        let resource = dep("myapp", "web");

        insert_observation(&db, &resource, "container_created", &serde_json::json!({}))
            .expect("insert 1");
        insert_observation(&db, &resource, "container_running", &serde_json::json!({}))
            .expect("insert 2");

        let obs = query_observations(&db, &resource).expect("query");
        assert_eq!(obs.len(), 2);
        assert_eq!(obs[0].obs_kind, "container_created");
        assert_eq!(obs[1].obs_kind, "container_running");
    }

    // r[verify history.world.entries]
    #[test]
    fn observations_scoped_to_resource() {
        let db = Db::open_in_memory().expect("open");
        let web = dep("myapp", "web");
        let api = dep("myapp", "api");

        insert_observation(&db, &web, "container_created", &serde_json::json!({}))
            .expect("insert web");
        insert_observation(&db, &api, "container_running", &serde_json::json!({}))
            .expect("insert api");

        let web_obs = query_observations(&db, &web).expect("query web");
        assert_eq!(web_obs.len(), 1);
        assert_eq!(web_obs[0].obs_kind, "container_created");

        let api_obs = query_observations(&db, &api).expect("query api");
        assert_eq!(api_obs.len(), 1);
        assert_eq!(api_obs[0].obs_kind, "container_running");
    }

    // r[verify history.world.state-derivation]
    #[test]
    fn observations_empty_for_unknown_resource() {
        let db = Db::open_in_memory().expect("open");
        let resource = dep("myapp", "nonexistent");
        let obs = query_observations(&db, &resource).expect("query");
        assert!(obs.is_empty());
    }

    // --- Autonomous operations ---

    // r[verify history.operations]
    // r[verify history.operations.entries]
    // r[verify history.operations.provenance]
    #[test]
    fn insert_and_retrieve_autonomous_operation() {
        let db = Db::open_in_memory().expect("open");
        let resource = dep("myapp", "web");
        let prov = Provenance {
            observation_ids: vec![1, 2],
            rule: "container_down".into(),
        };

        let id =
            insert_autonomous_operation(&db, &resource, "start_container", &prov).expect("insert");
        assert!(id > 0);

        let ops = query_autonomous_operations(&db, &resource).expect("query");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].operation, "start_container");
        assert!(ops[0].outcome.is_none());
        assert!(ops[0].completed_at.is_none());
        assert_eq!(ops[0].id, id);
    }

    // r[verify history.operations.entries]
    #[test]
    fn complete_autonomous_operation_sets_outcome() {
        let db = Db::open_in_memory().expect("open");
        let resource = dep("myapp", "web");
        let prov = Provenance {
            observation_ids: vec![],
            rule: "test".into(),
        };

        let id =
            insert_autonomous_operation(&db, &resource, "start_container", &prov).expect("insert");
        complete_autonomous_operation(&db, id, "ok").expect("complete");

        let ops = query_autonomous_operations(&db, &resource).expect("query");
        assert_eq!(ops[0].outcome.as_deref(), Some("ok"));
        assert!(ops[0].completed_at.is_some());
    }

    // r[verify history.operations.entries]
    #[test]
    fn complete_autonomous_operation_with_error() {
        let db = Db::open_in_memory().expect("open");
        let resource = dep("myapp", "web");
        let prov = Provenance {
            observation_ids: vec![42],
            rule: "retry_start".into(),
        };

        let id =
            insert_autonomous_operation(&db, &resource, "start_container", &prov).expect("insert");
        complete_autonomous_operation(&db, id, "err:timeout").expect("complete");

        let ops = query_autonomous_operations(&db, &resource).expect("query");
        assert_eq!(ops[0].outcome.as_deref(), Some("err:timeout"));
    }

    // r[verify history.operations.entries]
    #[test]
    fn autonomous_operations_scoped_to_resource() {
        let db = Db::open_in_memory().expect("open");
        let web = dep("myapp", "web");
        let api = dep("myapp", "api");
        let prov = Provenance {
            observation_ids: vec![],
            rule: "test".into(),
        };

        insert_autonomous_operation(&db, &web, "start_web", &prov).expect("insert web");
        insert_autonomous_operation(&db, &api, "start_api", &prov).expect("insert api");

        let web_ops = query_autonomous_operations(&db, &web).expect("query web");
        assert_eq!(web_ops.len(), 1);
        assert_eq!(web_ops[0].operation, "start_web");
    }

    // --- Action log ---

    // r[verify history.action-log]
    // r[verify history.action-log.entries]
    #[test]
    fn insert_and_load_action_log_entry_without_barrier() {
        let db = Db::open_in_memory().expect("open");
        let op = OperationId("test-op-1".into());
        let resource = dep("myapp", "web");

        let entry = ActionLogEntry {
            call_index: 0,
            call_kind: CallKind::Start,
            resources: vec![resource.clone()],
            barrier: None,
        };

        insert_action_log_entry(&db, &op, "myapp", "start", &entry).expect("insert");

        let loaded = load_action_log(&db, &op).expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].call_index, 0);
        assert_eq!(loaded[0].call_kind, CallKind::Start);
        assert_eq!(loaded[0].resources, vec![resource]);
        assert!(loaded[0].barrier.is_none());
    }

    // r[verify history.action-log.entries]
    #[test]
    fn insert_and_load_action_log_entry_with_barrier() {
        let db = Db::open_in_memory().expect("open");
        let op = OperationId("test-op-2".into());
        let resource = dep("myapp", "web");

        let barrier = BarrierRecord {
            required_state: LifecycleState::Ready,
            deadline_secs: 300,
            satisfied: false,
            started_at_secs: None,
        };
        let entry = ActionLogEntry {
            call_index: 0,
            call_kind: CallKind::Start,
            resources: vec![resource],
            barrier: Some(barrier),
        };

        insert_action_log_entry(&db, &op, "myapp", "start", &entry).expect("insert");

        let loaded = load_action_log(&db, &op).expect("load");
        assert_eq!(loaded.len(), 1);
        let b = loaded[0]
            .barrier
            .as_ref()
            .expect("barrier should be present");
        assert_eq!(b.required_state, LifecycleState::Ready);
        assert_eq!(b.deadline_secs, 300);
        assert!(!b.satisfied);
        assert!(b.started_at_secs.is_none());
    }

    // r[verify history.action-log.replay]
    // r[verify reconciliation.idempotency]
    #[test]
    fn barrier_satisfaction_update_via_replace() {
        let db = Db::open_in_memory().expect("open");
        let op = OperationId("test-op-3".into());
        let resource = dep("myapp", "web");

        // Insert with satisfied = false
        let entry = ActionLogEntry {
            call_index: 0,
            call_kind: CallKind::Start,
            resources: vec![resource],
            barrier: Some(BarrierRecord {
                required_state: LifecycleState::Running,
                deadline_secs: 60,
                satisfied: false,
                started_at_secs: Some(1000),
            }),
        };
        insert_action_log_entry(&db, &op, "myapp", "start", &entry).expect("insert");

        // Replace with satisfied = true
        let updated = ActionLogEntry {
            call_index: 0,
            call_kind: CallKind::Start,
            resources: entry.resources.clone(),
            barrier: Some(BarrierRecord {
                required_state: LifecycleState::Running,
                deadline_secs: 60,
                satisfied: true,
                started_at_secs: Some(1000),
            }),
        };
        insert_action_log_entry(&db, &op, "myapp", "start", &updated).expect("replace");

        let loaded = load_action_log(&db, &op).expect("load");
        // Still only one entry (replaced, not duplicated)
        assert_eq!(loaded.len(), 1);
        let b = loaded[0].barrier.as_ref().expect("barrier");
        assert!(b.satisfied);
    }

    // r[verify history.action-log.replay]
    // r[verify history.action-log.entries]
    #[test]
    fn action_log_multiple_entries_ordered_by_call_index() {
        let db = Db::open_in_memory().expect("open");
        let op = OperationId("test-op-4".into());
        let r1 = dep("myapp", "frontend");
        let r2 = dep("myapp", "backend");

        let e0 = ActionLogEntry {
            call_index: 0,
            call_kind: CallKind::Start,
            resources: vec![r1],
            barrier: None,
        };
        let e1 = ActionLogEntry {
            call_index: 1,
            call_kind: CallKind::Start,
            resources: vec![r2],
            barrier: Some(BarrierRecord {
                required_state: LifecycleState::Ready,
                deadline_secs: 120,
                satisfied: true,
                started_at_secs: Some(2000),
            }),
        };

        insert_action_log_entry(&db, &op, "myapp", "start", &e0).expect("insert 0");
        insert_action_log_entry(&db, &op, "myapp", "start", &e1).expect("insert 1");

        let loaded = load_action_log(&db, &op).expect("load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].call_index, 0);
        assert_eq!(loaded[1].call_index, 1);

        let b = loaded[1].barrier.as_ref().expect("barrier on entry 1");
        assert_eq!(b.required_state, LifecycleState::Ready);
        assert!(b.satisfied);
        assert_eq!(b.started_at_secs, Some(2000));
    }

    // r[verify history.action-log]
    // r[verify operation.lifecycle]
    #[test]
    fn action_log_scoped_to_operation_id() {
        let db = Db::open_in_memory().expect("open");
        let op1 = OperationId("op-A".into());
        let op2 = OperationId("op-B".into());
        let resource = dep("myapp", "web");

        let entry = |idx: usize| ActionLogEntry {
            call_index: idx,
            call_kind: CallKind::Start,
            resources: vec![resource.clone()],
            barrier: None,
        };

        insert_action_log_entry(&db, &op1, "myapp", "start", &entry(0)).expect("insert op1");
        insert_action_log_entry(&db, &op1, "myapp", "start", &entry(1)).expect("insert op1");
        insert_action_log_entry(&db, &op2, "myapp", "start", &entry(0)).expect("insert op2");

        let loaded1 = load_action_log(&db, &op1).expect("load op1");
        assert_eq!(loaded1.len(), 2);

        let loaded2 = load_action_log(&db, &op2).expect("load op2");
        assert_eq!(loaded2.len(), 1);
    }
}
