use rusqlite::params;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::defs::resource::ResourceKind;
use crate::runtime::barrier::{ActionLogEntry, BarrierRecord, CallKind, OperationId};
use crate::runtime::db::Db;
use crate::runtime::identity::{InstanceId, InstanceVariant, ResourceInstance};
use crate::runtime::lifecycle::LifecycleState;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ---------------------------------------------------------------------------
// Instance registry
// ---------------------------------------------------------------------------

// r[impl identity.stable]
// r[impl identity.components]
pub fn insert_instance(db: &Db, instance: &ResourceInstance) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR IGNORE INTO resource_instances
             (id, app, kind, name, is_scaled, display_name, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            instance.id.to_hex(),
            instance.app,
            format!("{:?}", instance.kind),
            instance.name,
            matches!(instance.variant, InstanceVariant::Scaled) as i64,
            instance.display_name,
            now_ms(),
        ],
    )?;
    Ok(())
}

// r[impl identity.stable]
pub fn find_instance(db: &Db, id: InstanceId) -> rusqlite::Result<Option<ResourceInstance>> {
    let mut stmt = db.conn.prepare(
        "SELECT app, kind, name, is_scaled, display_name
         FROM resource_instances
         WHERE id = ?1",
    )?;
    let result = stmt.query_row(params![id.to_hex()], |row| {
        let app: String = row.get(0)?;
        let kind_str: String = row.get(1)?;
        let name: Option<String> = row.get(2)?;
        let is_scaled: i64 = row.get(3)?;
        let display_name: String = row.get(4)?;
        Ok((app, kind_str, name, is_scaled, display_name))
    });

    match result {
        Ok((app, kind_str, name, is_scaled, display_name)) => {
            let kind = parse_resource_kind(&kind_str);
            let variant = if is_scaled != 0 {
                InstanceVariant::Scaled
            } else {
                InstanceVariant::Singleton
            };
            Ok(Some(ResourceInstance {
                id,
                app,
                kind,
                name,
                variant,
                display_name,
            }))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

// r[impl identity.components]
pub fn find_instances_for_group(
    db: &Db,
    app: &str,
    kind: ResourceKind,
    name: Option<&str>,
) -> rusqlite::Result<Vec<ResourceInstance>> {
    let mut stmt = db.conn.prepare(
        "SELECT id, is_scaled, display_name
         FROM resource_instances
         WHERE app = ?1 AND kind = ?2 AND name IS ?3
         ORDER BY created_at ASC",
    )?;
    let kind_str = format!("{:?}", kind);
    let rows = stmt.query_map(params![app, kind_str, name], |row| {
        let id_hex: String = row.get(0)?;
        let is_scaled: i64 = row.get(1)?;
        let display_name: String = row.get(2)?;
        Ok((id_hex, is_scaled, display_name))
    })?;

    let mut instances = Vec::new();
    for row in rows {
        let (id_hex, is_scaled, display_name) = row?;
        if let Some(id) = InstanceId::from_hex(&id_hex) {
            let variant = if is_scaled != 0 {
                InstanceVariant::Scaled
            } else {
                InstanceVariant::Singleton
            };
            instances.push(ResourceInstance {
                id,
                app: app.to_owned(),
                kind,
                name: name.map(|s| s.to_owned()),
                variant,
                display_name,
            });
        }
    }
    Ok(instances)
}

// r[impl identity.stable]
// r[impl identity.components]
pub fn get_or_create_singleton(
    db: &Db,
    app: &str,
    kind: ResourceKind,
    name: Option<&str>,
) -> rusqlite::Result<ResourceInstance> {
    let kind_str = format!("{:?}", kind);
    let mut stmt = db.conn.prepare(
        "SELECT id, display_name
         FROM resource_instances
         WHERE app = ?1 AND kind = ?2 AND name IS ?3 AND is_scaled = 0
         LIMIT 1",
    )?;

    let result = stmt.query_row(params![app, kind_str, name], |row| {
        let id_hex: String = row.get(0)?;
        let display_name: String = row.get(1)?;
        Ok((id_hex, display_name))
    });

    match result {
        Ok((id_hex, display_name)) => {
            let id = InstanceId::from_hex(&id_hex).ok_or_else(|| {
                rusqlite::Error::InvalidColumnType(0, "id".to_string(), rusqlite::types::Type::Text)
            })?;
            Ok(ResourceInstance {
                id,
                app: app.to_owned(),
                kind,
                name: name.map(|s| s.to_owned()),
                variant: InstanceVariant::Singleton,
                display_name,
            })
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            let instance = match name {
                Some(n) => ResourceInstance::new_singleton(app, kind, n),
                None => ResourceInstance::new_anonymous(app, kind),
            };
            insert_instance(db, &instance)?;
            Ok(instance)
        }
        Err(e) => Err(e),
    }
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
    instance: &ResourceInstance,
    obs_kind: &str,
    payload: &serde_json::Value,
) -> rusqlite::Result<()> {
    // Ensure the instance is registered before referencing it.
    insert_instance(db, instance)?;

    db.conn.execute(
        "INSERT INTO world_observations (recorded_at, instance_id, obs_kind, payload)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            now_ms(),
            instance.id.to_hex(),
            obs_kind,
            serde_json::to_string(payload).unwrap_or_default(),
        ],
    )?;
    Ok(())
}

// r[impl history.world.state-derivation]
pub fn query_observations(
    db: &Db,
    instance: &ResourceInstance,
) -> rusqlite::Result<Vec<WorldObservation>> {
    let mut stmt = db.conn.prepare(
        "SELECT id, recorded_at, obs_kind, payload
         FROM world_observations
         WHERE instance_id = ?1
         ORDER BY recorded_at ASC",
    )?;
    let rows = stmt.query_map(params![instance.id.to_hex()], |row| {
        Ok(WorldObservation {
            id: row.get(0)?,
            recorded_at: row.get(1)?,
            resource: instance.clone(),
            obs_kind: row.get(2)?,
            payload: serde_json::from_str(&row.get::<_, String>(3)?)
                .unwrap_or(serde_json::Value::Null),
        })
    })?;
    rows.collect()
}

// ---------------------------------------------------------------------------
// Autonomous operations
// ---------------------------------------------------------------------------

// r[impl history.operations]
pub struct AutonomousOperation {
    pub id: i64,
    pub recorded_at: i64,
    pub resource_id: InstanceId,
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
    resource_id: InstanceId,
    operation: &str,
    provenance: &Provenance,
) -> rusqlite::Result<i64> {
    let prov_json = serde_json::json!({
        "observations": provenance.observation_ids,
        "rule": provenance.rule,
    });
    db.conn.execute(
        "INSERT INTO autonomous_operations
             (recorded_at, instance_id, operation, provenance)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            now_ms(),
            resource_id.to_hex(),
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
    resource_id: InstanceId,
) -> rusqlite::Result<Vec<AutonomousOperation>> {
    let mut stmt = db.conn.prepare(
        "SELECT id, recorded_at, operation, provenance, outcome, completed_at
         FROM autonomous_operations
         WHERE instance_id = ?1
         ORDER BY recorded_at ASC",
    )?;
    let rows = stmt.query_map(params![resource_id.to_hex()], |row| {
        Ok(AutonomousOperation {
            id: row.get(0)?,
            recorded_at: row.get(1)?,
            resource_id,
            operation: row.get(2)?,
            provenance: serde_json::from_str(&row.get::<_, String>(3)?)
                .unwrap_or(serde_json::Value::Null),
            outcome: row.get(4)?,
            completed_at: row.get(5)?,
        })
    })?;
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

// r[impl operation.lifecycle.events]
pub fn save_current_operation(db: &Db, op: &CurrentOperation) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR REPLACE INTO current_operation (singleton, operation_id, app, action_name)
         VALUES (1, ?1, ?2, ?3)",
        params![op.operation_id.0, op.app, op.action_name],
    )?;
    Ok(())
}

// r[impl barrier.replay]
pub fn load_current_operation(db: &Db) -> rusqlite::Result<Option<CurrentOperation>> {
    let result = db.conn.query_row(
        "SELECT operation_id, app, action_name FROM current_operation WHERE singleton = 1",
        [],
        |row| {
            Ok(CurrentOperation {
                operation_id: OperationId(row.get(0)?),
                app: row.get(1)?,
                action_name: row.get(2)?,
            })
        },
    );
    match result {
        Ok(op) => Ok(Some(op)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

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

fn parse_resource_kind(s: &str) -> ResourceKind {
    match s {
        "Parameter" => ResourceKind::Parameter,
        "Service" => ResourceKind::Service,
        "HttpService" => ResourceKind::HttpService,
        "ExternalService" => ResourceKind::ExternalService,
        "Ingress" => ResourceKind::Ingress,
        "Deployment" => ResourceKind::Deployment,
        "Job" => ResourceKind::Job,
        "Volume" => ResourceKind::Volume,
        "ExternalVolume" => ResourceKind::ExternalVolume,
        "Action" => ResourceKind::Action,
        _ => ResourceKind::Deployment,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::resource::ResourceKind;
    use crate::runtime::barrier::{ActionLogEntry, BarrierRecord, CallKind, OperationId};
    use crate::runtime::db::Db;
    use crate::runtime::identity::{InstanceId, InstanceVariant, ResourceInstance};
    use crate::runtime::lifecycle::LifecycleState;

    fn dep(app: &str, name: &str) -> ResourceInstance {
        ResourceInstance::new_singleton(app, ResourceKind::Deployment, name)
    }

    // -----------------------------------------------------------------------
    // Current operation
    // -----------------------------------------------------------------------

    // r[verify history.persistence]
    #[test]
    fn save_and_load_current_operation() {
        let db = Db::open_in_memory().unwrap();
        let op = CurrentOperation {
            operation_id: OperationId("test-op-id".into()),
            app: "myapp".into(),
            action_name: "start".into(),
        };
        save_current_operation(&db, &op).unwrap();
        let loaded = load_current_operation(&db).unwrap().unwrap();
        assert_eq!(loaded.operation_id.0, "test-op-id");
        assert_eq!(loaded.app, "myapp");
        assert_eq!(loaded.action_name, "start");
    }

    #[test]
    fn load_current_operation_returns_none_when_empty() {
        let db = Db::open_in_memory().unwrap();
        assert!(load_current_operation(&db).unwrap().is_none());
    }

    #[test]
    fn clear_current_operation_removes_record() {
        let db = Db::open_in_memory().unwrap();
        let op = CurrentOperation {
            operation_id: OperationId("op-1".into()),
            app: "app".into(),
            action_name: "start".into(),
        };
        save_current_operation(&db, &op).unwrap();
        clear_current_operation(&db).unwrap();
        assert!(load_current_operation(&db).unwrap().is_none());
    }

    #[test]
    fn save_overwrites_previous_current_operation() {
        let db = Db::open_in_memory().unwrap();
        let op1 = CurrentOperation {
            operation_id: OperationId("op-1".into()),
            app: "app".into(),
            action_name: "start".into(),
        };
        let op2 = CurrentOperation {
            operation_id: OperationId("op-2".into()),
            app: "app".into(),
            action_name: "stop".into(),
        };
        save_current_operation(&db, &op1).unwrap();
        save_current_operation(&db, &op2).unwrap();
        let loaded = load_current_operation(&db).unwrap().unwrap();
        assert_eq!(loaded.operation_id.0, "op-2");
        assert_eq!(loaded.action_name, "stop");
    }

    // -----------------------------------------------------------------------
    // Instance registry
    // -----------------------------------------------------------------------

    // r[verify identity.stable]
    // r[verify identity.components]
    #[test]
    fn insert_and_find_instance() {
        let db = Db::open_in_memory().unwrap();
        let instance = dep("myapp", "web");
        insert_instance(&db, &instance).unwrap();

        let found = find_instance(&db, instance.id).unwrap().unwrap();
        assert_eq!(found.id, instance.id);
        assert_eq!(found.app, "myapp");
        assert_eq!(found.name.as_deref(), Some("web"));
        assert_eq!(found.display_name, instance.display_name);
    }

    // r[verify identity.stable]
    #[test]
    fn find_instance_returns_none_for_unknown_id() {
        let db = Db::open_in_memory().unwrap();
        let id = InstanceId::generate();
        assert!(find_instance(&db, id).unwrap().is_none());
    }

    // r[verify identity.stable]
    #[test]
    fn insert_instance_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        let instance = dep("myapp", "web");
        insert_instance(&db, &instance).unwrap();
        insert_instance(&db, &instance).unwrap();
        let found = find_instance(&db, instance.id).unwrap().unwrap();
        assert_eq!(found.id, instance.id);
    }

    // r[verify identity.stable]
    #[test]
    fn get_or_create_singleton_creates_on_first_call() {
        let db = Db::open_in_memory().unwrap();
        let instance =
            get_or_create_singleton(&db, "myapp", ResourceKind::Deployment, Some("web")).unwrap();
        assert_eq!(instance.app, "myapp");
        assert_eq!(instance.name.as_deref(), Some("web"));
        assert_eq!(instance.variant, InstanceVariant::Singleton);
    }

    // r[verify identity.stable]
    #[test]
    fn get_or_create_singleton_returns_same_id_on_second_call() {
        let db = Db::open_in_memory().unwrap();
        let a =
            get_or_create_singleton(&db, "myapp", ResourceKind::Deployment, Some("web")).unwrap();
        let b =
            get_or_create_singleton(&db, "myapp", ResourceKind::Deployment, Some("web")).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.display_name, b.display_name);
    }

    // r[verify identity.components]
    #[test]
    fn find_instances_for_group_returns_all_scaled() {
        let db = Db::open_in_memory().unwrap();
        let a = ResourceInstance::new_scaled("myapp", ResourceKind::Deployment, "web");
        let b = ResourceInstance::new_scaled("myapp", ResourceKind::Deployment, "web");
        insert_instance(&db, &a).unwrap();
        insert_instance(&db, &b).unwrap();

        let found =
            find_instances_for_group(&db, "myapp", ResourceKind::Deployment, Some("web")).unwrap();
        assert_eq!(found.len(), 2);
        let ids: std::collections::HashSet<_> = found.iter().map(|i| i.id).collect();
        assert!(ids.contains(&a.id));
        assert!(ids.contains(&b.id));
    }

    // -----------------------------------------------------------------------
    // World observations
    // -----------------------------------------------------------------------

    // r[verify history.world.entries]
    #[test]
    fn insert_and_retrieve_observation() {
        let db = Db::open_in_memory().unwrap();
        let resource = dep("app", "web");
        insert_observation(&db, &resource, "container_created", &serde_json::json!({})).unwrap();

        let obs = query_observations(&db, &resource).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].obs_kind, "container_created");
        assert_eq!(obs[0].resource.id, resource.id);
    }

    // r[verify history.world.entries]
    #[test]
    fn observations_ordered_by_recorded_at() {
        let db = Db::open_in_memory().unwrap();
        let resource = dep("app", "web");
        insert_observation(&db, &resource, "container_created", &serde_json::json!({})).unwrap();
        insert_observation(&db, &resource, "container_running", &serde_json::json!({})).unwrap();

        let obs = query_observations(&db, &resource).unwrap();
        assert_eq!(obs.len(), 2);
        assert_eq!(obs[0].obs_kind, "container_created");
        assert_eq!(obs[1].obs_kind, "container_running");
    }

    // r[verify history.world.entries]
    #[test]
    fn observations_scoped_to_instance_id() {
        let db = Db::open_in_memory().unwrap();
        let web = dep("app", "web");
        let api = dep("app", "api");
        insert_observation(&db, &web, "container_created", &serde_json::json!({})).unwrap();
        insert_observation(&db, &api, "container_running", &serde_json::json!({})).unwrap();

        let web_obs = query_observations(&db, &web).unwrap();
        assert_eq!(web_obs.len(), 1);
        assert_eq!(web_obs[0].obs_kind, "container_created");

        let api_obs = query_observations(&db, &api).unwrap();
        assert_eq!(api_obs.len(), 1);
        assert_eq!(api_obs[0].obs_kind, "container_running");
    }

    // r[verify history.world.entries]
    #[test]
    fn observations_empty_for_unknown_instance() {
        let db = Db::open_in_memory().unwrap();
        let resource = dep("app", "web");
        let obs = query_observations(&db, &resource).unwrap();
        assert!(obs.is_empty());
    }

    // -----------------------------------------------------------------------
    // Autonomous operations
    // -----------------------------------------------------------------------

    // r[verify history.operations.entries]
    #[test]
    fn insert_and_retrieve_autonomous_operation() {
        let db = Db::open_in_memory().unwrap();
        let resource = dep("app", "web");
        let prov = Provenance {
            observation_ids: vec![1, 2],
            rule: "health-check-failed".into(),
        };
        let id = insert_autonomous_operation(&db, resource.id, "restart", &prov).unwrap();
        assert!(id > 0);

        let ops = query_autonomous_operations(&db, resource.id).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].resource_id, resource.id);
        assert_eq!(ops[0].operation, "restart");
        assert!(ops[0].outcome.is_none());
    }

    // r[verify history.operations.entries]
    #[test]
    fn complete_autonomous_operation_sets_outcome() {
        let db = Db::open_in_memory().unwrap();
        let resource = dep("app", "web");
        let prov = Provenance {
            observation_ids: vec![],
            rule: "test".into(),
        };
        let id = insert_autonomous_operation(&db, resource.id, "restart", &prov).unwrap();
        complete_autonomous_operation(&db, id, "success").unwrap();

        let ops = query_autonomous_operations(&db, resource.id).unwrap();
        assert_eq!(ops[0].outcome.as_deref(), Some("success"));
        assert!(ops[0].completed_at.is_some());
    }

    // r[verify history.operations.entries]
    #[test]
    fn complete_autonomous_operation_with_error() {
        let db = Db::open_in_memory().unwrap();
        let resource = dep("app", "web");
        let prov = Provenance {
            observation_ids: vec![],
            rule: "test".into(),
        };
        let id = insert_autonomous_operation(&db, resource.id, "restart", &prov).unwrap();
        complete_autonomous_operation(&db, id, "error: container exited 1").unwrap();

        let ops = query_autonomous_operations(&db, resource.id).unwrap();
        assert_eq!(ops[0].outcome.as_deref(), Some("error: container exited 1"));
    }

    // r[verify history.operations.entries]
    #[test]
    fn autonomous_operations_scoped_to_instance_id() {
        let db = Db::open_in_memory().unwrap();
        let web = dep("app", "web");
        let api = dep("app", "api");
        let prov = Provenance {
            observation_ids: vec![],
            rule: "test".into(),
        };
        insert_autonomous_operation(&db, web.id, "restart", &prov).unwrap();
        insert_autonomous_operation(&db, api.id, "rebuild", &prov).unwrap();

        let web_ops = query_autonomous_operations(&db, web.id).unwrap();
        assert_eq!(web_ops.len(), 1);
        assert_eq!(web_ops[0].operation, "restart");

        let api_ops = query_autonomous_operations(&db, api.id).unwrap();
        assert_eq!(api_ops.len(), 1);
        assert_eq!(api_ops[0].operation, "rebuild");
    }

    // -----------------------------------------------------------------------
    // Action log
    // -----------------------------------------------------------------------

    fn make_entry(
        call_index: usize,
        call_kind: CallKind,
        barrier: Option<BarrierRecord>,
    ) -> ActionLogEntry {
        ActionLogEntry {
            call_index,
            call_kind,
            resources: vec![dep("app", "web")],
            barrier,
        }
    }

    // r[verify history.action-log.entries]
    #[test]
    fn insert_and_load_action_log_entry_without_barrier() {
        let db = Db::open_in_memory().unwrap();
        let op = OperationId("op-1".into());
        let entry = make_entry(0, CallKind::Start, None);
        insert_action_log_entry(&db, &op, "myapp", "start", &entry).unwrap();

        let loaded = load_action_log(&db, &op).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].call_index, 0);
        assert!(matches!(loaded[0].call_kind, CallKind::Start));
        assert!(loaded[0].barrier.is_none());
    }

    // r[verify history.action-log.entries]
    #[test]
    fn insert_and_load_action_log_entry_with_barrier() {
        let db = Db::open_in_memory().unwrap();
        let op = OperationId("op-1".into());
        let barrier = BarrierRecord {
            required_state: LifecycleState::Ready,
            deadline_secs: 30,
            satisfied: false,
            started_at_secs: Some(1000),
        };
        let entry = make_entry(0, CallKind::Start, Some(barrier));
        insert_action_log_entry(&db, &op, "myapp", "start", &entry).unwrap();

        let loaded = load_action_log(&db, &op).unwrap();
        let b = loaded[0].barrier.as_ref().unwrap();
        assert_eq!(b.required_state, LifecycleState::Ready);
        assert_eq!(b.deadline_secs, 30);
        assert!(!b.satisfied);
        assert_eq!(b.started_at_secs, Some(1000));
    }

    // r[verify reconciliation.idempotency]
    #[test]
    fn barrier_satisfaction_update_via_replace() {
        let db = Db::open_in_memory().unwrap();
        let op = OperationId("op-1".into());
        let barrier = BarrierRecord {
            required_state: LifecycleState::Ready,
            deadline_secs: 30,
            satisfied: false,
            started_at_secs: Some(1000),
        };
        let entry = make_entry(0, CallKind::Start, Some(barrier));
        insert_action_log_entry(&db, &op, "myapp", "start", &entry).unwrap();

        let satisfied_entry = ActionLogEntry {
            call_index: 0,
            call_kind: CallKind::Start,
            resources: vec![dep("app", "web")],
            barrier: Some(BarrierRecord {
                required_state: LifecycleState::Ready,
                deadline_secs: 30,
                satisfied: true,
                started_at_secs: Some(1000),
            }),
        };
        insert_action_log_entry(&db, &op, "myapp", "start", &satisfied_entry).unwrap();

        let loaded = load_action_log(&db, &op).unwrap();
        assert_eq!(loaded.len(), 1, "INSERT OR REPLACE should not duplicate");
        assert!(loaded[0].barrier.as_ref().unwrap().satisfied);
    }

    // r[verify history.action-log.entries]
    #[test]
    fn action_log_multiple_entries_ordered_by_call_index() {
        let db = Db::open_in_memory().unwrap();
        let op = OperationId("op-1".into());
        for i in [2usize, 0, 1] {
            insert_action_log_entry(
                &db,
                &op,
                "myapp",
                "start",
                &make_entry(i, CallKind::Start, None),
            )
            .unwrap();
        }
        let loaded = load_action_log(&db, &op).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].call_index, 0);
        assert_eq!(loaded[1].call_index, 1);
        assert_eq!(loaded[2].call_index, 2);
    }

    // r[verify history.action-log.entries]
    #[test]
    fn action_log_scoped_to_operation_id() {
        let db = Db::open_in_memory().unwrap();
        let op1 = OperationId("op-1".into());
        let op2 = OperationId("op-2".into());
        insert_action_log_entry(
            &db,
            &op1,
            "myapp",
            "start",
            &make_entry(0, CallKind::Start, None),
        )
        .unwrap();
        insert_action_log_entry(
            &db,
            &op2,
            "myapp",
            "start",
            &make_entry(0, CallKind::Stop, None),
        )
        .unwrap();

        let loaded1 = load_action_log(&db, &op1).unwrap();
        assert_eq!(loaded1.len(), 1);
        assert!(matches!(loaded1[0].call_kind, CallKind::Start));

        let loaded2 = load_action_log(&db, &op2).unwrap();
        assert_eq!(loaded2.len(), 1);
        assert!(matches!(loaded2[0].call_kind, CallKind::Stop));
    }
}
