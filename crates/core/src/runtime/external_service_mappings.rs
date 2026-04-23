use rusqlite::params;
use seedling_protocol::names::{
    AppName, AppServiceName, ExternalServiceName, ServiceRef, SiteServiceName,
};

use crate::runtime::db::Db;

// r[impl service.external.mapping.events]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalServiceMapping {
    pub app: AppName,
    pub external_name: ExternalServiceName,
    pub target: ServiceRef,
}

/// Decompose a [`ServiceRef`] into the `(target_kind, target_app, target_service)`
/// tuple stored across three columns in the `external_service_mappings` table.
fn to_row(target: &ServiceRef) -> (&'static str, Option<&AppName>, &str) {
    match target {
        ServiceRef::App { app, service } => ("app", Some(app), service.as_str()),
        ServiceRef::Site { name } => ("site", None, name.as_str()),
    }
}

/// Reassemble a [`ServiceRef`] from the DB columns. The `target_kind` column
/// is always present; `target_app` is only populated for the `app` kind.
fn from_row(kind: &str, target_app: Option<AppName>, target_service: String) -> ServiceRef {
    match kind {
        "app" => ServiceRef::App {
            app: target_app.unwrap_or_default(),
            service: AppServiceName::new_unchecked(target_service),
        },
        _ => ServiceRef::Site {
            name: SiteServiceName::new_unchecked(target_service),
        },
    }
}

pub fn create(db: &Db, mapping: &ExternalServiceMapping) -> rusqlite::Result<()> {
    let (kind, target_app, target_service) = to_row(&mapping.target);
    db.conn.execute(
        "INSERT INTO external_service_mappings (app, external_name, target_kind, target_app, target_service)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![mapping.app, mapping.external_name, kind, target_app, target_service],
    )?;
    Ok(())
}

pub fn update(db: &Db, mapping: &ExternalServiceMapping) -> rusqlite::Result<bool> {
    let (kind, target_app, target_service) = to_row(&mapping.target);
    let count = db.conn.execute(
        "UPDATE external_service_mappings
         SET target_kind = ?3, target_app = ?4, target_service = ?5
         WHERE app = ?1 AND external_name = ?2",
        params![
            mapping.app,
            mapping.external_name,
            kind,
            target_app,
            target_service,
        ],
    )?;
    Ok(count > 0)
}

pub fn delete(
    db: &Db,
    app: &AppName,
    external_name: &ExternalServiceName,
) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "DELETE FROM external_service_mappings WHERE app = ?1 AND external_name = ?2",
        params![app, external_name],
    )?;
    Ok(count > 0)
}

pub fn list_for_app(db: &Db, app: &AppName) -> rusqlite::Result<Vec<ExternalServiceMapping>> {
    let mut stmt = db.conn.prepare(
        "SELECT app, external_name, target_kind, target_app, target_service
         FROM external_service_mappings WHERE app = ?1 ORDER BY external_name",
    )?;
    let rows = stmt.query_map(params![app], row_to_mapping)?;
    rows.collect()
}

pub fn list_all(db: &Db) -> rusqlite::Result<Vec<ExternalServiceMapping>> {
    let mut stmt = db.conn.prepare(
        "SELECT app, external_name, target_kind, target_app, target_service
         FROM external_service_mappings ORDER BY app, external_name",
    )?;
    let rows = stmt.query_map([], row_to_mapping)?;
    rows.collect()
}

