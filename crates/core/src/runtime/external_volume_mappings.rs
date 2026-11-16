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
fn from_row(
    kind: &str,
    target_app: Option<AppName>,
    target_volume: String,
) -> Result<VolumeRef, String> {
    match (kind, target_app) {
        ("app" | "exported", Some(app)) => Ok(VolumeRef::App {
            app,
            volume: AppVolumeName::new_unchecked(target_volume),
        }),
        ("app" | "exported", None) => Err("an app target with no app".to_owned()),
        ("site", _) => Ok(VolumeRef::Site {
            name: SiteVolumeName::new_unchecked(target_volume),
        }),
        (other, _) => Err(format!("unknown target kind {other:?}")),
    }
}

/// What [`create_unique`] and [`update_existing`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Written {
    Done,
    /// The app already has a mapping for the external name.
    Duplicate,
    /// The app has no mapping for the external name.
    Missing,
    /// The mapping targets a site volume that is not recorded.
    SiteTargetMissing,
}

fn site_target_missing(db: &Db, target: &VolumeRef) -> rusqlite::Result<bool> {
    match target {
        VolumeRef::Site { name } => {
            Ok(crate::runtime::site_volumes::get(db, name)?.is_none())
        }
        VolumeRef::App { .. } => Ok(false),
    }
}

/// Create a mapping the app does not already have, to a site target that is
/// recorded.
///
/// The target is looked for and the mapping written as one step, which
/// [`site_volumes::delete_unmapped`](crate::runtime::site_volumes::delete_unmapped)
/// relies on.
// spec: VOLM#mapping-an-external-volume
pub fn create_unique(db: &Db, mapping: &ExternalVolumeMapping) -> rusqlite::Result<Written> {
    if site_target_missing(db, &mapping.target)? {
        return Ok(Written::SiteTargetMissing);
    }
    if get(db, &mapping.app, &mapping.external_name)?.is_some() {
        return Ok(Written::Duplicate);
    }
    create(db, mapping)?;
    Ok(Written::Done)
}

/// Retarget a mapping the app has, to a site target that is recorded, and give
/// the mapping it had.
// spec: VOLM#remapping-an-external-volume
pub fn update_existing(
    db: &Db,
    mapping: &ExternalVolumeMapping,
) -> rusqlite::Result<(Written, Option<ExternalVolumeMapping>)> {
    let Some(previous) = get(db, &mapping.app, &mapping.external_name)? else {
        return Ok((Written::Missing, None));
    };
    if site_target_missing(db, &mapping.target)? {
        return Ok((Written::SiteTargetMissing, Some(previous)));
    }
    update(db, mapping)?;
    Ok((Written::Done, Some(previous)))
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

/// Discard every mapping held under an app's name, as when it is
/// deregistered. The app name is the runtime's identity for an app, so a
/// mapping left behind is inherited by the next app registered under that
/// name rather than belonging to nobody.
// spec: TRD#deregistration
pub fn delete_for_app(db: &Db, app: &AppName) -> rusqlite::Result<usize> {
    db.conn.execute(
        "DELETE FROM external_volume_mappings WHERE app = ?1",
        params![app],
    )
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
    let target = from_row(kind.as_str(), target_app, target_volume).map_err(|why| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, why.into())
    })?;
    Ok(ExternalVolumeMapping {
        app,
        external_name,
        target,
        read_only,
    })
}

#[cfg(test)]
mod tests;
