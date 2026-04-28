use std::{fmt, str::FromStr};

use rusqlite::{
    OptionalExtension, params,
    types::{FromSql, FromSqlError, ToSqlOutput, ValueRef},
};
use seedling_protocol::names::SiteIngressName;
use serde::{Deserialize, Serialize};

use crate::runtime::db::Db;

// r[impl ingress.site]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteIngressDef {
    pub name: SiteIngressName,
    pub hostname: String,
    pub description: Option<String>,
    pub source: SiteIngressSource,
    pub tls_provider: TlsProvider,
    pub stale: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SiteIngressSource {
    Manual,
    Discovered {
        provider: DiscoveryProvider,
        key: String,
    },
}

impl SiteIngressSource {
    pub fn is_discovered(&self) -> bool {
        matches!(self, Self::Discovered { .. })
    }

    pub fn is_manual(&self) -> bool {
        matches!(self, Self::Manual)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscoveryProvider {
    Tailscale,
}

impl DiscoveryProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tailscale => "tailscale",
        }
    }
}

impl fmt::Display for DiscoveryProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DiscoveryProvider {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tailscale" => Ok(Self::Tailscale),
            other => Err(format!(
                "invalid discovery provider {other:?}, expected \"tailscale\""
            )),
        }
    }
}

impl rusqlite::ToSql for DiscoveryProvider {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.as_str().to_sql()
    }
}

impl FromSql for DiscoveryProvider {
    fn column_result(value: ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = value.as_str()?;
        s.parse().map_err(|e: String| FromSqlError::Other(e.into()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsProvider {
    /// Public-PKI ACME issuance. Goes through the runtime's existing
    /// Coordinator/ACME flow.
    Acme,
    /// Issued by the host's local Tailscale facility. Only legal on a
    /// site ingress whose source is the Tailscale discovery provider.
    Tailscale,
    /// The runtime's internal CA.
    Internal,
    /// No TLS termination. Only plaintext attachments are permitted on
    /// this site ingress.
    None,
}

impl TlsProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acme => "acme",
            Self::Tailscale => "tailscale",
            Self::Internal => "internal",
            Self::None => "none",
        }
    }
}

impl fmt::Display for TlsProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TlsProvider {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "acme" => Ok(Self::Acme),
            "tailscale" => Ok(Self::Tailscale),
            "internal" => Ok(Self::Internal),
            "none" => Ok(Self::None),
            other => Err(format!(
                "invalid TLS provider {other:?}, expected \"acme\", \"tailscale\", \"internal\", or \"none\""
            )),
        }
    }
}

impl rusqlite::ToSql for TlsProvider {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.as_str().to_sql()
    }
}

impl FromSql for TlsProvider {
    fn column_result(value: ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = value.as_str()?;
        s.parse().map_err(|e: String| FromSqlError::Other(e.into()))
    }
}

fn source_columns(source: &SiteIngressSource) -> (&'static str, Option<&str>, Option<&str>) {
    match source {
        SiteIngressSource::Manual => ("manual", None, None),
        SiteIngressSource::Discovered { provider, key } => {
            ("discovered", Some(provider.as_str()), Some(key.as_str()))
        }
    }
}

fn source_from_columns(
    source: &str,
    provider: Option<String>,
    key: Option<String>,
) -> rusqlite::Result<SiteIngressSource> {
    match source {
        "manual" => Ok(SiteIngressSource::Manual),
        "discovered" => {
            let provider = provider
                .ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        "discovered site ingress missing provider".into(),
                    )
                })?
                .parse::<DiscoveryProvider>()
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        e.into(),
                    )
                })?;
            let key = key.ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    "discovered site ingress missing key".into(),
                )
            })?;
            Ok(SiteIngressSource::Discovered { provider, key })
        }
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown site ingress source {other:?}").into(),
        )),
    }
}

// r[impl ingress.site.lifecycle]
pub fn create(db: &Db, def: &SiteIngressDef) -> rusqlite::Result<()> {
    let (source, provider, key) = source_columns(&def.source);
    db.conn.execute(
        "INSERT INTO site_ingresses
             (name, hostname, description, source,
              discovered_provider, discovered_key,
              tls_provider, stale, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            def.name,
            def.hostname,
            def.description,
            source,
            provider,
            key,
            def.tls_provider,
            def.stale as i64,
            def.created_at,
        ],
    )?;
    Ok(())
}

