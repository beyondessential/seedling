use std::{fmt, str::FromStr};

use rusqlite::{
    OptionalExtension, params,
    types::{FromSql, FromSqlError, ToSqlOutput, ValueRef},
};
use seedling_protocol::names::{AppName, AppServiceName, SiteIngressName};
use serde::{Deserialize, Serialize};

use crate::runtime::db::Db;

// r[impl ingress.site.attachment]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteIngressAttachment {
    pub site_ingress: SiteIngressName,
    pub port: u16,
    pub protocol: AttachmentProtocol,
    pub target: AttachmentTarget,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentProtocol {
    /// L4 TCP passthrough on the listening port.
    Tcp,
    /// L4 UDP passthrough on the listening port.
    Udp,
    /// HTTP/1.1 reverse-proxy (TLS termination decided by the parent
    /// site ingress's TLS provider).
    Http,
    /// HTTP/2 reverse-proxy (TLS termination decided by the parent
    /// site ingress's TLS provider).
    Http2,
}

impl AttachmentProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Http => "http",
            Self::Http2 => "http2",
        }
    }
}

impl fmt::Display for AttachmentProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AttachmentProtocol {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tcp" => Ok(Self::Tcp),
            "udp" => Ok(Self::Udp),
            "http" => Ok(Self::Http),
            "http2" => Ok(Self::Http2),
            other => Err(format!(
                "invalid attachment protocol {other:?}, expected \"tcp\", \"udp\", \"http\", or \"http2\""
            )),
        }
    }
}

impl rusqlite::ToSql for AttachmentProtocol {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.as_str().to_sql()
    }
}

impl FromSql for AttachmentProtocol {
    fn column_result(value: ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = value.as_str()?;
        s.parse().map_err(|e: String| FromSqlError::Other(e.into()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachmentTarget {
    /// Forward traffic to an app service. The app's service is resolved
    /// to its current set of upstreams at reconcile time.
    Forward {
        app: AppName,
        service: AppServiceName,
    },
    /// Answer requests with an HTTP redirect to a fixed URL.
    Redirect {
        url: String,
        code: u16,
        preserve_path: bool,
    },
}

impl AttachmentTarget {
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Forward { .. } => "forward",
            Self::Redirect { .. } => "redirect",
        }
    }
}

fn target_columns(
    target: &AttachmentTarget,
) -> (
    &'static str,
    Option<&AppName>,
    Option<&AppServiceName>,
    Option<&str>,
    Option<i64>,
    Option<i64>,
) {
    match target {
        AttachmentTarget::Forward { app, service } => {
            ("forward", Some(app), Some(service), None, None, None)
        }
        AttachmentTarget::Redirect {
            url,
            code,
            preserve_path,
        } => (
            "redirect",
            None,
            None,
            Some(url.as_str()),
            Some(i64::from(*code)),
            Some(i64::from(*preserve_path)),
        ),
    }
}

fn target_from_columns(
    kind: &str,
    target_app: Option<AppName>,
    target_service: Option<AppServiceName>,
    redirect_url: Option<String>,
    redirect_code: Option<i64>,
    redirect_preserve_path: Option<i64>,
) -> rusqlite::Result<AttachmentTarget> {
    match kind {
        "forward" => {
            let app = target_app.ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    "forward attachment missing target_app".into(),
                )
            })?;
            let service = target_service.ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    "forward attachment missing target_service".into(),
                )
            })?;
            Ok(AttachmentTarget::Forward { app, service })
        }
        "redirect" => {
            let url = redirect_url.ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    "redirect attachment missing redirect_url".into(),
                )
            })?;
            let code = redirect_code.ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Integer,
                    "redirect attachment missing redirect_code".into(),
                )
            })?;
            let preserve_path = redirect_preserve_path.ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Integer,
                    "redirect attachment missing redirect_preserve_path".into(),
                )
            })?;
            Ok(AttachmentTarget::Redirect {
                url,
                code: code as u16,
                preserve_path: preserve_path != 0,
            })
        }
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown attachment target kind {other:?}").into(),
        )),
    }
}

// r[impl ingress.site.attachment]
pub fn attach(db: &Db, att: &SiteIngressAttachment) -> rusqlite::Result<()> {
    let (kind, app, service, url, code, preserve_path) = target_columns(&att.target);
    db.conn.execute(
        "INSERT INTO site_ingress_attachments
             (site_ingress, port, protocol, target_kind,
              target_app, target_service,
              redirect_url, redirect_code, redirect_preserve_path,
              created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            att.site_ingress,
            att.port,
            att.protocol,
            kind,
            app,
            service,
            url,
            code,
            preserve_path,
            att.created_at,
        ],
    )?;
    Ok(())
}

