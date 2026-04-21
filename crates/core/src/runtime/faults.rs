use std::sync::OnceLock;

use jiff::Timestamp;
use seedling_protocol::events::EventSender;
use seedling_protocol::names::AppName;
use serde::Serialize;
use tracing::warn;

static EVENT_TX: OnceLock<EventSender> = OnceLock::new();

/// Install the broadcast sender used by fault operations.
/// Call once at startup before any faults are filed.
pub fn init(tx: EventSender) {
    EVENT_TX
        .set(tx)
        .expect("faults::init must be called exactly once");
}

// r[impl fault.surfacing]
fn emit_filed(record: &FaultRecord) {
    if let Some(tx) = EVENT_TX.get() {
        tx.fault_filed(
            &record.id,
            &record.app,
            record.resource_type.as_deref(),
            record.resource_name.as_deref(),
            record.instance_id.as_deref(),
            &record.kind,
            &record.description,
        );
    }
}

fn emit_cleared(id: &str, app: &AppName, kind: &str) {
    if let Some(tx) = EVENT_TX.get() {
        tx.fault_cleared(id, app, kind);
    }
}

// r[impl fault.definition]
#[derive(Debug, Clone, Serialize)]
pub struct FaultRecord {
    pub id: String,
    pub app: AppName,
    pub resource_type: Option<String>,
    pub resource_name: Option<String>,
    pub instance_id: Option<String>,
    pub kind: String,
    pub timestamp: Timestamp,
    pub description: String,
}

// i[fault.record]
pub fn file_fault(
    db: &crate::runtime::db::Db,
    app: &AppName,
    resource_type: Option<&str>,
    resource_name: Option<&str>,
    instance_id: Option<&str>,
    kind: &str,
    description: &str,
) -> rusqlite::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Timestamp::now();
    let timestamp = now.to_string();
    db.conn.execute(
        "INSERT INTO faults (id, app, resource_type, resource_name, instance_id, kind, timestamp, description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![id, app, resource_type, resource_name, instance_id, kind, timestamp, description],
    )?;
    warn!(
        app = %app,
        kind, resource_type, resource_name, instance_id, "fault filed: {description}",
    );
    let record = FaultRecord {
        id: id.clone(),
        app: app.clone(),
        resource_type: resource_type.map(str::to_owned),
        resource_name: resource_name.map(str::to_owned),
        instance_id: instance_id.map(str::to_owned),
        kind: kind.to_owned(),
        timestamp: now,
        description: description.to_owned(),
    };
    emit_filed(&record);
    Ok(id)
}

/// Clear a single fault by ID. The `app` is needed for the event broadcast;
/// pass it from the context that looked up the fault record. The fault's
/// kind is read in the same statement and included in the emitted
/// `FaultCleared` event so clients can render a meaningful summary without
/// having to remember every fault ID they saw.
// i[impl fault.derived]
pub fn clear_fault(
    db: &crate::runtime::db::Db,
    fault_id: &str,
    app: &AppName,
) -> rusqlite::Result<()> {
    let now = Timestamp::now();
    // Read the kind before the UPDATE — after clearing, the row is no longer
    // "active" but still exists, so this also works post-clear, but reading
    // up-front keeps the happy path to a single pair of statements.
    let kind: Option<String> = db
        .conn
        .query_row("SELECT kind FROM faults WHERE id = ?1", [fault_id], |row| {
            row.get(0)
        })
        .ok();
    let changed = db.conn.execute(
        "UPDATE faults SET cleared_at = ?1 WHERE id = ?2 AND cleared_at IS NULL",
        rusqlite::params![now.to_string(), fault_id],
    )?;
    if changed > 0 {
        emit_cleared(fault_id, app, kind.as_deref().unwrap_or(""));
    }
    Ok(())
}

// i[fault.list]
pub fn list_active_faults(
    db: &crate::runtime::db::Db,
    app: Option<&AppName>,
) -> rusqlite::Result<Vec<FaultRecord>> {
    let mut records = Vec::new();
    match app {
        Some(app_name) => {
            let mut stmt = db.conn.prepare(
                "SELECT id, app, resource_type, resource_name, instance_id, kind, timestamp, description
                 FROM faults WHERE cleared_at IS NULL AND app = ?1
                 ORDER BY timestamp",
            )?;
            let rows = stmt.query_map([app_name], row_to_record)?;
            for row in rows {
                records.push(row?);
            }
        }
        None => {
            let mut stmt = db.conn.prepare(
                "SELECT id, app, resource_type, resource_name, instance_id, kind, timestamp, description
                 FROM faults WHERE cleared_at IS NULL
                 ORDER BY timestamp",
            )?;
            let rows = stmt.query_map([], row_to_record)?;
            for row in rows {
                records.push(row?);
            }
        }
    }
    Ok(records)
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<FaultRecord> {
    let ts_str: String = row.get(6)?;
    let timestamp = ts_str
        .parse::<Timestamp>()
        .unwrap_or_else(|_| Timestamp::now());
    Ok(FaultRecord {
        id: row.get(0)?,
        app: row.get(1)?,
        resource_type: row.get(2)?,
        resource_name: row.get(3)?,
        instance_id: row.get(4)?,
        kind: row.get(5)?,
        timestamp,
        description: row.get(7)?,
    })
}

/// Clear all active faults matching an app + kind. Returns how many were cleared.
// i[impl fault.derived]
pub fn clear_faults_by_kind(
    db: &crate::runtime::db::Db,
    app: &AppName,
    kind: &str,
) -> rusqlite::Result<u64> {
    let to_clear: Vec<_> = list_active_faults(db, Some(app))?
        .into_iter()
        .filter(|f| f.kind == kind)
        .collect();
    let count = to_clear.len() as u64;
    for f in &to_clear {
        clear_fault(db, &f.id, app)?;
    }
    Ok(count)
}

/// Clear all active faults for an app (used during deregistration).
pub fn clear_all_faults_for_app(
    db: &crate::runtime::db::Db,
    app: &AppName,
) -> rusqlite::Result<()> {
    let to_clear = list_active_faults(db, Some(app))?;
    for f in &to_clear {
        clear_fault(db, &f.id, app)?;
    }
    Ok(())
}

/// Clear every active fault tied to a specific instance_id. Called when the
/// instance is being torn down — either because the operation that created
/// an ephemeral dynamic resource has finished, or because the resource is
/// being retired. Without this, per-instance faults (image_pull_failed,
/// container_start_failed, …) filed against short-lived Job instances
/// linger forever because nothing ever references the dead instance id again.
// i[impl fault.derived]
pub fn clear_faults_for_instance(
    db: &crate::runtime::db::Db,
    app: &AppName,
    instance_id: &str,
) -> rusqlite::Result<()> {
    let to_clear: Vec<_> = list_active_faults(db, Some(app))?
        .into_iter()
        .filter(|f| f.instance_id.as_deref() == Some(instance_id))
        .collect();
    for f in &to_clear {
        clear_fault(db, &f.id, app)?;
    }
    Ok(())
}

pub fn has_active_faults(db: &crate::runtime::db::Db, app: &AppName) -> rusqlite::Result<bool> {
    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM faults WHERE app = ?1 AND cleared_at IS NULL",
        [app],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

pub fn count_active_faults(db: &crate::runtime::db::Db) -> rusqlite::Result<i64> {
    db.conn.query_row(
        "SELECT COUNT(*) FROM faults WHERE cleared_at IS NULL",
        [],
        |r| r.get(0),
    )
}

#[cfg(test)]
mod tests;
