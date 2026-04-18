use rusqlite::params;

use crate::runtime::db::Db;

pub const VALID_SCHEDULES: &[&str] = &["every hour", "twice a day", "every day"];

#[derive(Debug, Clone)]
pub struct BackupStrategy {
    pub name: String,
    pub via: String,
    pub schedule: String,
    pub volumes: Vec<String>,
}

// i[impl backup.strategy.create]
pub fn create(db: &Db, strategy: &BackupStrategy) -> rusqlite::Result<()> {
    let volumes_json =
        serde_json::to_string(&strategy.volumes).expect("Vec<String> always serialises");
    db.conn.execute(
        "INSERT INTO backup_strategies (name, via, schedule, volumes) VALUES (?1, ?2, ?3, ?4)",
        params![strategy.name, strategy.via, strategy.schedule, volumes_json],
    )?;
    Ok(())
}

pub fn get(db: &Db, name: &str) -> rusqlite::Result<Option<BackupStrategy>> {
    let mut stmt = db
        .conn
        .prepare("SELECT name, via, schedule, volumes FROM backup_strategies WHERE name = ?1")?;
    let mut rows = stmt.query_map(params![name], row_to_strategy)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

// i[impl backup.strategy.list]
pub fn list_all(db: &Db) -> rusqlite::Result<Vec<BackupStrategy>> {
    let mut stmt = db
        .conn
        .prepare("SELECT name, via, schedule, volumes FROM backup_strategies ORDER BY name")?;
    let rows = stmt.query_map([], row_to_strategy)?;
    rows.collect()
}

// i[impl backup.strategy.update]
pub fn update(
    db: &Db,
    name: &str,
    via: Option<&str>,
    schedule: Option<&str>,
    volumes: Option<&[String]>,
) -> rusqlite::Result<bool> {
    let strategy = match get(db, name)? {
        Some(s) => s,
        None => return Ok(false),
    };
    let new_via = via.unwrap_or(&strategy.via);
    let new_schedule = schedule.unwrap_or(&strategy.schedule);
    let new_volumes = volumes.unwrap_or(&strategy.volumes);
    let volumes_json = serde_json::to_string(new_volumes).expect("Vec<String> always serialises");
    let count = db.conn.execute(
        "UPDATE backup_strategies SET via = ?2, schedule = ?3, volumes = ?4 WHERE name = ?1",
        params![name, new_via, new_schedule, volumes_json],
    )?;
    Ok(count > 0)
}

// i[impl backup.strategy.delete]
pub fn delete(db: &Db, name: &str) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "DELETE FROM backup_strategies WHERE name = ?1",
        params![name],
    )?;
    Ok(count > 0)
}

pub fn references_backup_app(db: &Db, backup_app_name: &str) -> rusqlite::Result<bool> {
    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM backup_strategies WHERE via = ?1",
        params![backup_app_name],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

fn row_to_strategy(row: &rusqlite::Row<'_>) -> rusqlite::Result<BackupStrategy> {
    let name: String = row.get(0)?;
    let via: String = row.get(1)?;
    let schedule: String = row.get(2)?;
    let volumes_json: String = row.get(3)?;
    let volumes: Vec<String> = serde_json::from_str(&volumes_json).unwrap_or_default();
    Ok(BackupStrategy {
        name,
        via,
        schedule,
        volumes,
    })
}
