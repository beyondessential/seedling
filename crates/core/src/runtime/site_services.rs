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
    pub service_port: u16,
    pub protocol: SiteServiceProtocol,
    pub remote_host: String,
    pub remote_port: u16,
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
pub fn create(db: &Db, def: &SiteServiceDef) -> rusqlite::Result<()> {
    let tx = db.conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO site_services (name, description, created_at) VALUES (?1, ?2, ?3)",
        params![def.name, def.description, def.created_at],
    )?;
    for ep in &def.endpoints {
        tx.execute(
            "INSERT INTO site_service_endpoints
                 (site_service, service_port, protocol, remote_host, remote_port)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                def.name,
                ep.service_port,
                ep.protocol,
                ep.remote_host,
                ep.remote_port
            ],
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

pub fn delete(db: &Db, name: &SiteServiceName) -> rusqlite::Result<bool> {
    // Endpoints are cascaded by the `site_service_endpoints` FK (PRAGMA
    // foreign_keys is enabled on every connection in Db::open*).
    let count = db
        .conn
        .execute("DELETE FROM site_services WHERE name = ?1", params![name])?;
    Ok(count > 0)
}

pub fn add_endpoint(
    db: &Db,
    name: &SiteServiceName,
    ep: &SiteServiceEndpoint,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO site_service_endpoints
             (site_service, service_port, protocol, remote_host, remote_port)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            name,
            ep.service_port,
            ep.protocol,
            ep.remote_host,
            ep.remote_port
        ],
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
         WHERE site_service = ?1 AND service_port = ?2 AND protocol = ?3
           AND remote_host = ?4 AND remote_port = ?5",
        params![
            name,
            ep.service_port,
            ep.protocol,
            ep.remote_host,
            ep.remote_port
        ],
    )?;
    Ok(count > 0)
}

fn endpoints_for(db: &Db, name: &SiteServiceName) -> rusqlite::Result<Vec<SiteServiceEndpoint>> {
    let mut stmt = db.conn.prepare(
        "SELECT service_port, protocol, remote_host, remote_port
         FROM site_service_endpoints
         WHERE site_service = ?1
         ORDER BY service_port, protocol, remote_host, remote_port",
    )?;
    let rows = stmt.query_map(params![name], |row| {
        Ok(SiteServiceEndpoint {
            service_port: row.get::<_, i64>(0)? as u16,
            protocol: row.get(1)?,
            remote_host: row.get(2)?,
            remote_port: row.get::<_, i64>(3)? as u16,
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

    fn tcp(service_port: u16, remote_host: &str, remote_port: u16) -> SiteServiceEndpoint {
        SiteServiceEndpoint {
            service_port,
            protocol: SiteServiceProtocol::Tcp,
            remote_host: remote_host.into(),
            remote_port,
        }
    }

    #[test]
    fn create_with_endpoints_round_trips() {
        let db = mkdb();
        // A service exposing 5432/tcp balanced across two backends that
        // happen to listen on 15432. The schema keeps service_port and
        // remote_port independent so this works end to end.
        let def = SiteServiceDef {
            name: mkname("postgres-prod"),
            description: Some("primary PG cluster".into()),
            endpoints: vec![tcp(5432, "db1.corp", 15432), tcp(5432, "db2.corp", 15432)],
            created_at: "2026-04-23T00:00:00Z".into(),
        };
        create(&db, &def).unwrap();

        let got = get(&db, &def.name).unwrap().expect("row present");
        assert_eq!(got, def);

        let listed = list(&db).unwrap();
        assert_eq!(listed, vec![def]);
    }

    #[test]
    fn add_and_remove_endpoint() {
        let db = mkdb();
        let name = mkname("upstream-api");
        create(
            &db,
            &SiteServiceDef {
                name: name.clone(),
                description: None,
                endpoints: vec![],
                created_at: "2026-04-23T00:00:00Z".into(),
            },
        )
        .unwrap();

        let ep = SiteServiceEndpoint {
            service_port: 443,
            protocol: SiteServiceProtocol::Http,
            remote_host: "api.upstream".into(),
            remote_port: 8443,
        };
        add_endpoint(&db, &name, &ep).unwrap();
        assert_eq!(
            get(&db, &name).unwrap().unwrap().endpoints,
            vec![ep.clone()]
        );

        assert!(remove_endpoint(&db, &name, &ep).unwrap());
        assert!(get(&db, &name).unwrap().unwrap().endpoints.is_empty());
        assert!(!remove_endpoint(&db, &name, &ep).unwrap());
    }

    #[test]
    fn delete_cascades_endpoints() {
        let db = mkdb();
        let name = mkname("doomed");
        create(
            &db,
            &SiteServiceDef {
                name: name.clone(),
                description: None,
                endpoints: vec![tcp(80, "h1", 80), tcp(80, "h2", 80)],
                created_at: "2026-04-23T00:00:00Z".into(),
            },
        )
        .unwrap();

        assert!(delete(&db, &name).unwrap());
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
    fn endpoints_group_by_service_port() {
        // A site service with two exposed ports, each with its own backend pool.
        // Verify both pools are returned and grouping works.
        let db = mkdb();
        let name = mkname("multi-port");
        create(
            &db,
            &SiteServiceDef {
                name: name.clone(),
                description: None,
                endpoints: vec![
                    tcp(3000, "1.2.3.4", 3000),
                    tcp(3000, "1.2.3.5", 3000),
                    tcp(4000, "1.2.3.4", 4000),
                ],
                created_at: "2026-04-23T00:00:00Z".into(),
            },
        )
        .unwrap();

        let eps = get(&db, &name).unwrap().unwrap().endpoints;

        let port_3000: Vec<_> = eps.iter().filter(|e| e.service_port == 3000).collect();
        assert_eq!(port_3000.len(), 2);
        let port_4000: Vec<_> = eps.iter().filter(|e| e.service_port == 4000).collect();
        assert_eq!(port_4000.len(), 1);
        assert_eq!(port_4000[0].remote_host, "1.2.3.4");
    }

    #[test]
    fn endpoint_insert_for_unknown_site_service_is_rejected() {
        // With PRAGMA foreign_keys=ON, the REFERENCES clause on
        // site_service_endpoints.site_service must block orphan rows.
        let db = mkdb();
        let res = add_endpoint(
            &db,
            &mkname("ghost"),
            &SiteServiceEndpoint {
                service_port: 1,
                protocol: SiteServiceProtocol::Tcp,
                remote_host: "h".into(),
                remote_port: 1,
            },
        );
        let err = res.expect_err("FK must reject orphan endpoint");
        let msg = format!("{err}");
        assert!(
            msg.contains("FOREIGN KEY") || msg.contains("constraint"),
            "unexpected error: {msg}"
        );
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
