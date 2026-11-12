use std::collections::HashMap;
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

// spec: IDN#instance-identity
pub fn insert_instance(db: &Db, instance: &ResourceInstance) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR IGNORE INTO resource_instances
             (id, app, kind, name, is_scaled, display_name, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            instance.id.to_hex(),
            instance.app,
            resource_kind_str(instance.kind).to_owned(),
            instance.name,
            matches!(instance.variant, InstanceVariant::Scaled) as i64,
            instance.display_name,
            now_ms(),
        ],
    )?;
    Ok(())
}

// spec: IDN#instance-identity
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

/// Every `display_name` this app has ever actuated.
///
/// The registry knows an app's units exactly; `seedling-{app}-` does not.
/// `display_name` is `{app}-{name}[-{suffix}]` or `{app}-{kind_slug}[-{name}]`,
/// and both app names and resource names may contain hyphens, so the encoding
/// is not prefix-free: `seedling-app-` matches every unit of an app called
/// `app-db`. These rows survive until uninstall *completes*, so they are
/// available for the whole window in which the match runs.
// spec: IDN#instance-identity
pub fn display_names_for_app(db: &Db, app: &AppName) -> rusqlite::Result<Vec<String>> {
    let mut stmt = db
        .conn
        .prepare("SELECT DISTINCT display_name FROM resource_instances WHERE app = ?1")?;
    let rows = stmt.query_map(params![app], |row| row.get::<_, String>(0))?;
    rows.collect()
}

// spec: IDN#instance-identity
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
    let kind_str = resource_kind_str(kind).to_owned();
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

// spec: GC#unscheduled-instances
// spec: GC#instance-retirement
pub fn delete_instance(db: &Db, id: InstanceId) -> rusqlite::Result<()> {
    let hex = id.to_hex();
    db.atomically(|db| {
        db.conn.execute(
            "DELETE FROM world_observations WHERE instance_id = ?1",
            params![hex],
        )?;
        db.conn
            .execute("DELETE FROM faults WHERE instance_id = ?1", params![hex])?;
        db.conn.execute(
            "DELETE FROM autonomous_operations WHERE instance_id = ?1",
            params![hex],
        )?;
        db.conn
            .execute("DELETE FROM resource_instances WHERE id = ?1", params![hex])?;
        Ok(())
    })
}

/// Drop every registry row for `app`, once nothing of it is desired: its
/// uninstall has finished, or it is being deregistered. Observations left
/// behind are pruned as orphans.
pub fn delete_instances_for_app(db: &Db, app: &AppName) -> rusqlite::Result<usize> {
    db.conn.execute(
        "DELETE FROM resource_instances WHERE app = ?1",
        params![app],
    )
}