pub fn get(
    db: &Db,
    app: &AppName,
    external_name: &ExternalServiceName,
) -> rusqlite::Result<Option<ExternalServiceMapping>> {
    let mut stmt = db.conn.prepare(
        "SELECT app, external_name, target_kind, target_app, target_service
         FROM external_service_mappings WHERE app = ?1 AND external_name = ?2",
    )?;
    let mut rows = stmt.query_map(params![app, external_name], row_to_mapping)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Lists every app → external-service mapping that currently targets the
/// given site service. Used to validate that a site service can be deleted.
pub fn list_for_site_target(
    db: &Db,
    site_service: &SiteServiceName,
) -> rusqlite::Result<Vec<ExternalServiceMapping>> {
    let mut stmt = db.conn.prepare(
        "SELECT app, external_name, target_kind, target_app, target_service
         FROM external_service_mappings
         WHERE target_kind = 'site' AND target_service = ?1
         ORDER BY app, external_name",
    )?;
    let rows = stmt.query_map(params![site_service], row_to_mapping)?;
    rows.collect()
}

fn row_to_mapping(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExternalServiceMapping> {
    let app: AppName = row.get(0)?;
    let external_name: ExternalServiceName = row.get(1)?;
    let kind: String = row.get(2)?;
    let target_app: Option<AppName> = row.get(3)?;
    let target_service: String = row.get(4)?;
    let target = from_row(kind.as_str(), target_app, target_service);
    Ok(ExternalServiceMapping {
        app,
        external_name,
        target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mkdb() -> Db {
        Db::open_in_memory().expect("open in-memory db")
    }

    fn mapping_app(app: &str, ext: &str, target_app: &str, target_svc: &str) -> ExternalServiceMapping {
        ExternalServiceMapping {
            app: AppName::new(app).unwrap(),
            external_name: ExternalServiceName::new(ext).unwrap(),
            target: ServiceRef::App {
                app: AppName::new(target_app).unwrap(),
                service: AppServiceName::new(target_svc).unwrap(),
            },
        }
    }

    fn mapping_site(app: &str, ext: &str, target: &str) -> ExternalServiceMapping {
        ExternalServiceMapping {
            app: AppName::new(app).unwrap(),
            external_name: ExternalServiceName::new(ext).unwrap(),
            target: ServiceRef::Site {
                name: SiteServiceName::new(target).unwrap(),
            },
        }
    }

    #[test]
    fn create_and_get_app_target() {
        let db = mkdb();
        let m = mapping_app("web-app", "backend", "api-app", "api");
        create(&db, &m).unwrap();
        let got = get(&db, &m.app, &m.external_name).unwrap();
        assert_eq!(got, Some(m));
    }

    #[test]
    fn create_and_get_site_target() {
        let db = mkdb();
        let m = mapping_site("web-app", "postgres", "postgres-prod");
        create(&db, &m).unwrap();
        let got = get(&db, &m.app, &m.external_name).unwrap();
        assert_eq!(got, Some(m));
    }

    #[test]
    fn update_switches_target() {
        let db = mkdb();
        let mut m = mapping_site("web-app", "cache", "redis-prod");
        create(&db, &m).unwrap();

        m.target = ServiceRef::App {
            app: AppName::new("cache-app").unwrap(),
            service: AppServiceName::new("cache").unwrap(),
        };
        assert!(update(&db, &m).unwrap());

        let got = get(&db, &m.app, &m.external_name).unwrap();
        assert_eq!(got, Some(m));
    }

    #[test]
    fn delete_reports_whether_row_existed() {
        let db = mkdb();
        let m = mapping_site("web-app", "cache", "redis-prod");
        create(&db, &m).unwrap();

        assert!(delete(&db, &m.app, &m.external_name).unwrap());
        assert!(get(&db, &m.app, &m.external_name).unwrap().is_none());
        assert!(!delete(&db, &m.app, &m.external_name).unwrap());
    }

    #[test]
    fn list_for_site_target_filters() {
        let db = mkdb();
        create(&db, &mapping_site("app-a", "primary", "shared-db")).unwrap();
        create(&db, &mapping_site("app-b", "primary", "shared-db")).unwrap();
        create(&db, &mapping_site("app-a", "cache", "shared-cache")).unwrap();
        create(&db, &mapping_app("app-a", "other", "app-c", "svc-b")).unwrap();

        let hits = list_for_site_target(&db, &SiteServiceName::new("shared-db").unwrap()).unwrap();
        let apps: Vec<_> = hits.iter().map(|m| m.app.as_str().to_string()).collect();
        assert_eq!(apps, vec!["app-a", "app-b"]);
    }

    #[test]
    fn list_for_app_and_list_all() {
        let db = mkdb();
        create(&db, &mapping_site("web-app", "primary", "postgres-prod")).unwrap();
        create(&db, &mapping_site("web-app", "cache", "redis-prod")).unwrap();
        create(&db, &mapping_site("api-app", "primary", "postgres-prod")).unwrap();

        let web = list_for_app(&db, &AppName::new("web-app").unwrap()).unwrap();
        assert_eq!(web.len(), 2);
        assert_eq!(list_all(&db).unwrap().len(), 3);
    }
}
