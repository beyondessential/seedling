use std::collections::HashSet;

use rusqlite::params;
use seedling_protocol::names::AppName;

use crate::defs::resource::ResourceKind;
use crate::runtime::db::Db;

/// The set of resources explicitly stopped for an app.
/// Each entry is `(kind, name)`.
pub type StoppedSet = HashSet<(ResourceKind, String)>;

// spec: CTL#resource-stop
pub fn load_stopped(db: &Db, app: &AppName) -> rusqlite::Result<StoppedSet> {
    let mut stmt = db
        .conn
        .prepare("SELECT kind, name FROM stopped_resources WHERE app = ?1")?;
    let rows = stmt.query_map(params![app], |row| {
        let kind_str: String = row.get(0)?;
        let name: String = row.get(1)?;
        Ok((kind_str, name))
    })?;
    let mut set = StoppedSet::new();
    for row in rows {
        let (kind_str, name) = row?;
        if let Some(kind) = ResourceKind::from_type_name(&kind_str) {
            set.insert((kind, name));
        }
    }
    Ok(set)
}

// spec: CTL#resource-stop
pub fn stop_resource(
    db: &Db,
    app: &AppName,
    kind: ResourceKind,
    name: &str,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR IGNORE INTO stopped_resources (app, kind, name) VALUES (?1, ?2, ?3)",
        params![app, kind.type_name(), name],
    )?;
    Ok(())
}

/// Remove a stop, reporting whether one was recorded to remove.
// spec: CTL#resource-stop
pub fn unstop_resource(
    db: &Db,
    app: &AppName,
    kind: ResourceKind,
    name: &str,
) -> rusqlite::Result<bool> {
    let removed = db.conn.execute(
        "DELETE FROM stopped_resources WHERE app = ?1 AND kind = ?2 AND name = ?3",
        params![app, kind.type_name(), name],
    )?;
    Ok(removed > 0)
}

// spec: CTL#resource-stop
pub fn unstop_all(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM stopped_resources WHERE app = ?1", params![app])?;
    Ok(())
}

/// Delete all stopped records for an app on deregister / uninstall.
pub fn delete_stopped_for_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    unstop_all(db, app)
}

#[cfg(test)]
mod tests;
