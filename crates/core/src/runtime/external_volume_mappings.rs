use rusqlite::params;
use seedling_protocol::names::{
    AppName, AppVolumeName, ExternalVolumeName, SiteVolumeName, VolumeRef,
};

use crate::runtime::db::Db;

#[derive(Debug, Clone)]
pub struct ExternalVolumeMapping {
    pub app: AppName,
    pub external_name: ExternalVolumeName,
    pub target: VolumeRef,
    pub read_only: bool,
}

/// Decompose a [`VolumeRef`] into the `(target_kind, target_app, target_volume)`
/// tuple stored across three columns in the `external_volume_mappings` table.
fn to_row(target: &VolumeRef) -> (&'static str, Option<&AppName>, &str) {
    match target {
        VolumeRef::App { app, volume } => ("app", Some(app), volume.as_str()),
        VolumeRef::Site { name } => ("site", None, name.as_str()),
    }
}

/// Reassemble a [`VolumeRef`] from the DB columns. The `target_kind` column
/// is always present; `target_app` is only populated for the `app` kind.
/// Historical rows may carry the legacy `"exported"` kind — treat that as
/// `"app"`.
fn from_row(kind: &str, target_app: Option<AppName>, target_volume: String) -> VolumeRef {
    match kind {
        "app" | "exported" => VolumeRef::App {
            app: target_app.unwrap_or_default(),
            volume: AppVolumeName::new_unchecked(target_volume),
        },
        _ => VolumeRef::Site {
            name: SiteVolumeName::new_unchecked(target_volume),
        },
    }
}

pub fn create(db: &Db, mapping: &ExternalVolumeMapping) -> rusqlite::Result<()> {
    let (kind, target_app, target_volume) = to_row(&mapping.target);
    db.conn.execute(
        "INSERT INTO external_volume_mappings (app, external_name, target_kind, target_app, target_volume, read_only)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![mapping.app, mapping.external_name, kind, target_app, target_volume, mapping.read_only as i32],
    )?;
    Ok(())
}

pub fn update(db: &Db, mapping: &ExternalVolumeMapping) -> rusqlite::Result<bool> {
    let (kind, target_app, target_volume) = to_row(&mapping.target);
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

pub fn delete(
    db: &Db,
    app: &AppName,
    external_name: &ExternalVolumeName,
) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "DELETE FROM external_volume_mappings WHERE app = ?1 AND external_name = ?2",
        params![app, external_name],
    )?;
    Ok(count > 0)
}

pub fn list_for_app(db: &Db, app: &AppName) -> rusqlite::Result<Vec<ExternalVolumeMapping>> {
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
    app: &AppName,
    external_name: &ExternalVolumeName,
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
    let app: AppName = row.get(0)?;
    let external_name: ExternalVolumeName = row.get(1)?;
    let kind: String = row.get(2)?;
    let target_app: Option<AppName> = row.get(3)?;
    let target_volume: String = row.get(4)?;
    let read_only: bool = row.get(5)?;
    let target = from_row(kind.as_str(), target_app, target_volume);
    Ok(ExternalVolumeMapping {
        app,
        external_name,
        target,
        read_only,
    })
}

#[cfg(test)]
mod tests;
