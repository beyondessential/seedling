use rusqlite::params;
use seedling_protocol::names::{AppName, AppVolumeName, SiteVolumeName, VolumeRef};

use crate::runtime::db::Db;

// spec: SITE#site-volumes
#[derive(Debug, Clone)]
pub struct SiteVolumeDef {
    pub name: SiteVolumeName,
    pub kind: SiteVolumeKind,
    pub created_at: String,
}

impl SiteVolumeDef {
    /// Snapshot volumes are inherently read-only at the filesystem level.
    pub fn is_read_only(&self) -> bool {
        matches!(self.kind, SiteVolumeKind::Snapshot { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteVolumeKind {
    Managed,
    Bind { host_path: String },
    Snapshot { source: VolumeRef },
}

/// Decompose the snapshot source into the `(source_app, source_volume)`
/// columns stored in the `site_volumes` table.
fn snapshot_source_row(source: &VolumeRef) -> (Option<&AppName>, &str) {
    match source {
        VolumeRef::App { app, volume } => (Some(app), volume.as_str()),
        VolumeRef::Site { name } => (None, name.as_str()),
    }
}

/// Reassemble a snapshot source from the `(source_app, source_volume)`
/// column pair. `None` in `source_app` means the source was a site volume.
fn snapshot_source_from_row(source_app: Option<AppName>, source_volume: String) -> VolumeRef {
    match source_app {
        Some(app) => VolumeRef::App {
            app,
            volume: AppVolumeName::new_unchecked(source_volume),
        },
        None => VolumeRef::Site {
            name: SiteVolumeName::new_unchecked(source_volume),
        },
    }
}

pub fn create(db: &Db, def: &SiteVolumeDef) -> rusqlite::Result<()> {
    let (kind_str, host_path, source_app, source_volume) = match &def.kind {
        SiteVolumeKind::Managed => ("managed", None, None, None),
        SiteVolumeKind::Bind { host_path } => ("bind", Some(host_path.as_str()), None, None),
        SiteVolumeKind::Snapshot { source } => {
            let (app, volume) = snapshot_source_row(source);
            ("snapshot", None, app.map(|a| a.as_str()), Some(volume))
        }
    };
    db.conn.execute(
        "INSERT INTO site_volumes (name, kind, host_path, source_app, source_volume, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            def.name,
            kind_str,
            host_path,
            source_app,
            source_volume,
            def.created_at
        ],
    )?;
    Ok(())
}

fn row_to_def(row: &rusqlite::Row<'_>) -> rusqlite::Result<SiteVolumeDef> {
    let name: SiteVolumeName = row.get(0)?;
    let kind_str: String = row.get(1)?;
    let host_path: Option<String> = row.get(2)?;
    let source_app: Option<AppName> = row.get(3)?;
    let source_volume: Option<String> = row.get(4)?;
    let created_at: String = row.get(5)?;
    let kind = match kind_str.as_str() {
        "bind" => SiteVolumeKind::Bind {
            host_path: host_path.unwrap_or_default(),
        },
        "snapshot" => SiteVolumeKind::Snapshot {
            source: snapshot_source_from_row(source_app, source_volume.unwrap_or_default()),
        },
        "managed" => SiteVolumeKind::Managed,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                format!("unknown site volume kind {other:?}").into(),
            ));
        }
    };
    Ok(SiteVolumeDef {
        name,
        kind,
        created_at,
    })
}

pub fn list(db: &Db) -> rusqlite::Result<Vec<SiteVolumeDef>> {
    let mut stmt = db.conn.prepare(
        "SELECT name, kind, host_path, source_app, source_volume, created_at FROM site_volumes ORDER BY name",
    )?;
    let rows = stmt.query_map([], row_to_def)?;
    rows.collect()
}

pub fn get(db: &Db, name: &SiteVolumeName) -> rusqlite::Result<Option<SiteVolumeDef>> {
    let mut stmt = db.conn.prepare(
        "SELECT name, kind, host_path, source_app, source_volume, created_at FROM site_volumes WHERE name = ?1",
    )?;
    let mut rows = stmt.query_map(params![name], row_to_def)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// What [`delete_unmapped`] did.
#[derive(Debug)]
pub enum Deletion {
    /// The record was deleted; this is what it held.
    Deleted(SiteVolumeDef),
    /// No site volume is recorded under the name.
    Absent,
    /// An external volume mapping targets the site volume, and it was left.
    Mapped {
        app: AppName,
        external_name: seedling_protocol::names::ExternalVolumeName,
    },
}

/// Delete a site volume's record unless an external volume mapping targets it.
///
/// Looking for a mapping and deleting are one step on the database's thread, and
/// creating a mapping checks its site target in the same way, so no mapping is
/// left targeting a deleted site volume.
// spec: SITE#site-volumes
pub fn delete_unmapped(db: &Db, name: &SiteVolumeName) -> rusqlite::Result<Deletion> {
    let Some(def) = get(db, name)? else {
        return Ok(Deletion::Absent);
    };
    let mapped = db
        .conn
        .query_row(
            "SELECT app, external_name FROM external_volume_mappings
             WHERE target_kind = 'site' AND target_volume = ?1
             ORDER BY app, external_name LIMIT 1",
            params![name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    if let Some((app, external_name)) = mapped {
        return Ok(Deletion::Mapped { app, external_name });
    }
    delete(db, name)?;
    Ok(Deletion::Deleted(def))
}

pub fn delete(db: &Db, name: &SiteVolumeName) -> rusqlite::Result<bool> {
    let count = db
        .conn
        .execute("DELETE FROM site_volumes WHERE name = ?1", params![name])?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests;
