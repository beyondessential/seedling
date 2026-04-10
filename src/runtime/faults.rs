use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct FaultRecord {
    pub id: String,
    pub app: String,
    pub resource_type: Option<String>,
    pub resource_name: Option<String>,
    pub instance_id: Option<String>,
    pub kind: String,
    pub timestamp: DateTime<Utc>,
    pub description: String,
}

// i[fault.record]
pub fn file_fault(
    db: &crate::runtime::db::Db,
    app: &str,
    resource_type: Option<&str>,
    resource_name: Option<&str>,
    instance_id: Option<&str>,
    kind: &str,
    description: &str,
) -> rusqlite::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now: DateTime<Utc> = std::time::SystemTime::now().into();
    let timestamp = now.to_rfc3339();
    db.conn.execute(
        "INSERT INTO faults (id, app, resource_type, resource_name, instance_id, kind, timestamp, description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![id, app, resource_type, resource_name, instance_id, kind, timestamp, description],
    )?;
    Ok(id)
}

pub fn clear_fault(db: &crate::runtime::db::Db, fault_id: &str) -> rusqlite::Result<()> {
    let now: DateTime<Utc> = std::time::SystemTime::now().into();
    db.conn.execute(
        "UPDATE faults SET cleared_at = ?1 WHERE id = ?2",
        rusqlite::params![now.to_rfc3339(), fault_id],
    )?;
    Ok(())
}

// i[fault.list]
pub fn list_active_faults(
    db: &crate::runtime::db::Db,
    app: Option<&str>,
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
    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| std::time::SystemTime::now().into());
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

pub fn clear_faults_by_kind(
    db: &crate::runtime::db::Db,
    app: &str,
    kind: &str,
) -> rusqlite::Result<u64> {
    let now: DateTime<Utc> = std::time::SystemTime::now().into();
    let count = db.conn.execute(
        "UPDATE faults SET cleared_at = ?1 WHERE app = ?2 AND kind = ?3 AND cleared_at IS NULL",
        rusqlite::params![now.to_rfc3339(), app, kind],
    )?;
    Ok(count as u64)
}

pub fn clear_all_faults_for_app(db: &crate::runtime::db::Db, app: &str) -> rusqlite::Result<()> {
    let now: DateTime<Utc> = std::time::SystemTime::now().into();
    db.conn.execute(
        "UPDATE faults SET cleared_at = ?1 WHERE app = ?2 AND cleared_at IS NULL",
        rusqlite::params![now.to_rfc3339(), app],
    )?;
    Ok(())
}

pub fn has_active_faults(db: &crate::runtime::db::Db, app: &str) -> rusqlite::Result<bool> {
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
