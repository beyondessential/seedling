//! Durable Canopy settings and the identity Canopy knows this instance by.

use rusqlite::params;

use crate::runtime::db::Db;

/// Canopy access settings as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanopySettings {
    pub enabled: bool,
    /// What Canopy answered when asked which server the offering client's
    /// identity is enrolled as. `None` until first resolved.
    pub server_id: Option<String>,
    pub updated_at: i64,
    /// The last moment an offer was live, in unix seconds. `None` if none ever has been.
    pub last_offer_live_at: Option<i64>,
    /// When access was last turned on from off, in unix seconds. `None` if it never has been.
    pub enabled_at: Option<i64>,
}

/// An app that declares a Canopy application type, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanopyApp {
    pub app: String,
    pub canopy_type: String,
    /// When the app came to carry this type, in unix seconds.
    pub since: i64,
    /// The `health/app` result graded before a lifecycle operation began.
    pub last_grade: Option<String>,
}

pub(super) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// spec: CNR#canopy-access
pub fn get_settings(db: &Db) -> rusqlite::Result<CanopySettings> {
    db.conn.query_row(
        "SELECT enabled, server_id, updated_at, last_offer_live_at, enabled_at \
         FROM canopy_settings WHERE singleton = 1",
        [],
        |row| {
            // A stored empty string normalises to None so callers need not tell
            // "never resolved" from "resolved to nothing".
            let server_id: Option<String> = row.get(1)?;
            Ok(CanopySettings {
                enabled: row.get::<_, i64>(0)? != 0,
                server_id: server_id.filter(|s| !s.is_empty()),
                updated_at: row.get(2)?,
                last_offer_live_at: row.get(3)?,
                enabled_at: row.get(4)?,
            })
        },
    )
}

// spec: CNR#canopy-access
pub fn set_enabled(db: &Db, enabled: bool) -> rusqlite::Result<()> {
    set_enabled_at(db, enabled, now_secs())
}

// spec: CNR#missing-offers
/// [`set_enabled`] at a given time. `enabled_at` is stamped only when access
/// goes from off to on, so enabling an already-enabled instance does not
/// restart the missing-offer period.
pub fn set_enabled_at(db: &Db, enabled: bool, at: i64) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE canopy_settings SET \
            enabled_at = CASE WHEN enabled = 0 AND ?1 = 1 THEN ?2 ELSE enabled_at END, \
            enabled = ?1, updated_at = ?2 \
         WHERE singleton = 1",
        params![i64::from(enabled), at],
    )?;
    Ok(())
}

// spec: CNR#missing-offers
/// Record that an offer was live at `at`.
pub fn mark_offer_live(db: &Db, at: i64) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE canopy_settings SET last_offer_live_at = ?1 WHERE singleton = 1",
        params![at],
    )?;
    Ok(())
}

// spec: CNR#applications
/// Record the Canopy type an app's definition declares, or that it declares
/// none. `since` is stamped only when the row is new or the type changes.
pub fn sync_app_type(db: &Db, app: &str, canopy_type: Option<&str>) -> rusqlite::Result<()> {
    sync_app_type_at(db, app, canopy_type, now_secs())
}

/// [`sync_app_type`] at a given time.
pub fn sync_app_type_at(
    db: &Db,
    app: &str,
    canopy_type: Option<&str>,
    at: i64,
) -> rusqlite::Result<()> {
    match canopy_type {
        Some(canopy_type) => {
            db.conn.execute(
                "INSERT INTO canopy_apps (app, type, since) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(app) DO UPDATE SET \
                    since = CASE WHEN type = excluded.type THEN since ELSE excluded.since END, \
                    last_grade = CASE WHEN type = excluded.type THEN last_grade ELSE NULL END, \
                    type = excluded.type",
                params![app, canopy_type, at],
            )?;
        }
        None => delete_app(db, app)?,
    }
    Ok(())
}

/// Forget an app's Canopy type, as when it is deregistered.
pub fn delete_app(db: &Db, app: &str) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM canopy_apps WHERE app = ?1", params![app])?;
    Ok(())
}

// spec: CNR#application-health
/// Hold `grade` against `app` for an operation that follows, or `None` to
/// drop the one held.
pub fn set_last_grade(db: &Db, app: &str, grade: Option<&str>) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE canopy_apps SET last_grade = ?2 WHERE app = ?1",
        params![app, grade],
    )?;
    Ok(())
}

/// Every app that declares a Canopy type, by app name.
pub fn list_apps(db: &Db) -> rusqlite::Result<Vec<CanopyApp>> {
    let mut stmt = db
        .conn
        .prepare("SELECT app, type, since, last_grade FROM canopy_apps ORDER BY app")?;
    stmt.query_map([], |row| {
        Ok(CanopyApp {
            app: row.get(0)?,
            canopy_type: row.get(1)?,
            since: row.get(2)?,
            last_grade: row.get(3)?,
        })
    })?
    .collect()
}

