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
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// r[impl canopy.settings.enabled]
pub fn get_settings(db: &Db) -> rusqlite::Result<CanopySettings> {
    db.conn.query_row(
        "SELECT enabled, server_id, updated_at FROM canopy_settings WHERE singleton = 1",
        [],
        |row| {
            // A stored empty string normalises to None so callers need not tell
            // "never resolved" from "resolved to nothing".
            let server_id: Option<String> = row.get(1)?;
            Ok(CanopySettings {
                enabled: row.get::<_, i64>(0)? != 0,
                server_id: server_id.filter(|s| !s.is_empty()),
                updated_at: row.get(2)?,
            })
        },
    )
}

// r[impl canopy.settings.enabled]
pub fn set_enabled(db: &Db, enabled: bool) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE canopy_settings SET enabled = ?1, updated_at = ?2 WHERE singleton = 1",
        params![i64::from(enabled), now_secs()],
    )?;
    Ok(())
}

// r[impl canopy.report.identity]
/// Cache the server identifier Canopy resolved, so the resolution is not
/// repeated on every report.
pub fn set_server_id(db: &Db, server_id: Option<&str>) -> rusqlite::Result<()> {
    let stored = server_id.map(str::trim).filter(|s| !s.is_empty());
    db.conn.execute(
        "UPDATE canopy_settings SET server_id = ?1, updated_at = ?2 WHERE singleton = 1",
        params![stored, now_secs()],
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

    // r[verify canopy.settings.enabled]
    #[test]
    fn a_fresh_instance_has_canopy_access_on() {
        let settings = get_settings(&db()).expect("the singleton row exists");
        assert!(
            settings.enabled,
            "an unconfigured instance defaults to enabled"
        );
        assert!(settings.server_id.is_none());
    }

    // r[verify canopy.settings.enabled]
    #[test]
    fn the_setting_survives_being_turned_off_and_on() {
        let db = db();
        set_enabled(&db, false).unwrap();
        assert!(!get_settings(&db).unwrap().enabled);
        set_enabled(&db, true).unwrap();
        assert!(get_settings(&db).unwrap().enabled);
    }

    // r[verify canopy.settings.enabled]
    #[test]
    fn changing_the_setting_stamps_when_it_changed() {
        let db = db();
        assert_eq!(get_settings(&db).unwrap().updated_at, 0);
        set_enabled(&db, false).unwrap();
        assert!(get_settings(&db).unwrap().updated_at > 0);
    }

    // r[verify canopy.report.identity]
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

    // r[verify canopy.report.identity]
    #[test]
    fn a_blank_server_id_reads_as_unresolved() {
        let db = db();
        set_server_id(&db, Some("   ")).unwrap();
        assert!(
            get_settings(&db).unwrap().server_id.is_none(),
            "whitespace is not an identity"
        );
    }

    // r[verify canopy.settings.enabled]
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
}
