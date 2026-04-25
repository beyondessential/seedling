use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;
use seedling_protocol::names::{ActionName, AppName};

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
        let app: AppName = row.get(0)?;
        let kind_str: String = row.get(1)?;
        let name: Option<String> = row.get(2)?;
        let is_scaled: i64 = row.get(3)?;
        let display_name: String = row.get(4)?;
        Ok((app, kind_str, name, is_scaled, display_name))
    });

    match result {
        Ok((app, kind_str, name, is_scaled, display_name)) => {
            let kind = parse_resource_kind(&kind_str)?;
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
    app: &AppName,
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
                app: app.clone(),
                kind,
                name: name.map(|s| s.to_owned()),
                variant,
                display_name,
            });
        }
    }
    Ok(instances)
}

// r[impl gc.instances]
// r[impl gc.instances.atomic]
pub fn delete_instance(db: &Db, id: InstanceId) -> rusqlite::Result<()> {
    let hex = id.to_hex();
    let tx = db.conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM world_observations WHERE instance_id = ?1",
        params![hex],
    )?;
    tx.execute("DELETE FROM faults WHERE instance_id = ?1", params![hex])?;
    tx.execute("DELETE FROM resource_instances WHERE id = ?1", params![hex])?;
    tx.commit()?;
    Ok(())
}