// spec: CNR#server-identity
/// Cache the server identifier Canopy resolved, so the resolution is not
/// repeated on every report.
///
/// Deliberately leaves `updated_at` alone. That field is part of the settings
/// surface and reports when an operator last changed the setting; the runtime
/// refreshing its own cache is not such a change, and stamping it here would
/// have the interface report a configuration change that never happened.
pub fn set_server_id(db: &Db, server_id: Option<&str>) -> rusqlite::Result<()> {
    let stored = server_id.map(str::trim).filter(|s| !s.is_empty());
    db.conn.execute(
        "UPDATE canopy_settings SET server_id = ?1 WHERE singleton = 1",
        params![stored],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::db::Db;

    fn db() -> Db {
        Db::open_in_memory().expect("in-memory db")
    }

    // spec: CNR#canopy-access
    #[test]
    fn a_fresh_instance_has_canopy_access_on() {
        let settings = get_settings(&db()).expect("the singleton row exists");
        assert!(
            settings.enabled,
            "an unconfigured instance defaults to enabled"
        );
        assert!(settings.server_id.is_none());
    }

    // spec: CNR#canopy-access
    #[test]
    fn the_setting_survives_being_turned_off_and_on() {
        let db = db();
        set_enabled(&db, false).unwrap();
        assert!(!get_settings(&db).unwrap().enabled);
        set_enabled(&db, true).unwrap();
        assert!(get_settings(&db).unwrap().enabled);
    }

    // spec: CNR#canopy-access
    #[test]
    fn changing_the_setting_stamps_when_it_changed() {
        let db = db();
        assert_eq!(get_settings(&db).unwrap().updated_at, 0);
        set_enabled(&db, false).unwrap();
        assert!(get_settings(&db).unwrap().updated_at > 0);
    }

    // spec: CNR#server-identity
    #[test]
    fn a_resolved_server_id_round_trips_and_can_be_cleared() {
        let db = db();
        set_server_id(&db, Some("srv-123")).unwrap();
        assert_eq!(
            get_settings(&db).unwrap().server_id.as_deref(),
            Some("srv-123")
        );

        set_server_id(&db, None).unwrap();
        assert!(
            get_settings(&db).unwrap().server_id.is_none(),
            "clearing lets the next report re-resolve"
        );
    }

    // spec: CNR#server-identity
    #[test]
    fn a_blank_server_id_reads_as_unresolved() {
        let db = db();
        set_server_id(&db, Some("   ")).unwrap();
        assert!(
            get_settings(&db).unwrap().server_id.is_none(),
            "whitespace is not an identity"
        );
    }

    // spec: CNR#server-identity
    #[test]
    fn caching_a_server_id_is_not_a_settings_change() {
        // `updated_at` says when an operator last changed the setting. The
        // runtime refreshing its own cache must not look like one.
        let db = db();
        set_enabled(&db, false).unwrap();
        let after_change = get_settings(&db).unwrap().updated_at;
        assert!(after_change > 0);

        set_server_id(&db, Some("srv-123")).unwrap();
        let settings = get_settings(&db).unwrap();
        assert_eq!(settings.server_id.as_deref(), Some("srv-123"));
        assert_eq!(
            settings.updated_at, after_change,
            "the settings surface must not report a change that did not happen"
        );
    }

    // spec: CNR#canopy-access
    #[test]
    fn the_server_id_and_the_setting_do_not_disturb_each_other() {
        let db = db();
        set_server_id(&db, Some("srv-1")).unwrap();
        set_enabled(&db, false).unwrap();
        let settings = get_settings(&db).unwrap();
        assert!(!settings.enabled);
        assert_eq!(
            settings.server_id.as_deref(),
            Some("srv-1"),
            "turning Canopy off does not forget which server this is"
        );
    }

    // spec: CNR#applications
    #[test]
    fn since_survives_reapplying_the_same_type_and_a_change_restamps_it() {
        let db = db();
        sync_app_type_at(&db, "central", Some("tamanu-central"), 100).unwrap();
        set_last_grade(&db, "central", Some("passed")).unwrap();
        sync_app_type_at(&db, "central", Some("tamanu-central"), 200).unwrap();
        let apps = list_apps(&db).unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].since, 100);
        assert_eq!(apps[0].last_grade.as_deref(), Some("passed"));

        sync_app_type_at(&db, "central", Some("tamanu-facility"), 300).unwrap();
        let apps = list_apps(&db).unwrap();
        assert_eq!(apps[0].canopy_type, "tamanu-facility");
        assert_eq!(apps[0].since, 300);
        assert_eq!(
            apps[0].last_grade, None,
            "a grade of one application is not the grade of another"
        );
    }

    // spec: CNR#applications
    #[test]
    fn no_type_removes_the_row() {
        let db = db();
        sync_app_type_at(&db, "a", Some("x"), 1).unwrap();
        sync_app_type_at(&db, "b", Some("x"), 1).unwrap();
        sync_app_type_at(&db, "a", None, 2).unwrap();
        let apps = list_apps(&db).unwrap();
        assert_eq!(
            apps.iter().map(|a| a.app.as_str()).collect::<Vec<_>>(),
            ["b"]
        );
        sync_app_type_at(&db, "never-there", None, 2).unwrap();
    }

    // spec: CNR#missing-offers
    #[test]
    fn enabled_at_is_stamped_only_when_access_goes_from_off_to_on() {
        let db = db();
        assert_eq!(get_settings(&db).unwrap().enabled_at, None);
        set_enabled_at(&db, true, 100).unwrap();
        assert_eq!(
            get_settings(&db).unwrap().enabled_at,
            None,
            "enabling an already-enabled instance is not a transition"
        );
        set_enabled_at(&db, false, 200).unwrap();
        assert_eq!(get_settings(&db).unwrap().enabled_at, None);
        set_enabled_at(&db, true, 300).unwrap();
        assert_eq!(get_settings(&db).unwrap().enabled_at, Some(300));
        set_enabled_at(&db, true, 400).unwrap();
        assert_eq!(get_settings(&db).unwrap().enabled_at, Some(300));
    }

    // spec: CNR#missing-offers
    #[test]
    fn the_last_live_offer_is_recorded() {
        let db = db();
        assert_eq!(get_settings(&db).unwrap().last_offer_live_at, None);
        mark_offer_live(&db, 500).unwrap();
        assert_eq!(get_settings(&db).unwrap().last_offer_live_at, Some(500));
    }
}
