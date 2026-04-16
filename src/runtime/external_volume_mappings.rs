use rusqlite::params;

use crate::runtime::db::Db;

#[derive(Debug, Clone)]
pub struct ExternalVolumeMapping {
    pub app: String,
    pub external_name: String,
    pub target: MappingTarget,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub enum MappingTarget {
    Exported {
        target_app: String,
        target_volume: String,
    },
    Site {
        target_volume: String,
    },
}

pub fn create(db: &Db, mapping: &ExternalVolumeMapping) -> rusqlite::Result<()> {
    let (kind, target_app, target_volume) = match &mapping.target {
        MappingTarget::Exported {
            target_app,
            target_volume,
        } => (
            "exported",
            Some(target_app.as_str()),
            target_volume.as_str(),
        ),
        MappingTarget::Site { target_volume } => ("site", None, target_volume.as_str()),
    };
    db.conn.execute(
        "INSERT INTO external_volume_mappings (app, external_name, target_kind, target_app, target_volume, read_only)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![mapping.app, mapping.external_name, kind, target_app, target_volume, mapping.read_only as i32],
    )?;
    Ok(())
}

pub fn update(db: &Db, mapping: &ExternalVolumeMapping) -> rusqlite::Result<bool> {
    let (kind, target_app, target_volume) = match &mapping.target {
        MappingTarget::Exported {
            target_app,
            target_volume,
        } => (
            "exported",
            Some(target_app.as_str()),
            target_volume.as_str(),
        ),
        MappingTarget::Site { target_volume } => ("site", None, target_volume.as_str()),
    };
    let count = db.conn.execute(
        "UPDATE external_volume_mappings
         SET target_kind = ?3, target_app = ?4, target_volume = ?5, read_only = ?6
         WHERE app = ?1 AND external_name = ?2",
        params![
            mapping.app,
            mapping.external_name,
            kind,
            target_app,
            target_volume,
            mapping.read_only as i32,
        ],
    )?;
    Ok(count > 0)
}

pub fn delete(db: &Db, app: &str, external_name: &str) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "DELETE FROM external_volume_mappings WHERE app = ?1 AND external_name = ?2",
        params![app, external_name],
    )?;
    Ok(count > 0)
}

pub fn list_for_app(db: &Db, app: &str) -> rusqlite::Result<Vec<ExternalVolumeMapping>> {
    let mut stmt = db.conn.prepare(
        "SELECT app, external_name, target_kind, target_app, target_volume, read_only
         FROM external_volume_mappings WHERE app = ?1 ORDER BY external_name",
    )?;
    let rows = stmt.query_map(params![app], row_to_mapping)?;
    rows.collect()
}

pub fn list_all(db: &Db) -> rusqlite::Result<Vec<ExternalVolumeMapping>> {
    let mut stmt = db.conn.prepare(
        "SELECT app, external_name, target_kind, target_app, target_volume, read_only
         FROM external_volume_mappings ORDER BY app, external_name",
    )?;
    let rows = stmt.query_map([], row_to_mapping)?;
    rows.collect()
}

pub fn get(
    db: &Db,
    app: &str,
    external_name: &str,
) -> rusqlite::Result<Option<ExternalVolumeMapping>> {
    let mut stmt = db.conn.prepare(
        "SELECT app, external_name, target_kind, target_app, target_volume, read_only
         FROM external_volume_mappings WHERE app = ?1 AND external_name = ?2",
    )?;
    let mut rows = stmt.query_map(params![app, external_name], row_to_mapping)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

fn row_to_mapping(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExternalVolumeMapping> {
    let app: String = row.get(0)?;
    let external_name: String = row.get(1)?;
    let kind: String = row.get(2)?;
    let target_app: Option<String> = row.get(3)?;
    let target_volume: String = row.get(4)?;
    let read_only: bool = row.get(5)?;
    let target = match kind.as_str() {
        "exported" => MappingTarget::Exported {
            target_app: target_app.unwrap_or_default(),
            target_volume,
        },
        _ => MappingTarget::Site { target_volume },
    };
    Ok(ExternalVolumeMapping {
        app,
        external_name,
        target,
        read_only,
    })
}