pub fn list(db: &Db) -> rusqlite::Result<Vec<SiteIngressDef>> {
    let mut stmt = db.conn.prepare(
        "SELECT name, hostname, description, source,
                discovered_provider, discovered_key,
                tls_provider, stale, created_at
         FROM site_ingresses
         ORDER BY name",
    )?;
    let rows = stmt.query_map([], row_to_def)?;
    rows.collect()
}

pub fn get(db: &Db, name: &SiteIngressName) -> rusqlite::Result<Option<SiteIngressDef>> {
    db.conn
        .query_row(
            "SELECT name, hostname, description, source,
                    discovered_provider, discovered_key,
                    tls_provider, stale, created_at
             FROM site_ingresses
             WHERE name = ?1",
            params![name],
            row_to_def,
        )
        .optional()
}

pub fn find_discovered(
    db: &Db,
    provider: DiscoveryProvider,
    key: &str,
) -> rusqlite::Result<Option<SiteIngressDef>> {
    db.conn
        .query_row(
            "SELECT name, hostname, description, source,
                    discovered_provider, discovered_key,
                    tls_provider, stale, created_at
             FROM site_ingresses
             WHERE discovered_provider = ?1 AND discovered_key = ?2",
            params![provider, key],
            row_to_def,
        )
        .optional()
}

pub fn update_description(
    db: &Db,
    name: &SiteIngressName,
    description: Option<&str>,
) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "UPDATE site_ingresses SET description = ?2 WHERE name = ?1",
        params![name, description],
    )?;
    Ok(count > 0)
}

pub fn update_tls_provider(
    db: &Db,
    name: &SiteIngressName,
    tls_provider: TlsProvider,
) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "UPDATE site_ingresses SET tls_provider = ?2 WHERE name = ?1",
        params![name, tls_provider],
    )?;
    Ok(count > 0)
}

/// Update the hostname of a discovered site ingress in place. Used by
/// the Tailscale provider when the operator renames their node — the
/// stable provider key is unchanged so attachments stay bound.
pub fn update_hostname_for_discovery(
    db: &Db,
    provider: DiscoveryProvider,
    key: &str,
    hostname: &str,
) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "UPDATE site_ingresses SET hostname = ?3
         WHERE discovered_provider = ?1 AND discovered_key = ?2",
        params![provider, key, hostname],
    )?;
    Ok(count > 0)
}

pub fn set_stale(db: &Db, name: &SiteIngressName, stale: bool) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "UPDATE site_ingresses SET stale = ?2 WHERE name = ?1",
        params![name, stale as i64],
    )?;
    Ok(count > 0)
}

/// Delete a site ingress unconditionally. Callers must enforce the
/// "discovered ingresses cannot be deleted by operators" rule above this
/// layer (typically in the OI handler) — this function exists at the
/// layer that does the actual SQL.
pub fn delete(db: &Db, name: &SiteIngressName) -> rusqlite::Result<bool> {
    // Attachments cascade via FK (PRAGMA foreign_keys is enabled on every
    // connection in Db::open*).
    let count = db
        .conn
        .execute("DELETE FROM site_ingresses WHERE name = ?1", params![name])?;
    Ok(count > 0)
}

