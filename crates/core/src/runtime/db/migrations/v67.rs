use rusqlite::{Connection, Result as SqlResult, params};

use crate::runtime::tls::parse;

pub const SQL: &str = include_str!("v67.sql");

/// Create the SAN index and fill it from every stored certificate.
///
/// The rows are written with SQL of this migration's own rather than through
/// the runtime's indexing code, which follows the schema as it is now and not
/// as it was at this version.
///
/// A certificate whose PEM will not parse is indexed as covering nothing. It
/// could not be served before this migration either, and refusing to migrate
/// would stop the daemon starting over one unreadable row.
// r[impl tls.cert.serve]
pub fn run(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(SQL)?;

    let certificates: Vec<(i64, String)> = {
        let mut stmt =
            conn.prepare("SELECT id, cert_pem FROM tls_certificates WHERE cert_pem IS NOT NULL")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<SqlResult<_>>()?
    };

    for (id, pem) in certificates {
        let facts = match parse::leaf_facts(&pem) {
            Ok(facts) => Some(facts),
            Err(e) => {
                tracing::warn!(
                    cert_id = id,
                    error = %e,
                    "stored certificate could not be parsed; indexing it as covering nothing"
                );
                None
            }
        };
        conn.execute(
            "INSERT INTO tls_cert_index (cert_id, self_issued) VALUES (?1, ?2)",
            params![id, facts.as_ref().map(|f| f.self_issued)],
        )?;
        for san in facts.iter().flat_map(|f| &f.san_dns_names) {
            conn.execute(
                "INSERT INTO tls_cert_sans (cert_id, san) VALUES (?1, ?2)
                 ON CONFLICT DO NOTHING",
                params![id, san.to_ascii_lowercase()],
            )?;
        }
    }

    Ok(())
}
