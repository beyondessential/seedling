use rusqlite::params;

use crate::runtime::db::Db;

// r[impl scaling.decision]
/// Load the stored scaling decision for a specific deployment.
/// Returns `None` if no decision has been stored.
pub fn load_scaling_decision(
    db: &Db,
    app: &str,
    deployment: &str,
) -> rusqlite::Result<Option<u16>> {
    let mut stmt = db
        .conn
        .prepare("SELECT scale FROM scaling_decisions WHERE app = ?1 AND deployment = ?2")?;
    let mut rows = stmt.query(params![app, deployment])?;
    match rows.next()? {
        Some(row) => {
            let scale: i64 = row.get(0)?;
            Ok(Some(scale as u16))
        }
        None => Ok(None),
    }
}

// r[impl scaling.decision]
/// Store a scaling decision for a deployment.
pub fn save_scaling_decision(
    db: &Db,
    app: &str,
    deployment: &str,
    scale: u16,
) -> rusqlite::Result<()> {
    let now = jiff::Timestamp::now().to_string();
    db.conn.execute(
        "INSERT OR REPLACE INTO scaling_decisions (app, deployment, scale, updated_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![app, deployment, scale as i64, now],
    )?;
    Ok(())
}

/// Delete all scaling decisions for an app (e.g. on deregister).
pub fn delete_scaling_decisions_for_app(db: &Db, app: &str) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM scaling_decisions WHERE app = ?1", params![app])?;
    Ok(())
}

// r[impl scaling.clamp]
/// Clamp all stored scaling decisions for an app to the bounds defined
/// in the provided deployments map. Removes decisions for deployments
/// that no longer exist.
///
/// `deployments` maps deployment name -> (low, high) bounds.
pub fn clamp_scaling_decisions(
    db: &Db,
    app: &str,
    deployments: &std::collections::BTreeMap<String, (u16, u16)>,
) -> rusqlite::Result<()> {
    let mut stmt = db
        .conn
        .prepare("SELECT deployment, scale FROM scaling_decisions WHERE app = ?1")?;
    let rows: Vec<(String, i64)> = stmt
        .query_map(params![app], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);

    for (deployment, scale) in rows {
        match deployments.get(&deployment) {
            None => {
                db.conn.execute(
                    "DELETE FROM scaling_decisions WHERE app = ?1 AND deployment = ?2",
                    params![app, deployment],
                )?;
            }
            Some(&(low, high)) => {
                let clamped = (scale as u16).clamp(low, high);
                if clamped != scale as u16 {
                    save_scaling_decision(db, app, &deployment, clamped)?;
                }
            }
        }
    }
    Ok(())
}

// r[impl scaling.decision]
/// Compute the effective scale for a deployment: the stored decision
/// clamped to bounds, or the lower bound if no decision exists.
pub fn effective_scale(
    db: &Db,
    app: &str,
    deployment: &str,
    low: u16,
    high: u16,
) -> rusqlite::Result<u16> {
    match load_scaling_decision(db, app, deployment)? {
        Some(stored) => Ok(stored.clamp(low, high)),
        None => Ok(low),
    }
}