pub fn update(db: &Db, att: &SiteIngressAttachment) -> rusqlite::Result<bool> {
    let (kind, app, service, url, code, preserve_path) = target_columns(&att.target);
    let count = db.conn.execute(
        "UPDATE site_ingress_attachments
         SET target_kind = ?4, target_app = ?5, target_service = ?6,
             redirect_url = ?7, redirect_code = ?8, redirect_preserve_path = ?9
         WHERE site_ingress = ?1 AND port = ?2 AND protocol = ?3",
        params![
            att.site_ingress,
            att.port,
            att.protocol,
            kind,
            app,
            service,
            url,
            code,
            preserve_path,
        ],
    )?;
    Ok(count > 0)
}

pub fn detach(
    db: &Db,
    site_ingress: &SiteIngressName,
    port: u16,
    protocol: AttachmentProtocol,
) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "DELETE FROM site_ingress_attachments
         WHERE site_ingress = ?1 AND port = ?2 AND protocol = ?3",
        params![site_ingress, port, protocol],
    )?;
    Ok(count > 0)
}

pub fn get(
    db: &Db,
    site_ingress: &SiteIngressName,
    port: u16,
    protocol: AttachmentProtocol,
) -> rusqlite::Result<Option<SiteIngressAttachment>> {
    db.conn
        .query_row(
            "SELECT site_ingress, port, protocol, target_kind,
                    target_app, target_service,
                    redirect_url, redirect_code, redirect_preserve_path,
                    created_at
             FROM site_ingress_attachments
             WHERE site_ingress = ?1 AND port = ?2 AND protocol = ?3",
            params![site_ingress, port, protocol],
            row_to_att,
        )
        .optional()
}

pub fn list_for_ingress(
    db: &Db,
    site_ingress: &SiteIngressName,
) -> rusqlite::Result<Vec<SiteIngressAttachment>> {
    let mut stmt = db.conn.prepare(
        "SELECT site_ingress, port, protocol, target_kind,
                target_app, target_service,
                redirect_url, redirect_code, redirect_preserve_path,
                created_at
         FROM site_ingress_attachments
         WHERE site_ingress = ?1
         ORDER BY port, protocol",
    )?;
    let rows = stmt.query_map(params![site_ingress], row_to_att)?;
    rows.collect()
}

pub fn list_all(db: &Db) -> rusqlite::Result<Vec<SiteIngressAttachment>> {
    let mut stmt = db.conn.prepare(
        "SELECT site_ingress, port, protocol, target_kind,
                target_app, target_service,
                redirect_url, redirect_code, redirect_preserve_path,
                created_at
         FROM site_ingress_attachments
         ORDER BY site_ingress, port, protocol",
    )?;
    let rows = stmt.query_map([], row_to_att)?;
    rows.collect()
}