// spec: IDN#instance-identity
pub fn get_or_create_singleton(
    db: &Db,
    app: &AppName,
    kind: ResourceKind,
    name: Option<&str>,
) -> rusqlite::Result<ResourceInstance> {
    let kind_str = resource_kind_str(kind).to_owned();

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

// spec: HIST#world-observation-history
pub struct WorldObservation {
    pub id: i64,
    pub recorded_at: i64,
    pub resource: ResourceInstance,
    pub obs_kind: String,
    pub payload: serde_json::Value,
}

// spec: HIST#observations
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

// spec: LCY#derivation
// spec: HIST#observations
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

// spec: HIST#observations
/// The observations of every instance given, each instance's in the same
/// order [`query_observations`] returns them.
///
/// One statement for the group rather than one per instance. The callers
/// that resolve a scaled group want every replica's history at once, and
/// asking per replica put a prepare and a round trip between each.
pub fn query_observations_for(
    db: &Db,
    instances: &[ResourceInstance],
) -> rusqlite::Result<HashMap<InstanceId, Vec<WorldObservation>>> {
    let mut by_instance: HashMap<InstanceId, Vec<WorldObservation>> =
        instances.iter().map(|inst| (inst.id, Vec::new())).collect();
    if instances.is_empty() {
        return Ok(by_instance);
    }
    let by_hex: HashMap<String, &ResourceInstance> =
        instances.iter().map(|i| (i.id.to_hex(), i)).collect();

    let placeholders = vec!["?"; instances.len()].join(", ");
    let mut stmt = db.conn.prepare(&format!(
        "SELECT instance_id, id, recorded_at, obs_kind, payload
         FROM world_observations
         WHERE instance_id IN ({placeholders})
         ORDER BY recorded_at ASC, id ASC"
    ))?;
    let params: Vec<String> = instances.iter().map(|i| i.id.to_hex()).collect();
    let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
        let hex: String = row.get(0)?;
        Ok((
            hex,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    for row in rows {
        let (hex, id, recorded_at, obs_kind, payload) = row?;
        // An id the query did not ask for cannot be matched to an instance,
        // so it is not silently filed under one.
        let Some(instance) = by_hex.get(&hex) else {
            continue;
        };
        let payload = serde_json::from_str(&payload).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
        })?;
        by_instance
            .entry(instance.id)
            .or_default()
            .push(WorldObservation {
                id,
                recorded_at,
                resource: (*instance).clone(),
                obs_kind,
                payload,
            });
    }
    Ok(by_instance)
}

// ---------------------------------------------------------------------------
// Autonomous operations
// ---------------------------------------------------------------------------

// spec: HIST#autonomous-operations-log
pub struct AutonomousOperation {
    pub id: i64,
    pub recorded_at: i64,
    pub resource_id: InstanceId,
    pub operation: String,
    pub provenance: serde_json::Value,
    pub outcome: Option<String>,
    pub completed_at: Option<i64>,
}

// spec: HIST#operation-provenance
pub struct Provenance {
    pub observation_ids: Vec<i64>,
    pub rule: String,
}

// spec: HIST#autonomous-operations
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

/// The outcome recorded for an operation that was in progress when the
/// runtime stopped.
pub const INTERRUPTED_OUTCOME: &str = "interrupted";

/// Complete every autonomous operation still in progress as interrupted,
/// returning how many there were.
///
/// Only an [`AutonomousOpHandle`](crate::runtime::autonomous_ops::AutonomousOpHandle)
/// completes an operation, and none outlives the process, so this is called
/// at startup before the reconciler records anything: an incomplete record
/// then can only be one whose handle died with the previous run.
// spec: HIST#autonomous-operations
pub fn interrupt_incomplete_autonomous_operations(db: &Db) -> rusqlite::Result<usize> {
    db.conn.execute(
        "UPDATE autonomous_operations SET outcome = ?1, completed_at = ?2
         WHERE completed_at IS NULL",
        params![INTERRUPTED_OUTCOME, now_ms()],
    )
}

/// The ids of the most recent observations recorded for `instance_id`, oldest
/// first: what the runtime had observed of it when it decided to act.
// spec: HIST#operation-provenance
pub fn recent_observation_ids(
    db: &Db,
    instance_id: InstanceId,
    limit: usize,
) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = db.conn.prepare(
        "SELECT id FROM (
             SELECT id FROM world_observations WHERE instance_id = ?1
             ORDER BY id DESC LIMIT ?2
         ) ORDER BY id ASC",
    )?;
    stmt.query_map(params![instance_id.to_hex(), limit as i64], |r| r.get(0))?
        .collect()
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

// spec: HIST#action-execution-log
// spec: HIST#action-calls
pub fn insert_action_log_entry(
    db: &Db,
    operation_id: &OperationId,
    app: &AppName,
    action_name: &ActionName,
    entry: &ActionLogEntry,
) -> rusqlite::Result<()> {
    let resources_json = serde_json::to_string(&entry.resources).unwrap_or_default();
    // spec: SUSP#barrier-anchoring
    // Every barrier of the call goes in `barriers`. The four single-record
    // columns are still written, from the call's first barrier, so a row
    // this version writes is still legible to the reader that only knows
    // them.
    let barriers_json = if entry.barriers.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&entry.barriers).unwrap_or_default())
    };
    let (barrier_state, barrier_deadline, barrier_satisfied, barrier_started_at) =
        match entry.barriers.first() {
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
              barrier_started_at, extra, barriers, begun)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
            barriers_json,
            entry.begun,
        ],
    )?;
    Ok(())
}

