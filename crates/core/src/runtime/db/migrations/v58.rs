use rusqlite::{Connection, Result as SqlResult};

use crate::runtime::definition::{Bundle, Source};

pub const SQL: &str = include_str!("v58.sql");

/// Every stored script becomes a one-file bundle, hashed in the bundle's
/// canonical form, and every generation and template is repointed at it.
/// No provenance was recorded before, so each installed definition is
/// recorded as pushed by nobody known.
pub fn run(conn: &Connection) -> SqlResult<()> {
    let (schema_sql, cleanup_sql) = SQL
        .split_once("-- [backfill]\n")
        .expect("v58.sql must contain the '-- [backfill]' marker");

    conn.execute_batch(schema_sql)?;

    let unknown = Source::unknown_push().to_db();

    let bodies: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT hash, body FROM script_bodies")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<SqlResult<_>>()?
    };
    for (old_hash, body) in bodies {
        let bundle = Bundle::from_stored_script(&body);
        insert_bundle(conn, &bundle)?;
        conn.execute(
            "UPDATE generations SET bundle_hash = ?1 WHERE bundle_hash = ?2",
            rusqlite::params![bundle.hash(), old_hash],
        )?;
    }
    conn.execute(
        "UPDATE generations SET provenance = ?1 WHERE kind IN ('register', 'script_update')",
        [&unknown],
    )?;

    let templates: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT name, body FROM templates")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<SqlResult<_>>()?
    };
    for (name, body) in templates {
        let bundle = Bundle::from_stored_script(&body);
        insert_bundle(conn, &bundle)?;
        conn.execute(
            "UPDATE templates SET bundle_hash = ?1, provenance = ?2 WHERE name = ?3",
            rusqlite::params![bundle.hash(), unknown, name],
        )?;
    }

    conn.execute_batch(cleanup_sql)?;
    Ok(())
}

fn insert_bundle(conn: &Connection, bundle: &Bundle) -> SqlResult<()> {
    conn.execute(
        "INSERT INTO definition_bundles (hash, contents) VALUES (?1, ?2)
         ON CONFLICT(hash) DO NOTHING",
        rusqlite::params![bundle.hash(), bundle.canonical_bytes()],
    )?;
    Ok(())
}
