use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct FaultRecord {
    pub id: String,
    pub app: String,
    pub resource_type: Option<String>,
    pub resource_name: Option<String>,
    pub instance_id: Option<String>,
    pub kind: String,
    pub timestamp: DateTime<Utc>,
    pub description: String,
}

// i[fault.record]
pub fn file_fault(
    db: &crate::runtime::db::Db,
    app: &str,
    resource_type: Option<&str>,
    resource_name: Option<&str>,
    instance_id: Option<&str>,
    kind: &str,
    description: &str,
) -> rusqlite::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now: DateTime<Utc> = std::time::SystemTime::now().into();
    let timestamp = now.to_rfc3339();
    db.conn.execute(
        "INSERT INTO faults (id, app, resource_type, resource_name, instance_id, kind, timestamp, description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![id, app, resource_type, resource_name, instance_id, kind, timestamp, description],
    )?;
    Ok(id)
}

pub fn clear_fault(db: &crate::runtime::db::Db, fault_id: &str) -> rusqlite::Result<()> {
    let now: DateTime<Utc> = std::time::SystemTime::now().into();
    db.conn.execute(
        "UPDATE faults SET cleared_at = ?1 WHERE id = ?2",
        rusqlite::params![now.to_rfc3339(), fault_id],
    )?;
    Ok(())
}

// i[fault.list]
pub fn list_active_faults(
    db: &crate::runtime::db::Db,
    app: Option<&str>,
) -> rusqlite::Result<Vec<FaultRecord>> {
    let mut records = Vec::new();
    match app {
        Some(app_name) => {
            let mut stmt = db.conn.prepare(
                "SELECT id, app, resource_type, resource_name, instance_id, kind, timestamp, description
                 FROM faults WHERE cleared_at IS NULL AND app = ?1
                 ORDER BY timestamp",
            )?;
            let rows = stmt.query_map([app_name], row_to_record)?;
            for row in rows {
                records.push(row?);
            }
        }
        None => {
            let mut stmt = db.conn.prepare(
                "SELECT id, app, resource_type, resource_name, instance_id, kind, timestamp, description
                 FROM faults WHERE cleared_at IS NULL
                 ORDER BY timestamp",
            )?;
            let rows = stmt.query_map([], row_to_record)?;
            for row in rows {
                records.push(row?);
            }
        }
    }
    Ok(records)
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<FaultRecord> {
    let ts_str: String = row.get(6)?;
    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| std::time::SystemTime::now().into());
    Ok(FaultRecord {
        id: row.get(0)?,
        app: row.get(1)?,
        resource_type: row.get(2)?,
        resource_name: row.get(3)?,
        instance_id: row.get(4)?,
        kind: row.get(5)?,
        timestamp,
        description: row.get(7)?,
    })
}

pub fn clear_faults_by_kind(
    db: &crate::runtime::db::Db,
    app: &str,
    kind: &str,
) -> rusqlite::Result<u64> {
    let now: DateTime<Utc> = std::time::SystemTime::now().into();
    let count = db.conn.execute(
        "UPDATE faults SET cleared_at = ?1 WHERE app = ?2 AND kind = ?3 AND cleared_at IS NULL",
        rusqlite::params![now.to_rfc3339(), app, kind],
    )?;
    Ok(count as u64)
}

pub fn clear_all_faults_for_app(db: &crate::runtime::db::Db, app: &str) -> rusqlite::Result<()> {
    let now: DateTime<Utc> = std::time::SystemTime::now().into();
    db.conn.execute(
        "UPDATE faults SET cleared_at = ?1 WHERE app = ?2 AND cleared_at IS NULL",
        rusqlite::params![now.to_rfc3339(), app],
    )?;
    Ok(())
}