// spec: HIST#action-calls
pub fn load_action_log(
    db: &Db,
    operation_id: &OperationId,
) -> rusqlite::Result<Vec<ActionLogEntry>> {
    let mut stmt = db.conn.prepare(
        "SELECT call_index, call_kind, resources, barrier_state, barrier_deadline,
                barrier_satisfied, barrier_started_at, extra, barriers, begun
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
        let barriers_json: Option<String> = row.get(8)?;
        let begun: bool = row.get(9)?;

        let call_kind = match call_kind_str.as_str() {
            "Start" => CallKind::Start,
            "Stop" => CallKind::Stop,
            "Query" => CallKind::Query,
            "WarmCerts" => CallKind::WarmCerts,
            "WarmImages" => CallKind::WarmImages,
            "Signal" => CallKind::Signal,
            "Write" => CallKind::Write,
            "Exec" => CallKind::Exec,
            "Restart" => CallKind::Restart,
            "SubAction" => CallKind::SubAction,
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

        // spec: SUSP#barrier-anchoring
        // A row written before the call could hold more than one barrier
        // has nothing in `barriers`, and its one record is read from the
        // columns that do carry it — so an operation suspended across the
        // upgrade resumes on the wait it was already timing rather than
        // starting a fresh one.
        let barriers = match barriers_json {
            Some(json) => serde_json::from_str(&json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
            None => match (barrier_state, barrier_satisfied) {
                (Some(state_str), Some(satisfied)) => vec![BarrierRecord {
                    seq: 0,
                    required_state: parse_lifecycle_state(&state_str)?,
                    deadline_secs: barrier_deadline.map(|d| d as u64),
                    satisfied: satisfied != 0,
                    started_at_secs: barrier_started_at.map(|s| s as u64),
                }],
                _ => Vec::new(),
            },
        };

        Ok(ActionLogEntry {
            call_index: call_index as usize,
            call_kind,
            resources,
            barriers,
            extra,
            begun,
        })
    })?;

    rows.collect()
}

// ---------------------------------------------------------------------------
// Current operation tracking
// ---------------------------------------------------------------------------

/// Persists the identity of the one in-progress lifecycle operation so that
/// a runtime restart can detect it and replay rather than starting fresh.
// spec: OPS#initiating-events
// spec: SUSP#replay
pub struct CurrentOperation {
    pub operation_id: OperationId,
    pub app: AppName,
    pub action_name: ActionName,
    pub source_generation: u64,
    pub target_generation: u64,
}

// spec: OPS#persistence, SUSP#replay
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
            // spec: HIST#durability
            // Not INSERT OR REPLACE. That is delete-then-insert, so every
            // column this statement does not name reverts to its default —
            // including `cancel_requested`, which a *different* writer owns.
            // The replay path re-saves this row immediately after the daemon
            // has read the cancel flag, so a cancellation was silently
            // dropped while the operation it cancelled was still in flight.
            // The column was added by a later migration than the statement,
            // which is exactly how this kind of bug arrives.
            "INSERT INTO current_operation
                (singleton, operation_id, app, action_name,
                 source_generation, target_generation, params_ciphertext)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(singleton) DO UPDATE SET
                operation_id = excluded.operation_id,
                app = excluded.app,
                action_name = excluded.action_name,
                source_generation = excluded.source_generation,
                target_generation = excluded.target_generation,
                params_ciphertext = excluded.params_ciphertext",
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
// spec: OPS#persistence, SUSP#replay
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

// spec: SUSP#replay
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

// ---------------------------------------------------------------------------
// Queued operations
// ---------------------------------------------------------------------------

/// Persist a queued request, with its parameters protected under the secret
/// key as a persisted operation's are.
// spec: OPS#concurrency
pub fn save_queued_operation(
    db: &Db,
    cipher: &crate::runtime::secrets::Cipher,
    op: &crate::runtime::scheduler::QueuedOperation,
) -> Result<(), OperationPersistError> {
    let plaintext = serde_json::to_string(&op.params).map_err(OperationPersistError::Json)?;
    let ciphertext = cipher
        .encrypt(&secrecy::SecretString::new(plaintext.into()))
        .map_err(OperationPersistError::Cipher)?;
    let actor = op
        .actor
        .as_deref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(OperationPersistError::Json)?;
    db.conn.execute(
        "INSERT INTO queued_operations
            (operation_id, app, action, trigger, source_generation, target_generation,
             params_ciphertext, actor)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            op.operation_id.0,
            op.app,
            op.action,
            op.trigger,
            op.source_generation as i64,
            op.target_generation as i64,
            ciphertext,
            actor,
        ],
    )?;
    Ok(())
}

