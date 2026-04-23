use rusqlite::params;
use seedling_protocol::names::{AppName, BackupStrategyName};

use crate::runtime::db::Db;

pub const VALID_SCHEDULES: &[&str] = &["every hour", "twice a day", "every day"];

#[derive(Debug, Clone)]
pub struct BackupStrategy {
    pub name: BackupStrategyName,
    pub via: AppName,
    pub schedule: String,
    pub volumes: Vec<String>,
    pub last_fired_at: Option<String>,
}

// i[impl backup.strategy.create]
pub fn create(db: &Db, strategy: &BackupStrategy) -> rusqlite::Result<()> {
    let volumes_json =
        serde_json::to_string(&strategy.volumes).expect("Vec<String> always serialises");
    db.conn.execute(
        "INSERT INTO backup_strategies (name, via, schedule, volumes) VALUES (?1, ?2, ?3, ?4)",
        params![strategy.name, strategy.via, strategy.schedule, volumes_json],
    )?;
    Ok(())
}

pub fn get(db: &Db, name: &BackupStrategyName) -> rusqlite::Result<Option<BackupStrategy>> {
    let mut stmt = db.conn.prepare(
        "SELECT name, via, schedule, volumes, last_fired_at FROM backup_strategies WHERE name = ?1",
    )?;
    let mut rows = stmt.query_map(params![name], row_to_strategy)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

// i[impl backup.strategy.list]
pub fn list_all(db: &Db) -> rusqlite::Result<Vec<BackupStrategy>> {
    let mut stmt = db.conn.prepare(
        "SELECT name, via, schedule, volumes, last_fired_at FROM backup_strategies ORDER BY name",
    )?;
    let rows = stmt.query_map([], row_to_strategy)?;
    rows.collect()
}

// i[impl backup.strategy.update]
pub fn update(
    db: &Db,
    name: &BackupStrategyName,
    via: Option<&AppName>,
    schedule: Option<&str>,
    volumes: Option<&[String]>,
) -> rusqlite::Result<bool> {
    let strategy = match get(db, name)? {
        Some(s) => s,
        None => return Ok(false),
    };
    let new_via = via.unwrap_or(&strategy.via);
    let new_schedule = schedule.unwrap_or(&strategy.schedule);
    let new_volumes = volumes.unwrap_or(&strategy.volumes);
    let volumes_json = serde_json::to_string(new_volumes).expect("Vec<String> always serialises");
    let count = db.conn.execute(
        "UPDATE backup_strategies SET via = ?2, schedule = ?3, volumes = ?4 WHERE name = ?1",
        params![name, new_via, new_schedule, volumes_json],
    )?;
    Ok(count > 0)
}

// i[impl backup.strategy.delete]
pub fn delete(db: &Db, name: &BackupStrategyName) -> rusqlite::Result<bool> {
    let count = db.conn.execute(
        "DELETE FROM backup_strategies WHERE name = ?1",
        params![name],
    )?;
    Ok(count > 0)
}

pub fn references_backup_app(db: &Db, backup_app_name: &AppName) -> rusqlite::Result<bool> {
    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM backup_strategies WHERE via = ?1",
        params![backup_app_name],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

// r[impl backup.execution]
pub fn update_last_fired_at(
    db: &Db,
    name: &BackupStrategyName,
    fired_at: &str,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE backup_strategies SET last_fired_at = ?2 WHERE name = ?1",
        params![name, fired_at],
    )?;
    Ok(())
}

fn row_to_strategy(row: &rusqlite::Row<'_>) -> rusqlite::Result<BackupStrategy> {
    let name: BackupStrategyName = row.get(0)?;
    let via: AppName = row.get(1)?;
    let schedule: String = row.get(2)?;
    let volumes_json: String = row.get(3)?;
    let volumes: Vec<String> = serde_json::from_str(&volumes_json).unwrap_or_default();
    let last_fired_at: Option<String> = row.get(4)?;
    Ok(BackupStrategy {
        name,
        via,
        schedule,
        volumes,
        last_fired_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strategy(name: &str, via: &str, schedule: &str, volumes: &[&str]) -> BackupStrategy {
        BackupStrategy {
            name: BackupStrategyName::new(name).unwrap(),
            via: AppName::new(via).unwrap(),
            schedule: schedule.to_owned(),
            volumes: volumes.iter().map(|s| (*s).to_owned()).collect(),
            last_fired_at: None,
        }
    }

    // i[verify backup.strategy.create]
    #[test]
    fn create_persists_strategy() {
        let db = Db::open_in_memory().unwrap();
        create(
            &db,
            &strategy("nightly", "backup-kopia-s3", "every day", &["myapp/data"]),
        )
        .unwrap();
        let got = get(&db, &BackupStrategyName::new("nightly").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(got.via, "backup-kopia-s3");
        assert_eq!(got.schedule, "every day");
        assert_eq!(got.volumes, vec!["myapp/data".to_owned()]);
        assert!(got.last_fired_at.is_none());
    }

    // i[verify backup.strategy.create]
    #[test]
    fn create_duplicate_name_errors() {
        let db = Db::open_in_memory().unwrap();
        create(&db, &strategy("dup", "via", "every hour", &["a"])).unwrap();
        assert!(create(&db, &strategy("dup", "via", "every hour", &["b"])).is_err());
    }

    // i[verify backup.strategy.list]
    #[test]
    fn list_all_returns_ordered_by_name() {
        let db = Db::open_in_memory().unwrap();
        create(&db, &strategy("zeta", "via", "every day", &["a"])).unwrap();
        create(&db, &strategy("alpha", "via", "every hour", &["b"])).unwrap();
        let names: Vec<_> = list_all(&db).unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }

    // i[verify backup.strategy.update]
    #[test]
    fn update_changes_schedule_and_volumes() {
        let db = Db::open_in_memory().unwrap();
        create(&db, &strategy("updatable", "old-app", "every hour", &["v1"])).unwrap();
        let new_via = AppName::new("new-app").unwrap();
        assert!(update(
            &db,
            &BackupStrategyName::new("updatable").unwrap(),
            Some(&new_via),
            Some("every day"),
            Some(&["v1".to_owned(), "v2".to_owned()]),
        )
        .unwrap());
        let got = get(&db, &BackupStrategyName::new("updatable").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(got.via, "new-app");
        assert_eq!(got.schedule, "every day");
        assert_eq!(got.volumes, vec!["v1".to_owned(), "v2".to_owned()]);
    }

    // i[verify backup.strategy.update]
    #[test]
    fn update_absent_returns_false() {
        let db = Db::open_in_memory().unwrap();
        assert!(!update(
            &db,
            &BackupStrategyName::new("ghost").unwrap(),
            None,
            Some("every day"),
            None,
        )
        .unwrap());
    }

    // i[verify backup.strategy.delete]
    #[test]
    fn delete_returns_true_when_present() {
        let db = Db::open_in_memory().unwrap();
        create(&db, &strategy("gone", "via", "every day", &["a"])).unwrap();
        assert!(delete(&db, &BackupStrategyName::new("gone").unwrap()).unwrap());
        assert!(get(&db, &BackupStrategyName::new("gone").unwrap())
            .unwrap()
            .is_none());
    }

    // i[verify backup.strategy.delete]
    #[test]
    fn delete_absent_returns_false() {
        let db = Db::open_in_memory().unwrap();
        assert!(!delete(&db, &BackupStrategyName::new("ghost").unwrap()).unwrap());
    }
}
