use rusqlite::params;
use seedling_protocol::names::{AppName, AppVolumeName, SiteVolumeName, VolumeRef};

use crate::runtime::db::Db;

// r[impl volume.site]
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
        _ => SiteVolumeKind::Managed,
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

pub fn delete(db: &Db, name: &SiteVolumeName) -> rusqlite::Result<bool> {
    let count = db
        .conn
        .execute("DELETE FROM site_volumes WHERE name = ?1", params![name])?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests;
