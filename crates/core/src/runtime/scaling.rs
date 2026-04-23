use rusqlite::params;
use seedling_protocol::names::AppName;

use crate::runtime::db::Db;

// r[impl scaling.decision]
/// Load the stored scaling decision for a specific deployment.
/// Returns `None` if no decision has been stored.
pub fn load_scaling_decision(
    db: &Db,
    app: &AppName,
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
    app: &AppName,
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
pub fn delete_scaling_decisions_for_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
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
    app: &AppName,
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
    app: &AppName,
    deployment: &str,
    low: u16,
    high: u16,
) -> rusqlite::Result<u16> {
    match load_scaling_decision(db, app, deployment)? {
        Some(stored) => Ok(stored.clamp(low, high)),
        None => Ok(low),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn app() -> AppName {
        AppName::new("myapp").unwrap()
    }

    // r[verify scaling.decision]
    #[test]
    fn load_returns_none_before_save() {
        let db = Db::open_in_memory().unwrap();
        assert!(load_scaling_decision(&db, &app(), "web").unwrap().is_none());
    }

    // r[verify scaling.decision]
    #[test]
    fn save_then_load_round_trips() {
        let db = Db::open_in_memory().unwrap();
        save_scaling_decision(&db, &app(), "web", 5).unwrap();
        assert_eq!(
            load_scaling_decision(&db, &app(), "web").unwrap(),
            Some(5),
        );
    }

    // r[verify scaling.decision]
    #[test]
    fn save_overwrites_previous_decision() {
        let db = Db::open_in_memory().unwrap();
        save_scaling_decision(&db, &app(), "web", 3).unwrap();
        save_scaling_decision(&db, &app(), "web", 7).unwrap();
        assert_eq!(
            load_scaling_decision(&db, &app(), "web").unwrap(),
            Some(7),
        );
    }

    // r[verify scaling.decision]
    #[test]
    fn effective_scale_uses_lower_bound_when_no_decision() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(effective_scale(&db, &app(), "web", 2, 10).unwrap(), 2);
    }

    // r[verify scaling.decision]
    #[test]
    fn effective_scale_clamps_stored_decision_to_bounds() {
        let db = Db::open_in_memory().unwrap();
        save_scaling_decision(&db, &app(), "web", 15).unwrap();
        assert_eq!(effective_scale(&db, &app(), "web", 2, 10).unwrap(), 10);
        save_scaling_decision(&db, &app(), "web", 1).unwrap();
        assert_eq!(effective_scale(&db, &app(), "web", 2, 10).unwrap(), 2);
    }

    // r[verify scaling.decision]
    #[test]
    fn delete_scaling_decisions_for_app_removes_all() {
        let db = Db::open_in_memory().unwrap();
        save_scaling_decision(&db, &app(), "web", 5).unwrap();
        save_scaling_decision(&db, &app(), "api", 3).unwrap();
        delete_scaling_decisions_for_app(&db, &app()).unwrap();
        assert!(load_scaling_decision(&db, &app(), "web").unwrap().is_none());
        assert!(load_scaling_decision(&db, &app(), "api").unwrap().is_none());
    }

    // r[verify scaling.clamp]
    #[test]
    fn clamp_raises_value_below_new_lower_bound() {
        let db = Db::open_in_memory().unwrap();
        save_scaling_decision(&db, &app(), "web", 1).unwrap();
        let mut bounds = BTreeMap::new();
        bounds.insert("web".to_owned(), (3u16, 10u16));
        clamp_scaling_decisions(&db, &app(), &bounds).unwrap();
        assert_eq!(load_scaling_decision(&db, &app(), "web").unwrap(), Some(3));
    }

    // r[verify scaling.clamp]
    #[test]
    fn clamp_lowers_value_above_new_upper_bound() {
        let db = Db::open_in_memory().unwrap();
        save_scaling_decision(&db, &app(), "web", 20).unwrap();
        let mut bounds = BTreeMap::new();
        bounds.insert("web".to_owned(), (1u16, 5u16));
        clamp_scaling_decisions(&db, &app(), &bounds).unwrap();
        assert_eq!(load_scaling_decision(&db, &app(), "web").unwrap(), Some(5));
    }

    // r[verify scaling.clamp]
    #[test]
    fn clamp_leaves_in_range_values_alone() {
        let db = Db::open_in_memory().unwrap();
        save_scaling_decision(&db, &app(), "web", 4).unwrap();
        let mut bounds = BTreeMap::new();
        bounds.insert("web".to_owned(), (1u16, 10u16));
        clamp_scaling_decisions(&db, &app(), &bounds).unwrap();
        assert_eq!(load_scaling_decision(&db, &app(), "web").unwrap(), Some(4));
    }

    // r[verify scaling.clamp]
    #[test]
    fn clamp_removes_decision_for_deleted_deployment() {
        let db = Db::open_in_memory().unwrap();
        save_scaling_decision(&db, &app(), "ghost", 4).unwrap();
        save_scaling_decision(&db, &app(), "web", 2).unwrap();
        let mut bounds = BTreeMap::new();
        bounds.insert("web".to_owned(), (1u16, 5u16));
        clamp_scaling_decisions(&db, &app(), &bounds).unwrap();
        assert!(load_scaling_decision(&db, &app(), "ghost").unwrap().is_none());
        assert_eq!(load_scaling_decision(&db, &app(), "web").unwrap(), Some(2));
    }
}