/// Remove a queued request's record, because it has started or been refused.
// spec: OPS#concurrency
pub fn clear_queued_operation(db: &Db, operation_id: &OperationId) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM queued_operations WHERE operation_id = ?1",
        [&operation_id.0],
    )?;
    Ok(())
}

/// Every persisted queued request, in the order it was made.
///
/// A record that cannot be read fails the whole read rather than being
/// skipped: a queue missing a request is not the queue that was persisted.
// spec: OPS#concurrency
pub fn load_queued_operations(
    db: &Db,
    cipher: &crate::runtime::secrets::Cipher,
) -> Result<Vec<crate::runtime::scheduler::QueuedOperation>, OperationPersistError> {
    use secrecy::ExposeSecret;
    let mut stmt = db.conn.prepare(
        "SELECT operation_id, app, action, trigger, source_generation, target_generation,
                params_ciphertext, actor
           FROM queued_operations ORDER BY seq",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, AppName>(1)?,
            row.get::<_, ActionName>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, Vec<u8>>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, app, action, trigger, source, target, ciphertext, actor) = row?;
        let plaintext = cipher
            .decrypt(&ciphertext)
            .map_err(OperationPersistError::Cipher)?;
        let params =
            serde_json::from_str(plaintext.expose_secret()).map_err(OperationPersistError::Json)?;
        let actor = actor
            .map(|a| serde_json::from_str(&a))
            .transpose()
            .map_err(OperationPersistError::Json)?
            .map(std::sync::Arc::new);
        out.push(crate::runtime::scheduler::QueuedOperation {
            app,
            action,
            operation_id: OperationId(id),
            params,
            source_generation: source as u64,
            target_generation: target as u64,
            trigger,
            actor,
        });
    }
    Ok(out)
}

/// What the runtime persisted about lifecycle operations when it stopped.
pub struct PersistedOperationState {
    /// The operation that was in progress, if any.
    pub current: Option<CurrentOperation>,
    /// The requests that were waiting, in the order they were made.
    pub queued: Vec<crate::runtime::scheduler::QueuedOperation>,
}

/// Read the persisted operation, whether a cancel was persisted for it, and
/// the persisted queue, or say which could not be read.
///
/// All three are read or none is answered for: acting on a part of the state
/// would treat what could not be read as absent.
// spec: OPS#unreadable-operation-state
pub fn read_operation_state(
    db: &Db,
    cipher: &crate::runtime::secrets::Cipher,
) -> Result<PersistedOperationState, String> {
    let current =
        load_current_operation(db).map_err(|e| format!("the persisted operation: {e}"))?;
    if current.is_some() {
        load_cancel_requested(db).map_err(|e| format!("the persisted cancel: {e}"))?;
    }
    let queued = load_queued_operations(db, cipher)
        .map_err(|e| format!("the persisted operation queue: {e}"))?;
    Ok(PersistedOperationState { current, queued })
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
// spec: OPS#cancel-persistence
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
// spec: OPS#cancel-persistence
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

/// How a resource kind is spelt in `resource_instances.kind`.
///
/// The column was written with `format!("{:?}")` and read by a hand-written
/// match, so the two could drift — and did: `ExternalService` rows are
/// persisted by the reconciler but had no arm here, so `find_instance` on one
/// returned a conversion failure rather than the instance. An exhaustive
/// match means a new variant stops compiling here instead.
fn resource_kind_str(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Parameter => "Parameter",
        ResourceKind::Service => "Service",
        ResourceKind::Ingress => "Ingress",
        ResourceKind::Deployment => "Deployment",
        ResourceKind::Job => "Job",
        ResourceKind::Volume => "Volume",
        ResourceKind::ExternalVolume => "ExternalVolume",
        ResourceKind::ExternalService => "ExternalService",
        ResourceKind::Action => "Action",
    }
}

fn parse_resource_kind(s: &str) -> Result<ResourceKind, rusqlite::Error> {
    match s {
        "Parameter" => Ok(ResourceKind::Parameter),
        "Service" => Ok(ResourceKind::Service),
        "Ingress" => Ok(ResourceKind::Ingress),
        "Deployment" => Ok(ResourceKind::Deployment),
        "Job" => Ok(ResourceKind::Job),
        "Volume" => Ok(ResourceKind::Volume),
        "ExternalVolume" => Ok(ResourceKind::ExternalVolume),
        "ExternalService" => Ok(ResourceKind::ExternalService),
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
