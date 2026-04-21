use rusqlite::params;
use seedling_protocol::names::AppName;

use crate::runtime::db::Db;

// i[impl deployment.restart]
/// Load the stored restart generation for a deployment. Returns 0 if none has been stored.
pub fn load_restart_gen(db: &Db, app: &AppName, deployment: &str) -> rusqlite::Result<u64> {
    let mut stmt = db
        .conn
        .prepare("SELECT generation FROM restart_generations WHERE app = ?1 AND deployment = ?2")?;
    let mut rows = stmt.query(params![app, deployment])?;
    match rows.next()? {
        Some(row) => {
            let n: i64 = row.get(0)?;
            Ok(n as u64)
        }
        None => Ok(0),
    }
}

// i[impl deployment.restart]
/// Increment the restart generation for a deployment and return the new value.
pub fn bump_restart_gen(db: &Db, app: &AppName, deployment: &str) -> rusqlite::Result<u64> {
    let now = jiff::Timestamp::now().to_string();
    db.conn.execute(
        "INSERT INTO restart_generations (app, deployment, generation, updated_at)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT (app, deployment) DO UPDATE SET
             generation = generation + 1,
             updated_at = excluded.updated_at",
        params![app, deployment, now],
    )?;
    load_restart_gen(db, app, deployment)
}

/// Delete all restart generations for an app (e.g. on deregister or uninstall).
pub fn delete_restart_gens_for_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM restart_generations WHERE app = ?1",
        params![app],
    )?;
    Ok(())
}