fn row_to_att(row: &rusqlite::Row<'_>) -> rusqlite::Result<SiteIngressAttachment> {
    let site_ingress: SiteIngressName = row.get(0)?;
    let port: i64 = row.get(1)?;
    let protocol: AttachmentProtocol = row.get(2)?;
    let kind: String = row.get(3)?;
    let target_app: Option<AppName> = row.get(4)?;
    let target_service: Option<AppServiceName> = row.get(5)?;
    let redirect_url: Option<String> = row.get(6)?;
    let redirect_code: Option<i64> = row.get(7)?;
    let redirect_preserve_path: Option<i64> = row.get(8)?;
    let created_at: String = row.get(9)?;
    let target = target_from_columns(
        &kind,
        target_app,
        target_service,
        redirect_url,
        redirect_code,
        redirect_preserve_path,
    )?;
    Ok(SiteIngressAttachment {
        site_ingress,
        port: port as u16,
        protocol,
        target,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use seedling_protocol::names::SiteIngressName;

    use super::*;
    use crate::runtime::site_ingresses::{self, SiteIngressDef, SiteIngressSource, TlsProvider};

    fn mkdb() -> Db {
        Db::open_in_memory().expect("open in-memory db")
    }

    fn parent(db: &Db, name: &str, hostname: &str) -> SiteIngressName {
        let def = SiteIngressDef {
            name: SiteIngressName::new(name).unwrap(),
            hostname: hostname.into(),
            description: None,
            source: SiteIngressSource::Manual,
            tls_provider: TlsProvider::Acme,
            stale: false,
            created_at: "2026-04-28T00:00:00Z".into(),
        };
        site_ingresses::create(db, &def).unwrap();
        def.name
    }

    fn forward(name: &SiteIngressName, port: u16) -> SiteIngressAttachment {
        SiteIngressAttachment {
            site_ingress: name.clone(),
            port,
            protocol: AttachmentProtocol::Http,
            target: AttachmentTarget::Forward {
                app: AppName::new("web").unwrap(),
                service: AppServiceName::new("api").unwrap(),
            },
            created_at: "2026-04-28T00:00:00Z".into(),
        }
    }

    fn redirect(name: &SiteIngressName, port: u16) -> SiteIngressAttachment {
        SiteIngressAttachment {
            site_ingress: name.clone(),
            port,
            protocol: AttachmentProtocol::Http,
            target: AttachmentTarget::Redirect {
                url: "https://new.example.com".into(),
                code: 307,
                preserve_path: true,
            },
            created_at: "2026-04-28T00:00:00Z".into(),
        }
    }

    #[test]
    fn attach_and_round_trip_forward() {
        let db = mkdb();
        let p = parent(&db, "front", "host.example.com");
        let att = forward(&p, 443);
        attach(&db, &att).unwrap();
        assert_eq!(
            get(&db, &p, 443, AttachmentProtocol::Http).unwrap().as_ref(),
            Some(&att)
        );
    }

    #[test]
    fn attach_and_round_trip_redirect() {
        let db = mkdb();
        let p = parent(&db, "legacy", "old.example.com");
        let att = redirect(&p, 443);
        attach(&db, &att).unwrap();
        assert_eq!(
            get(&db, &p, 443, AttachmentProtocol::Http).unwrap().as_ref(),
            Some(&att)
        );
    }

    #[test]
    fn duplicate_port_protocol_rejected() {
        let db = mkdb();
        let p = parent(&db, "front", "host.example.com");
        attach(&db, &forward(&p, 443)).unwrap();
        let res = attach(&db, &forward(&p, 443));
        assert!(res.is_err(), "duplicate (port, protocol) must be rejected");
    }

    #[test]
    fn detach_removes_row() {
        let db = mkdb();
        let p = parent(&db, "front", "host.example.com");
        attach(&db, &forward(&p, 443)).unwrap();
        assert!(detach(&db, &p, 443, AttachmentProtocol::Http).unwrap());
        assert!(
            get(&db, &p, 443, AttachmentProtocol::Http)
                .unwrap()
                .is_none()
        );
        assert!(!detach(&db, &p, 443, AttachmentProtocol::Http).unwrap());
    }

    #[test]
    fn update_replaces_target() {
        let db = mkdb();
        let p = parent(&db, "front", "host.example.com");
        attach(&db, &forward(&p, 443)).unwrap();
        let mut updated = redirect(&p, 443);
        updated.protocol = AttachmentProtocol::Http;
        assert!(update(&db, &updated).unwrap());
        let got = get(&db, &p, 443, AttachmentProtocol::Http).unwrap().unwrap();
        assert!(matches!(got.target, AttachmentTarget::Redirect { .. }));
    }

    #[test]
    fn cascade_on_parent_delete() {
        let db = mkdb();
        let p = parent(&db, "front", "host.example.com");
        attach(&db, &forward(&p, 443)).unwrap();
        attach(
            &db,
            &SiteIngressAttachment {
                protocol: AttachmentProtocol::Http,
                port: 80,
                ..redirect(&p, 80)
            },
        )
        .unwrap();
        assert_eq!(list_for_ingress(&db, &p).unwrap().len(), 2);

        site_ingresses::delete(&db, &p).unwrap();
        assert!(list_for_ingress(&db, &p).unwrap().is_empty());
    }

    #[test]
    fn list_all_orders_deterministically() {
        let db = mkdb();
        let a = parent(&db, "alpha", "a.example.com");
        let b = parent(&db, "beta", "b.example.com");
        attach(&db, &forward(&b, 443)).unwrap();
        attach(&db, &forward(&a, 8080)).unwrap();
        attach(&db, &forward(&a, 443)).unwrap();
        let names: Vec<(String, u16)> = list_all(&db)
            .unwrap()
            .into_iter()
            .map(|a| (a.site_ingress.into_string(), a.port))
            .collect();
        assert_eq!(
            names,
            vec![
                ("alpha".to_owned(), 443),
                ("alpha".to_owned(), 8080),
                ("beta".to_owned(), 443),
            ]
        );
    }

    #[test]
    fn orphan_attachment_rejected_by_fk() {
        // PRAGMA foreign_keys=ON should reject orphan attachments.
        let db = mkdb();
        let res = attach(
            &db,
            &SiteIngressAttachment {
                site_ingress: SiteIngressName::new("ghost").unwrap(),
                port: 443,
                protocol: AttachmentProtocol::Http,
                target: AttachmentTarget::Forward {
                    app: AppName::new("web").unwrap(),
                    service: AppServiceName::new("api").unwrap(),
                },
                created_at: "2026-04-28T00:00:00Z".into(),
            },
        );
        let err = res.expect_err("FK must reject orphan attachment");
        let msg = format!("{err}").to_lowercase();
        assert!(
            msg.contains("foreign key") || msg.contains("constraint"),
            "unexpected error: {msg}"
        );
    }
}