fn row_to_def(row: &rusqlite::Row<'_>) -> rusqlite::Result<SiteIngressDef> {
    let name: SiteIngressName = row.get(0)?;
    let hostname: String = row.get(1)?;
    let description: Option<String> = row.get(2)?;
    let source: String = row.get(3)?;
    let discovered_provider: Option<String> = row.get(4)?;
    let discovered_key: Option<String> = row.get(5)?;
    let tls_provider: TlsProvider = row.get(6)?;
    let stale: i64 = row.get(7)?;
    let created_at: String = row.get(8)?;
    let source = source_from_columns(&source, discovered_provider, discovered_key)?;
    Ok(SiteIngressDef {
        name,
        hostname,
        description,
        source,
        tls_provider,
        stale: stale != 0,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mkdb() -> Db {
        Db::open_in_memory().expect("open in-memory db")
    }

    fn mkname(s: &str) -> SiteIngressName {
        SiteIngressName::new(s).expect("valid name")
    }

    fn manual(name: &str, hostname: &str) -> SiteIngressDef {
        SiteIngressDef {
            name: mkname(name),
            hostname: hostname.into(),
            description: None,
            source: SiteIngressSource::Manual,
            tls_provider: TlsProvider::Acme,
            stale: false,
            created_at: "2026-04-28T00:00:00Z".into(),
        }
    }

    fn discovered_tailscale(name: &str, hostname: &str, key: &str) -> SiteIngressDef {
        SiteIngressDef {
            name: mkname(name),
            hostname: hostname.into(),
            description: None,
            source: SiteIngressSource::Discovered {
                provider: DiscoveryProvider::Tailscale,
                key: key.into(),
            },
            tls_provider: TlsProvider::Tailscale,
            stale: false,
            created_at: "2026-04-28T00:00:00Z".into(),
        }
    }

    #[test]
    fn create_manual_round_trips() {
        let db = mkdb();
        let def = manual("legacy-redirect", "old.example.com");
        create(&db, &def).unwrap();
        assert_eq!(get(&db, &def.name).unwrap().as_ref(), Some(&def));
        assert_eq!(list(&db).unwrap(), vec![def]);
    }

    #[test]
    fn create_discovered_round_trips() {
        let db = mkdb();
        let def = discovered_tailscale("tailscale", "host.tailnet.ts.net", "n-abc123");
        create(&db, &def).unwrap();

        let got = get(&db, &def.name).unwrap().expect("row present");
        assert_eq!(got, def);

        let by_key =
            find_discovered(&db, DiscoveryProvider::Tailscale, "n-abc123")
                .unwrap()
                .expect("row present");
        assert_eq!(by_key, def);
    }

    #[test]
    fn discovered_uniqueness_across_providers_is_per_key() {
        let db = mkdb();
        create(
            &db,
            &discovered_tailscale("tailscale", "a.tailnet.ts.net", "n-1"),
        )
        .unwrap();
        // Same provider, same key → unique constraint must reject.
        let res = create(
            &db,
            &discovered_tailscale("tailscale-dup", "a.tailnet.ts.net", "n-1"),
        );
        let err = res.expect_err("UNIQUE must reject duplicate (provider, key)");
        assert!(format!("{err}").to_lowercase().contains("unique"));
    }

    #[test]
    fn manual_rows_with_null_keys_do_not_collide() {
        // Two manual rows have NULL discovered_(provider|key); SQLite treats
        // NULLs as distinct in UNIQUE indices, so both should insert.
        let db = mkdb();
        create(&db, &manual("first", "one.example.com")).unwrap();
        create(&db, &manual("second", "two.example.com")).unwrap();
        assert_eq!(list(&db).unwrap().len(), 2);
    }

    #[test]
    fn update_hostname_for_discovery_renames_in_place() {
        let db = mkdb();
        create(
            &db,
            &discovered_tailscale("tailscale", "old.tailnet.ts.net", "n-1"),
        )
        .unwrap();

        let updated = update_hostname_for_discovery(
            &db,
            DiscoveryProvider::Tailscale,
            "n-1",
            "new.tailnet.ts.net",
        )
        .unwrap();
        assert!(updated);

        let after = get(&db, &mkname("tailscale")).unwrap().unwrap();
        assert_eq!(after.hostname, "new.tailnet.ts.net");
    }

    #[test]
    fn set_stale_round_trips() {
        let db = mkdb();
        let def = discovered_tailscale("tailscale", "h.ts.net", "n-1");
        create(&db, &def).unwrap();
        assert!(!get(&db, &def.name).unwrap().unwrap().stale);

        assert!(set_stale(&db, &def.name, true).unwrap());
        assert!(get(&db, &def.name).unwrap().unwrap().stale);

        assert!(set_stale(&db, &def.name, false).unwrap());
        assert!(!get(&db, &def.name).unwrap().unwrap().stale);
    }

    #[test]
    fn delete_removes_row() {
        let db = mkdb();
        let def = manual("legacy", "old.example.com");
        create(&db, &def).unwrap();
        assert!(delete(&db, &def.name).unwrap());
        assert!(get(&db, &def.name).unwrap().is_none());
        assert!(!delete(&db, &def.name).unwrap());
    }

    #[test]
    fn tls_provider_round_trip() {
        for p in [
            TlsProvider::Acme,
            TlsProvider::Tailscale,
            TlsProvider::Internal,
            TlsProvider::None,
        ] {
            let s = p.as_str();
            assert_eq!(s.parse::<TlsProvider>().unwrap(), p);
        }
    }
}
