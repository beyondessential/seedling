use rusqlite::params;
use seedling_protocol::names::AppName;

use crate::runtime::db::Db;

// i[impl backup.app.register]
/// Opt `app` in to the backup role. The BSL script for `app` must already
/// define the `save-snapshot`, `list-snapshots`, and `restore-snapshot`
/// actions — that validation lives on the OI handler side.
pub fn register(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("INSERT INTO backup_apps (app) VALUES (?1)", params![app])?;
    Ok(())
}

// i[impl backup.app.deregister]
pub fn deregister(db: &Db, app: &AppName) -> rusqlite::Result<bool> {
    let count = db
        .conn
        .execute("DELETE FROM backup_apps WHERE app = ?1", params![app])?;
    Ok(count > 0)
}

/// Returns `true` if `app` is currently registered as a backup app.
pub fn is_registered(db: &Db, app: &AppName) -> rusqlite::Result<bool> {
    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM backup_apps WHERE app = ?1",
        params![app],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

// i[impl backup.app.list]
pub fn list_all(db: &Db) -> rusqlite::Result<Vec<AppName>> {
    let mut stmt = db
        .conn
        .prepare("SELECT app FROM backup_apps ORDER BY app")?;
    let rows = stmt.query_map([], |row| row.get::<_, AppName>(0))?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(s: &str) -> AppName {
        AppName::new(s).unwrap()
    }

    // i[verify backup.app.register]
    // i[verify backup.app.list]
    #[test]
    fn register_then_list_contains_app() {
        let db = Db::open_in_memory().unwrap();
        register(&db, &app("backup-kopia")).unwrap();
        assert_eq!(list_all(&db).unwrap(), vec![app("backup-kopia")]);
    }

    // i[verify backup.app.list]
    #[test]
    fn list_returns_apps_ordered_by_name() {
        let db = Db::open_in_memory().unwrap();
        register(&db, &app("zebra-bk")).unwrap();
        register(&db, &app("alpha-bk")).unwrap();
        register(&db, &app("mu-bk")).unwrap();
        let names: Vec<_> = list_all(&db)
            .unwrap()
            .iter()
            .map(|a| a.to_string())
            .collect();
        assert_eq!(names, vec!["alpha-bk", "mu-bk", "zebra-bk"]);
    }

    // i[verify backup.app.register]
    #[test]
    fn register_duplicate_name_errors() {
        let db = Db::open_in_memory().unwrap();
        register(&db, &app("backup-kopia")).unwrap();
        assert!(register(&db, &app("backup-kopia")).is_err());
    }

    // i[verify backup.app.deregister]
    #[test]
    fn deregister_returns_true_when_present() {
        let db = Db::open_in_memory().unwrap();
        register(&db, &app("backup-kopia")).unwrap();
        assert!(deregister(&db, &app("backup-kopia")).unwrap());
        assert!(list_all(&db).unwrap().is_empty());
    }

    // i[verify backup.app.deregister]
    #[test]
    fn deregister_returns_false_when_absent() {
        let db = Db::open_in_memory().unwrap();
        assert!(!deregister(&db, &app("ghost-bk")).unwrap());
    }

    #[test]
    fn is_registered_reflects_state() {
        let db = Db::open_in_memory().unwrap();
        assert!(!is_registered(&db, &app("backup-kopia")).unwrap());
        register(&db, &app("backup-kopia")).unwrap();
        assert!(is_registered(&db, &app("backup-kopia")).unwrap());
        deregister(&db, &app("backup-kopia")).unwrap();
        assert!(!is_registered(&db, &app("backup-kopia")).unwrap());
    }
}