// r[impl identity.stable]
// r[impl identity.components]
pub fn get_or_create_singleton(
    db: &Db,
    app: &AppName,
    kind: ResourceKind,
    name: Option<&str>,
) -> rusqlite::Result<ResourceInstance> {
    let kind_str = format!("{:?}", kind);

    // Atomic insert-or-select: the partial unique index (app, kind, name)
    // WHERE is_scaled = 0 guarantees at most one singleton row exists.
    let tx = db.conn.unchecked_transaction()?;

    let candidate = match name {
        Some(n) => ResourceInstance::new_singleton(app.clone(), kind, n),
        None => ResourceInstance::new_anonymous(app.clone(), kind),
    };
    tx.execute(
        "INSERT OR IGNORE INTO resource_instances
             (id, app, kind, name, is_scaled, display_name, created_at)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
        params![
            candidate.id.to_hex(),
            candidate.app,
            &kind_str,
            candidate.name,
            candidate.display_name,
            now_ms(),
        ],
    )?;

    let (id_hex, display_name): (String, String) = tx.query_row(
        "SELECT id, display_name
         FROM resource_instances
         WHERE app = ?1 AND kind = ?2 AND name IS ?3 AND is_scaled = 0
         LIMIT 1",
        params![app, &kind_str, name],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    tx.commit()?;

    let id = InstanceId::from_hex(&id_hex).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(0, "id".to_string(), rusqlite::types::Type::Text)
    })?;
    Ok(ResourceInstance {
        id,
        app: app.clone(),
        kind,
        name: name.map(|s| s.to_owned()),
        variant: InstanceVariant::Singleton,
        display_name,
    })
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
// r[impl history.world.ordering]
pub fn query_observations(
    db: &Db,
    instance: &ResourceInstance,
) -> rusqlite::Result<Vec<WorldObservation>> {
    let mut stmt = db.conn.prepare(
        "SELECT id, recorded_at, obs_kind, payload
         FROM world_observations
         WHERE instance_id = ?1
         ORDER BY recorded_at ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![instance.id.to_hex()], |row| {
        Ok(WorldObservation {
            id: row.get(0)?,
            recorded_at: row.get(1)?,
            resource: instance.clone(),
            obs_kind: row.get(2)?,
            payload: serde_json::from_str(&row.get::<_, String>(3)?).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
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
         ORDER BY recorded_at ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![resource_id.to_hex()], |row| {
        Ok(AutonomousOperation {
            id: row.get(0)?,
            recorded_at: row.get(1)?,
            resource_id,
            operation: row.get(2)?,
            provenance: serde_json::from_str(&row.get::<_, String>(3)?).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
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
    app: &AppName,
    action_name: &ActionName,
    entry: &ActionLogEntry,
) -> rusqlite::Result<()> {
    let resources_json = serde_json::to_string(&entry.resources).unwrap_or_default();
    let (barrier_state, barrier_deadline, barrier_satisfied, barrier_started_at) =
        match &entry.barrier {
            Some(b) => (
                Some(format!("{:?}", b.required_state)),
                b.deadline_secs.map(|d| d as i64),
                Some(b.satisfied as i32),
                b.started_at_secs.map(|s| s as i64),
            ),
            None => (None, None, None, None),
        };

    db.conn.execute(
        "INSERT OR REPLACE INTO action_log
             (recorded_at, operation_id, app, action_name, call_index, call_kind,
              resources, barrier_state, barrier_deadline, barrier_satisfied,
              barrier_started_at, extra)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
            entry.extra,
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
                barrier_satisfied, barrier_started_at, extra
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
        let extra: Option<String> = row.get(7)?;

        let call_kind = match call_kind_str.as_str() {
            "Start" => CallKind::Start,
            "Stop" => CallKind::Stop,
            "Query" => CallKind::Query,
            "WarmCerts" => CallKind::WarmCerts,
            "WarmImages" => CallKind::WarmImages,
            "Signal" => CallKind::Signal,
            other => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    format!("unknown call kind: {other}").into(),
                ));
            }
        };

        let resources: Vec<ResourceInstance> =
            serde_json::from_str(&resources_str).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;

        let barrier = match (barrier_state, barrier_satisfied) {
            (Some(state_str), Some(satisfied)) => {
                let required_state = parse_lifecycle_state(&state_str)?;
                Some(BarrierRecord {
                    required_state,
                    deadline_secs: barrier_deadline.map(|d| d as u64),
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
            extra,
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
// r[impl operation.lifecycle.generations]
// r[impl barrier.replay]
pub struct CurrentOperation {
    pub operation_id: OperationId,
    pub app: AppName,
    pub action_name: ActionName,
    pub source_generation: u64,
    pub target_generation: u64,
}

// r[impl operation.params] r[impl barrier.replay]
/// Persist the current lifecycle operation alongside its params, encrypted
/// with the site cipher. Params may carry secret values (install action
/// passwords, action params containing tokens, etc.); they are never stored
/// in the action log, but must survive a restart so the operation can be
/// replayed. Called with an empty map for operations that take no params.
///
/// The params are serialised as JSON and then encrypted as a single blob.
pub fn save_current_operation(
    db: &Db,
    cipher: &crate::runtime::secrets::Cipher,
    op: &CurrentOperation,
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), OperationPersistError> {
    let plaintext = serde_json::to_string(params).map_err(OperationPersistError::Json)?;
    let ciphertext = cipher
        .encrypt(&secrecy::SecretString::new(plaintext.into()))
        .map_err(OperationPersistError::Cipher)?;
    db.conn
        .execute(
            "INSERT OR REPLACE INTO current_operation
                (singleton, operation_id, app, action_name,
                 source_generation, target_generation, params_ciphertext)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                op.operation_id.0,
                op.app,
                op.action_name,
                op.source_generation as i64,
                op.target_generation as i64,
                ciphertext,
            ],
        )
        .map_err(OperationPersistError::Db)?;
    Ok(())
}

/// Decrypt the persisted params for the current in-flight operation, if
/// any. Returns `Ok(None)` when no row is persisted, and an empty map when
/// the row exists but carries no params (i.e. the operation was dispatched
/// with no params).
// r[impl operation.params] r[impl barrier.replay]
pub fn load_current_operation_params(
    db: &Db,
    cipher: &crate::runtime::secrets::Cipher,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>, OperationPersistError> {
    let ciphertext = db
        .conn
        .query_row(
            "SELECT params_ciphertext FROM current_operation WHERE singleton = 1",
            [],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(OperationPersistError::Db(other)),
        })?;
    let Some(ciphertext) = ciphertext else {
        return Ok(None);
    };
    let plaintext = cipher
        .decrypt(&ciphertext)
        .map_err(OperationPersistError::Cipher)?;
    use secrecy::ExposeSecret;
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(plaintext.expose_secret()).map_err(OperationPersistError::Json)?;
    Ok(Some(map))
}

#[derive(Debug)]
pub enum OperationPersistError {
    Db(rusqlite::Error),
    Cipher(crate::runtime::secrets::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for OperationPersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "{e}"),
            Self::Cipher(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OperationPersistError {}

impl From<rusqlite::Error> for OperationPersistError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

// r[impl barrier.replay]
pub fn load_current_operation(db: &Db) -> rusqlite::Result<Option<CurrentOperation>> {
    let result = db.conn.query_row(
        "SELECT operation_id, app, action_name, source_generation, target_generation
         FROM current_operation WHERE singleton = 1",
        [],
        |row| {
            Ok(CurrentOperation {
                operation_id: OperationId(row.get(0)?),
                app: row.get(1)?,
                action_name: row.get(2)?,
                source_generation: row.get::<_, i64>(3)? as u64,
                target_generation: row.get::<_, i64>(4)? as u64,
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

/// Returns the number of `resource_instances` rows owned by `app`. Used by
/// the daemon's startup recovery sweep to detect apps left with stale
/// resources by a previous run.
pub fn app_resource_instance_count(db: &Db, app: &AppName) -> rusqlite::Result<i64> {
    db.conn.query_row(
        "SELECT COUNT(*) FROM resource_instances WHERE app = ?1",
        params![app],
        |row| row.get::<_, i64>(0),
    )
}

/// Mark the currently-persisted operation row as cancel-requested, provided
/// its operation_id matches. Returns `Ok(true)` when a row was updated and
/// `Ok(false)` when no matching row was found (op already completed, or a
/// different op is now active).
// r[impl operation.cancel.persistence]
pub fn set_cancel_requested(db: &Db, operation_id: &OperationId) -> rusqlite::Result<bool> {
    let n = db.conn.execute(
        "UPDATE current_operation \
             SET cancel_requested = 1 \
             WHERE singleton = 1 AND operation_id = ?1",
        params![operation_id.0],
    )?;
    Ok(n > 0)
}

/// Read the persisted cancel flag for the in-flight operation. Returns
/// `false` if no row exists.
// r[impl operation.cancel.persistence]
pub fn load_cancel_requested(db: &Db) -> rusqlite::Result<bool> {
    db.conn
        .query_row(
            "SELECT cancel_requested FROM current_operation WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0).map(|v| v != 0),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            other => Err(other),
        })
}

fn parse_lifecycle_state(s: &str) -> Result<LifecycleState, rusqlite::Error> {
    match s {
        "Pending" => Ok(LifecycleState::Pending),
        "Scheduled" => Ok(LifecycleState::Scheduled),
        "Running" => Ok(LifecycleState::Running),
        "Ready" => Ok(LifecycleState::Ready),
        "Terminating" => Ok(LifecycleState::Terminating),
        "Terminated" => Ok(LifecycleState::Terminated),
        "Unscheduled" => Ok(LifecycleState::Unscheduled),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown lifecycle state: {other}").into(),
        )),
    }
}

fn parse_resource_kind(s: &str) -> Result<ResourceKind, rusqlite::Error> {
    match s {
        "Parameter" => Ok(ResourceKind::Parameter),
        "Service" => Ok(ResourceKind::Service),
        "HttpService" => Ok(ResourceKind::HttpService),
        "Ingress" => Ok(ResourceKind::Ingress),
        "Deployment" => Ok(ResourceKind::Deployment),
        "Job" => Ok(ResourceKind::Job),
        "Volume" => Ok(ResourceKind::Volume),
        "ExternalVolume" => Ok(ResourceKind::ExternalVolume),
        "Action" => Ok(ResourceKind::Action),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown resource kind: {other}").into(),
        )),
    }
}

#[cfg(test)]
mod tests;
