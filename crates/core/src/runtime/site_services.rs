use std::{fmt, str::FromStr};

use rusqlite::{
    OptionalExtension, params,
    types::{FromSql, FromSqlError, ToSqlOutput, ValueRef},
};
use seedling_protocol::names::SiteServiceName;
use serde::{Deserialize, Serialize};

use crate::runtime::db::Db;

// r[impl service.site]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteServiceDef {
    pub name: SiteServiceName,
    pub description: Option<String>,
    pub endpoints: Vec<SiteServiceEndpoint>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SiteServiceEndpoint {
    pub host: String,
    pub port: u16,
    pub protocol: SiteServiceProtocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SiteServiceProtocol {
    Tcp,
    Udp,
    Http,
}

impl SiteServiceProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Http => "http",
        }
    }
}

impl fmt::Display for SiteServiceProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SiteServiceProtocol {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tcp" => Ok(Self::Tcp),
            "udp" => Ok(Self::Udp),
            "http" => Ok(Self::Http),
            other => Err(format!(
                "invalid site service protocol {other:?}, expected \"tcp\", \"udp\", or \"http\""
            )),
        }
    }
}

impl rusqlite::ToSql for SiteServiceProtocol {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.as_str().to_sql()
    }
}

impl FromSql for SiteServiceProtocol {
    fn column_result(value: ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = value.as_str()?;
        s.parse().map_err(|e: String| FromSqlError::Other(e.into()))
    }
}

// r[impl service.site.lifecycle]
pub fn create(db: &mut Db, def: &SiteServiceDef) -> rusqlite::Result<()> {
    let tx = db.conn.transaction()?;
    tx.execute(
        "INSERT INTO site_services (name, description, created_at) VALUES (?1, ?2, ?3)",
        params![def.name, def.description, def.created_at],
    )?;
    for ep in &def.endpoints {
        tx.execute(
            "INSERT INTO site_service_endpoints (site_service, host, port, protocol)
             VALUES (?1, ?2, ?3, ?4)",
            params![def.name, ep.host, ep.port, ep.protocol],
        )?;
    }
    tx.commit()
}

pub fn list(db: &Db) -> rusqlite::Result<Vec<SiteServiceDef>> {
    let mut stmt = db
        .conn
        .prepare("SELECT name, description, created_at FROM site_services ORDER BY name")?;
    let svcs = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, SiteServiceName>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::with_capacity(svcs.len());
    for (name, description, created_at) in svcs {
        let endpoints = endpoints_for(db, &name)?;
        out.push(SiteServiceDef {
            name,
            description,
            endpoints,
            created_at,
        });
    }
    Ok(out)
}

