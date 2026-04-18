use rusqlite::params;

use crate::runtime::db::Db;

pub const REQUIRED_ACTIONS: &[&str] = &["save-snapshot", "list-snapshots", "restore-snapshot"];

#[derive(Debug, Clone)]
pub struct BackupApp {
    pub name: String,
    pub app: String,
}

// i[impl backup.app.register]
pub fn register(db: &Db, name: &str, app: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO backup_apps (name, app) VALUES (?1, ?2)",
        params![name, app],
    )?;
    Ok(())
}

// i[impl backup.app.deregister]
pub fn deregister(db: &Db, name: &str) -> rusqlite::Result<bool> {
    let count = db
        .conn
        .execute("DELETE FROM backup_apps WHERE name = ?1", params![name])?;
    Ok(count > 0)
}

pub fn get_by_name(db: &Db, name: &str) -> rusqlite::Result<Option<BackupApp>> {
    let mut stmt = db
        .conn
        .prepare("SELECT name, app FROM backup_apps WHERE name = ?1")?;
    let mut rows = stmt.query_map(params![name], row_to_backup_app)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

pub fn get_by_app(db: &Db, app: &str) -> rusqlite::Result<Option<BackupApp>> {
    let mut stmt = db
        .conn
        .prepare("SELECT name, app FROM backup_apps WHERE app = ?1")?;
    let mut rows = stmt.query_map(params![app], row_to_backup_app)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

// i[impl backup.app.list]
pub fn list_all(db: &Db) -> rusqlite::Result<Vec<BackupApp>> {
    let mut stmt = db
        .conn
        .prepare("SELECT name, app FROM backup_apps ORDER BY name")?;
    let rows = stmt.query_map([], row_to_backup_app)?;
    rows.collect()
}

fn row_to_backup_app(row: &rusqlite::Row<'_>) -> rusqlite::Result<BackupApp> {
    Ok(BackupApp {
        name: row.get(0)?,
        app: row.get(1)?,
    })
}