pub fn has_active_faults(db: &crate::runtime::db::Db, app: &str) -> rusqlite::Result<bool> {
    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM faults WHERE app = ?1 AND cleared_at IS NULL",
        [app],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

pub fn count_active_faults(db: &crate::runtime::db::Db) -> rusqlite::Result<i64> {
    db.conn.query_row(
        "SELECT COUNT(*) FROM faults WHERE cleared_at IS NULL",
        [],
        |r| r.get(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::db::Db;

    // i[verify fault.record]
    #[test]
    fn file_and_list_fault() {
        let db = Db::open_in_memory().expect("open");
        let id = file_fault(
            &db,
            "myapp",
            None,
            None,
            None,
            "script_error",
            "parse failed",
        )
        .expect("file_fault");
        assert!(!id.is_empty());

        let faults = list_active_faults(&db, Some("myapp")).expect("list");
        assert_eq!(faults.len(), 1);
        assert_eq!(faults[0].id, id);
        assert_eq!(faults[0].app, "myapp");
        assert_eq!(faults[0].kind, "script_error");
        assert_eq!(faults[0].description, "parse failed");
        assert!(faults[0].resource_type.is_none());
    }

    // i[verify fault.record]
    #[test]
    fn file_fault_with_resource_fields() {
        let db = Db::open_in_memory().expect("open");
        let id = file_fault(
            &db,
            "myapp",
            Some("deployment"),
            Some("web"),
            Some("abcd1234"),
            "crash_loop",
            "container keeps restarting",
        )
        .expect("file_fault");

        let faults = list_active_faults(&db, Some("myapp")).expect("list");
        assert_eq!(faults.len(), 1);
        assert_eq!(faults[0].id, id);
        assert_eq!(faults[0].resource_type.as_deref(), Some("deployment"));
        assert_eq!(faults[0].resource_name.as_deref(), Some("web"));
        assert_eq!(faults[0].instance_id.as_deref(), Some("abcd1234"));
    }

    // i[verify fault.derived]
    #[test]
    fn clear_fault_sets_cleared_at() {
        let db = Db::open_in_memory().expect("open");
        let id =
            file_fault(&db, "myapp", None, None, None, "script_error", "err").expect("file_fault");

        clear_fault(&db, &id).expect("clear");

        let active = list_active_faults(&db, Some("myapp")).expect("list");
        assert!(active.is_empty());
    }

    // i[verify fault.derived]
    #[test]
    fn clear_faults_by_kind_clears_matching() {
        let db = Db::open_in_memory().expect("open");
        file_fault(&db, "myapp", None, None, None, "script_error", "err1").expect("file1");
        file_fault(&db, "myapp", None, None, None, "script_error", "err2").expect("file2");
        file_fault(
            &db,
            "myapp",
            Some("deployment"),
            Some("web"),
            None,
            "crash_loop",
            "boom",
        )
        .expect("file3");

        let cleared = clear_faults_by_kind(&db, "myapp", "script_error").expect("clear");
        assert_eq!(cleared, 2);

        let remaining = list_active_faults(&db, Some("myapp")).expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].kind, "crash_loop");
    }

    // i[verify fault.list]
    #[test]
    fn list_active_faults_filters_by_app() {
        let db = Db::open_in_memory().expect("open");
        file_fault(&db, "app-a", None, None, None, "script_error", "a err").expect("file a");
        file_fault(&db, "app-b", None, None, None, "script_error", "b err").expect("file b");

        let a_faults = list_active_faults(&db, Some("app-a")).expect("list a");
        assert_eq!(a_faults.len(), 1);
        assert_eq!(a_faults[0].app, "app-a");

        let all_faults = list_active_faults(&db, None).expect("list all");
        assert_eq!(all_faults.len(), 2);
    }

    // i[verify fault.list]
    #[test]
    fn list_active_faults_excludes_cleared() {
        let db = Db::open_in_memory().expect("open");
        let id =
            file_fault(&db, "myapp", None, None, None, "script_error", "err").expect("file_fault");
        file_fault(&db, "myapp", None, None, None, "other", "still active").expect("file2");

        clear_fault(&db, &id).expect("clear");

        let faults = list_active_faults(&db, None).expect("list");
        assert_eq!(faults.len(), 1);
        assert_eq!(faults[0].kind, "other");
    }

    #[test]
    fn clear_all_faults_for_app_clears_only_that_app() {
        let db = Db::open_in_memory().expect("open");
        file_fault(&db, "app-a", None, None, None, "script_error", "a err").expect("a");
        file_fault(
            &db,
            "app-a",
            Some("deployment"),
            Some("web"),
            None,
            "crash",
            "a crash",
        )
        .expect("a2");
        file_fault(&db, "app-b", None, None, None, "script_error", "b err").expect("b");

        clear_all_faults_for_app(&db, "app-a").expect("clear");

        let a = list_active_faults(&db, Some("app-a")).expect("list a");
        assert!(a.is_empty());

        let b = list_active_faults(&db, Some("app-b")).expect("list b");
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn has_active_faults_reflects_state() {
        let db = Db::open_in_memory().expect("open");
        assert!(!has_active_faults(&db, "myapp").expect("check"));

        let id = file_fault(&db, "myapp", None, None, None, "script_error", "err").expect("file");
        assert!(has_active_faults(&db, "myapp").expect("check"));

        clear_fault(&db, &id).expect("clear");
        assert!(!has_active_faults(&db, "myapp").expect("check"));
    }

    #[test]
    fn count_active_faults_counts_all_apps() {
        let db = Db::open_in_memory().expect("open");
        assert_eq!(count_active_faults(&db).expect("count"), 0);

        file_fault(&db, "app-a", None, None, None, "err", "a").expect("a");
        file_fault(&db, "app-b", None, None, None, "err", "b").expect("b");
        assert_eq!(count_active_faults(&db).expect("count"), 2);

        clear_all_faults_for_app(&db, "app-a").expect("clear");
        assert_eq!(count_active_faults(&db).expect("count"), 1);
    }
}