pub fn get(db: &Db, name: &SiteServiceName) -> rusqlite::Result<Option<SiteServiceDef>> {
    let row = db
        .conn
        .query_row(
            "SELECT name, description, created_at FROM site_services WHERE name = ?1",
            params![name],
            |row| {
                Ok((
                    row.get::<_, SiteServiceName>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    match row {
        Some((name, description, created_at)) => {
            let endpoints = endpoints_for(db, &name)?;
            Ok(Some(SiteServiceDef {
                name,
                description,
                endpoints,
                created_at,
            }))
        }
        None => Ok(None),
    }
}

pub fn delete(db: &mut Db, name: &SiteServiceName) -> rusqlite::Result<bool> {
    let tx = db.conn.transaction()?;
    tx.execute(
        "DELETE FROM site_service_endpoints WHERE site_service = ?1",
        params![name],
    )?;
    let count = tx.execute(
        "DELETE FROM site_services WHERE name = ?1",
        params![name],
    )?;
    tx.commit()?;
    Ok(count > 0)
}

pub fn add_endpoint(
    db: &Db,
    name: &SiteServiceName,
    ep: &SiteServiceEndpoint,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO site_service_endpoints (site_service, host, port, protocol)
         VALUES (?1, ?2, ?3, ?4)",
        params![name, ep.host, ep.port, ep.protocol],
    )?;
    Ok(())
}

pub fn remove_endpoint(
    db: &Db,
    name: &SiteServiceName,
    ep: &SiteServiceEndpoint,
) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "DELETE FROM site_service_endpoints
         WHERE site_service = ?1 AND host = ?2 AND port = ?3 AND protocol = ?4",
        params![name, ep.host, ep.port, ep.protocol],
    )?;
    Ok(count > 0)
}

fn endpoints_for(db: &Db, name: &SiteServiceName) -> rusqlite::Result<Vec<SiteServiceEndpoint>> {
    let mut stmt = db.conn.prepare(
        "SELECT host, port, protocol FROM site_service_endpoints
         WHERE site_service = ?1 ORDER BY host, port, protocol",
    )?;
    let rows = stmt.query_map(params![name], |row| {
        Ok(SiteServiceEndpoint {
            host: row.get(0)?,
            port: row.get::<_, i64>(1)? as u16,
            protocol: row.get(2)?,
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mkdb() -> Db {
        Db::open_in_memory().expect("open in-memory db")
    }

    fn mkname(s: &str) -> SiteServiceName {
        SiteServiceName::new(s).expect("valid name")
    }

    fn tcp(host: &str, port: u16) -> SiteServiceEndpoint {
        SiteServiceEndpoint {
            host: host.into(),
            port,
            protocol: SiteServiceProtocol::Tcp,
        }
    }

    #[test]
    fn create_with_endpoints_round_trips() {
        let mut db = mkdb();
        let def = SiteServiceDef {
            name: mkname("postgres-prod"),
            description: Some("primary PG cluster".into()),
            endpoints: vec![tcp("db1.corp", 5432), tcp("db2.corp", 5432)],
            created_at: "2026-04-23T00:00:00Z".into(),
        };
        create(&mut db, &def).unwrap();

        let got = get(&db, &def.name).unwrap().expect("row present");
        assert_eq!(got, def);

        let listed = list(&db).unwrap();
        assert_eq!(listed, vec![def]);
    }

    #[test]
    fn add_and_remove_endpoint() {
        let mut db = mkdb();
        let name = mkname("upstream-api");
        create(
            &mut db,
            &SiteServiceDef {
                name: name.clone(),
                description: None,
                endpoints: vec![],
                created_at: "2026-04-23T00:00:00Z".into(),
            },
        )
        .unwrap();

        let ep = SiteServiceEndpoint {
            host: "api.upstream".into(),
            port: 443,
            protocol: SiteServiceProtocol::Http,
        };
        add_endpoint(&db, &name, &ep).unwrap();
        assert_eq!(get(&db, &name).unwrap().unwrap().endpoints, vec![ep.clone()]);

        assert!(remove_endpoint(&db, &name, &ep).unwrap());
        assert!(get(&db, &name).unwrap().unwrap().endpoints.is_empty());
        assert!(!remove_endpoint(&db, &name, &ep).unwrap());
    }

    #[test]
    fn delete_cascades_endpoints() {
        let mut db = mkdb();
        let name = mkname("doomed");
        create(
            &mut db,
            &SiteServiceDef {
                name: name.clone(),
                description: None,
                endpoints: vec![tcp("h1", 1), tcp("h2", 2)],
                created_at: "2026-04-23T00:00:00Z".into(),
            },
        )
        .unwrap();

        assert!(delete(&mut db, &name).unwrap());
        assert!(get(&db, &name).unwrap().is_none());

        let orphan_count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM site_service_endpoints WHERE site_service = ?1",
                params![name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphan_count, 0);
    }

    #[test]
    fn protocol_serde_and_sql_round_trip() {
        for (p, s) in [
            (SiteServiceProtocol::Tcp, "\"tcp\""),
            (SiteServiceProtocol::Udp, "\"udp\""),
            (SiteServiceProtocol::Http, "\"http\""),
        ] {
            assert_eq!(serde_json::to_string(&p).unwrap(), s);
            let round: SiteServiceProtocol = serde_json::from_str(s).unwrap();
            assert_eq!(round, p);
        }
    }
}
