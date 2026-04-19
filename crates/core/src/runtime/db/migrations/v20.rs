use rusqlite::{Connection, Result as SqlResult};
use sha2::{Digest, Sha256};

pub const SQL: &str = include_str!("v20.sql");

pub fn run(conn: &Connection) -> SqlResult<()> {
    let (schema_sql, cleanup_sql) = SQL
        .split_once("-- [backfill]\n")
        .expect("v20.sql must contain the '-- [backfill]' marker");

    conn.execute_batch(schema_sql)?;

    fn hex_of(digest: &[u8]) -> String {
        use std::fmt::Write as FmtWrite;
        let mut s = String::with_capacity(digest.len() * 2);
        for b in digest {
            write!(s, "{b:02x}").expect("write to String is infallible");
        }
        s
    }

    let app_rows: Vec<(String, Option<String>)> = {
        let mut stmt = conn.prepare("SELECT name, current_version_id FROM registered_apps")?;
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };

    let now = jiff::Timestamp::now().to_string();
    for (app, version_id) in app_rows {
        let Some(vid) = version_id else {
            continue;
        };
        let script: String = match conn.query_row(
            "SELECT script FROM app_versions WHERE id = ?1",
            [&vid],
            |row| row.get(0),
        ) {
            Ok(s) => s,
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(e) => return Err(e),
        };

        let digest = Sha256::digest(script.as_bytes());
        let hash = hex_of(&digest);
        conn.execute(
            "INSERT OR IGNORE INTO script_bodies (hash, body) VALUES (?1, ?2)",
            rusqlite::params![hash, script],
        )?;
        conn.execute(
            "INSERT INTO generations (app, generation, created_at, kind, script_hash)
             VALUES (?1, 1, ?2, 'register', ?3)",
            rusqlite::params![app, now, hash],
        )?;

        let params: Vec<(String, String)> = {
            let mut pstmt = conn.prepare(
                "SELECT param_name, value FROM params WHERE app_name = ?1 ORDER BY param_name",
            )?;
            pstmt
                .query_map([&app], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?
        };

        let mut current = 1u64;
        for (param_name, value) in params {
            current += 1;
            conn.execute(
                "INSERT INTO generations
                    (app, generation, created_at, kind, param_name,
                     previous_value, new_value, script_hash)
                 VALUES (?1, ?2, ?3, 'param_set', ?4, NULL, ?5, ?6)",
                rusqlite::params![app, current as i64, now, param_name, value, hash],
            )?;
        }

        conn.execute(
            "UPDATE registered_apps SET current_generation = ?1 WHERE name = ?2",
            rusqlite::params![current as i64, app],
        )?;
    }

    conn.execute_batch(cleanup_sql)?;

    Ok(())
}
