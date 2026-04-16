use rusqlite::params;

use crate::runtime::db::Db;

// r[impl volume.site]
#[derive(Debug, Clone)]
pub struct SiteVolumeDef {
    pub name: String,
    pub kind: SiteVolumeKind,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteVolumeKind {
    Managed,
    Bind { host_path: String },
}

pub fn create(db: &Db, def: &SiteVolumeDef) -> rusqlite::Result<()> {
    let (kind_str, host_path) = match &def.kind {
        SiteVolumeKind::Managed => ("managed", None),
        SiteVolumeKind::Bind { host_path } => ("bind", Some(host_path.as_str())),
    };
    db.conn.execute(
        "INSERT INTO site_volumes (name, kind, host_path, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![def.name, kind_str, host_path, def.created_at],
    )?;
    Ok(())
}

pub fn list(db: &Db) -> rusqlite::Result<Vec<SiteVolumeDef>> {
    let mut stmt = db.conn.prepare(
        "SELECT name, kind, host_path, created_at FROM site_volumes ORDER BY name",
    )?;
    let rows = stmt.query_map([], |row| {
        let name: String = row.get(0)?;
        let kind_str: String = row.get(1)?;
        let host_path: Option<String> = row.get(2)?;
        let created_at: String = row.get(3)?;
        let kind = match kind_str.as_str() {
            "bind" => SiteVolumeKind::Bind {
                host_path: host_path.unwrap_or_default(),
            },
            _ => SiteVolumeKind::Managed,
        };
        Ok(SiteVolumeDef {
            name,
            kind,
            created_at,
        })
    })?;
    rows.collect()
}

pub fn get(db: &Db, name: &str) -> rusqlite::Result<Option<SiteVolumeDef>> {
    let mut stmt = db.conn.prepare(
        "SELECT name, kind, host_path, created_at FROM site_volumes WHERE name = ?1",
    )?;
    let mut rows = stmt.query_map(params![name], |row| {
        let name: String = row.get(0)?;
        let kind_str: String = row.get(1)?;
        let host_path: Option<String> = row.get(2)?;
        let created_at: String = row.get(3)?;
        let kind = match kind_str.as_str() {
            "bind" => SiteVolumeKind::Bind {
                host_path: host_path.unwrap_or_default(),
            },
            _ => SiteVolumeKind::Managed,
        };
        Ok(SiteVolumeDef {
            name,
            kind,
            created_at,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

pub fn delete(db: &Db, name: &str) -> rusqlite::Result<bool> {
    let count = db
        .conn
        .execute("DELETE FROM site_volumes WHERE name = ?1", params![name])?;
    Ok(count > 0)
}
