use rusqlite::{Connection, Result as SqlResult};

pub const SQL: &str = include_str!("v14.sql");

pub fn run(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(SQL)?;

    let mut sel = conn.prepare("SELECT name, script FROM registered_apps")?;
    let apps: Vec<(String, String)> = sel
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    drop(sel);

    let now = jiff::Timestamp::now().to_string();
    for (name, script) in apps {
        let vid = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO app_versions (id, app, script, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![vid, name, script, now],
        )?;
        conn.execute(
            "UPDATE registered_apps SET current_version_id = ?1 WHERE name = ?2",
            rusqlite::params![vid, name],
        )?;
    }

    Ok(())
}
