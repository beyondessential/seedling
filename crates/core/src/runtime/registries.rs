use crate::runtime::db::Db;

pub fn list_allowed_registries(db: &Db) -> rusqlite::Result<Vec<String>> {
    let mut stmt = db
        .conn
        .prepare("SELECT registry FROM allowed_registries ORDER BY registry")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    rows.collect()
}

pub fn add_allowed_registry(db: &Db, registry: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR IGNORE INTO allowed_registries (registry) VALUES (?1)",
        [registry],
    )?;
    Ok(())
}

pub fn remove_allowed_registry(db: &Db, registry: &str) -> rusqlite::Result<bool> {
    let changed = db.conn.execute(
        "DELETE FROM allowed_registries WHERE registry = ?1",
        [registry],
    )?;
    Ok(changed > 0)
}

pub fn is_registry_allowed(db: &Db, registry: &str) -> rusqlite::Result<bool> {
    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM allowed_registries WHERE registry = ?1",
        [registry],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}
