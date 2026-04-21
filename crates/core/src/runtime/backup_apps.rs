use rusqlite::params;
use seedling_protocol::names::AppName;

use crate::runtime::db::Db;

// i[impl backup.app.register]
/// Opt `app` in to the backup role. The BSL script for `app` must already
/// define the `save-snapshot`, `list-snapshots`, and `restore-snapshot`
/// actions — that validation lives on the OI handler side.
pub fn register(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("INSERT INTO backup_apps (app) VALUES (?1)", params![app])?;
    Ok(())
}

// i[impl backup.app.deregister]
pub fn deregister(db: &Db, app: &AppName) -> rusqlite::Result<bool> {
    let count = db
        .conn
        .execute("DELETE FROM backup_apps WHERE app = ?1", params![app])?;
    Ok(count > 0)
}

/// Returns `true` if `app` is currently registered as a backup app.
pub fn is_registered(db: &Db, app: &AppName) -> rusqlite::Result<bool> {
    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM backup_apps WHERE app = ?1",
        params![app],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

// i[impl backup.app.list]
pub fn list_all(db: &Db) -> rusqlite::Result<Vec<AppName>> {
    let mut stmt = db
        .conn
        .prepare("SELECT app FROM backup_apps ORDER BY app")?;
    let rows = stmt.query_map([], |row| row.get::<_, AppName>(0))?;
    rows.collect()
}
