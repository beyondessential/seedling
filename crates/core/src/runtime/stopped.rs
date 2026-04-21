use std::collections::HashSet;

use rusqlite::params;
use seedling_protocol::names::AppName;

use crate::defs::resource::ResourceKind;
use crate::runtime::db::Db;

/// The set of resources explicitly stopped for an app.
/// Each entry is `(kind, name)`.
pub type StoppedSet = HashSet<(ResourceKind, String)>;

// i[impl resource.stop]
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
        if let Some(kind) = parse_kind(&kind_str) {
            set.insert((kind, name));
        }
    }
    Ok(set)
}

// i[impl resource.stop]
pub fn stop_resource(
    db: &Db,
    app: &AppName,
    kind: ResourceKind,
    name: &str,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR IGNORE INTO stopped_resources (app, kind, name) VALUES (?1, ?2, ?3)",
        params![app, kind_str(kind), name],
    )?;
    Ok(())
}

// i[impl resource.unstop]
pub fn unstop_resource(
    db: &Db,
    app: &AppName,
    kind: ResourceKind,
    name: &str,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM stopped_resources WHERE app = ?1 AND kind = ?2 AND name = ?3",
        params![app, kind_str(kind), name],
    )?;
    Ok(())
}

// i[impl resource.unstop-all]
pub fn unstop_all(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM stopped_resources WHERE app = ?1", params![app])?;
    Ok(())
}

/// Delete all stopped records for an app on deregister / uninstall.
pub fn delete_stopped_for_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    unstop_all(db, app)
}

pub fn kind_str(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Deployment => "deployment",
        ResourceKind::Job => "job",
        ResourceKind::Ingress => "ingress",
        ResourceKind::Service => "service",
        ResourceKind::Volume => "volume",
        ResourceKind::ExternalVolume => "externalvolume",
        ResourceKind::Parameter => "parameter",
        ResourceKind::HttpService => "httpservice",
        ResourceKind::Action => "action",
    }
}

pub fn parse_kind(s: &str) -> Option<ResourceKind> {
    match s {
        "deployment" => Some(ResourceKind::Deployment),
        "job" => Some(ResourceKind::Job),
        "ingress" => Some(ResourceKind::Ingress),
        "service" => Some(ResourceKind::Service),
        "volume" => Some(ResourceKind::Volume),
        "externalvolume" => Some(ResourceKind::ExternalVolume),
        "parameter" => Some(ResourceKind::Parameter),
        "httpservice" => Some(ResourceKind::HttpService),
        "action" => Some(ResourceKind::Action),
        _ => None,
    }
}
