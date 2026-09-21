use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use rusqlite::params;
use seedling_protocol::names::AppName;

use crate::runtime::db::Db;

/// The operator-set priority of an app: its standing relative to other apps on
/// a shared host when resources are contended. Distinct from a Deployment's own
/// [`Priority`](crate::defs::enums::Priority): app priority is an operational
/// decision, not a claim the definition makes for itself.
///
/// Ordered `High > Normal > Low`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AppPriority {
    High,
    #[default]
    Normal,
    Low,
}

impl AppPriority {
    /// Lower-case wire form used in `/apps/*` responses, event payloads, and
    /// the durable store.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }
}

impl fmt::Display for AppPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Rejected when a caller supplies a level that is not one of the three the
/// system accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidAppPriority(pub String);

impl fmt::Display for InvalidAppPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid app priority '{}': must be one of high, normal, low",
            self.0
        )
    }
}

impl std::error::Error for InvalidAppPriority {}

impl FromStr for AppPriority {
    type Err = InvalidAppPriority;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "high" => Ok(Self::High),
            "normal" => Ok(Self::Normal),
            "low" => Ok(Self::Low),
            other => Err(InvalidAppPriority(other.to_owned())),
        }
    }
}

// r[impl priority.app]
/// Load the stored app priority for an app. Returns `None` when no decision has
/// been stored, which means the app is at its default (`normal`).
pub fn load_app_priority(db: &Db, app: &AppName) -> rusqlite::Result<Option<AppPriority>> {
    let mut stmt = db
        .conn
        .prepare("SELECT priority FROM app_priorities WHERE app = ?1")?;
    let mut rows = stmt.query(params![app])?;
    match rows.next()? {
        Some(row) => {
            let stored: String = row.get(0)?;
            // A value written by an older or divergent build that no longer
            // parses is treated as the default rather than surfaced as a
            // definite level the operator never chose.
            Ok(Some(stored.parse().unwrap_or_default()))
        }
        None => Ok(None),
    }
}

// r[impl priority.settings]
/// Store the app priority for an app.
pub fn save_app_priority(db: &Db, app: &AppName, priority: AppPriority) -> rusqlite::Result<()> {
    let now = jiff::Timestamp::now().to_string();
    db.conn.execute(
        // r[impl history.persist.partial-update]
        "INSERT INTO app_priorities (app, priority, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(app) DO UPDATE SET
             priority = excluded.priority,
             updated_at = excluded.updated_at",
        params![app, priority.as_str(), now],
    )?;
    Ok(())
}

// i[impl app.priority.reset-on-uninstall]
/// Discard the stored app priority for an app (on uninstall or deregister), so
/// a later reinstall starts again at `normal`.
pub fn delete_app_priority_for_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM app_priorities WHERE app = ?1", params![app])?;
    Ok(())
}

// r[impl priority.app]
/// The effective app priority: the stored decision, or `normal` when none is
/// stored.
pub fn effective_app_priority(db: &Db, app: &AppName) -> rusqlite::Result<AppPriority> {
    Ok(load_app_priority(db, app)?.unwrap_or_default())
}

// r[impl priority.app]
/// Every stored app priority, in one read.
///
/// Callers that need the standing of many apps at once — the reconcile tick and
/// `/apps/list` — use this rather than a query per app. An app absent from the
/// map has no stored decision and stands at `normal`; an error means the
/// standings could not be read at all, which is not the same thing.
pub fn load_all_app_priorities(db: &Db) -> rusqlite::Result<HashMap<AppName, AppPriority>> {
    let mut stmt = db
        .conn
        .prepare("SELECT app, priority FROM app_priorities")?;
    let rows = stmt.query_map([], |row| {
        let app: AppName = row.get(0)?;
        let stored: String = row.get(1)?;
        Ok((app, stored.parse().unwrap_or_default()))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (app, priority) = row?;
        out.insert(app, priority);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> AppName {
        AppName::new("myapp").unwrap()
    }

    // r[verify priority.app]
    #[test]
    fn load_returns_none_before_save() {
        let db = Db::open_in_memory().unwrap();
        assert!(load_app_priority(&db, &app()).unwrap().is_none());
    }

    // r[verify priority.app]
    #[test]
    fn effective_is_normal_without_a_decision() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(
            effective_app_priority(&db, &app()).unwrap(),
            AppPriority::Normal
        );
    }

    // r[verify priority.settings]
    #[test]
    fn save_then_load_round_trips() {
        let db = Db::open_in_memory().unwrap();
        save_app_priority(&db, &app(), AppPriority::High).unwrap();
        assert_eq!(
            load_app_priority(&db, &app()).unwrap(),
            Some(AppPriority::High)
        );
    }

    // r[verify priority.settings]
    #[test]
    fn save_overwrites_previous_decision() {
        let db = Db::open_in_memory().unwrap();
        save_app_priority(&db, &app(), AppPriority::High).unwrap();
        save_app_priority(&db, &app(), AppPriority::Low).unwrap();
        assert_eq!(
            load_app_priority(&db, &app()).unwrap(),
            Some(AppPriority::Low)
        );
    }

    // i[verify app.priority.reset-on-uninstall]
    #[test]
    fn delete_removes_the_decision() {
        let db = Db::open_in_memory().unwrap();
        save_app_priority(&db, &app(), AppPriority::High).unwrap();
        delete_app_priority_for_app(&db, &app()).unwrap();
        assert!(load_app_priority(&db, &app()).unwrap().is_none());
    }

    // r[verify priority.app]
    #[test]
    fn bulk_read_returns_every_stored_decision_and_omits_the_rest() {
        let db = Db::open_in_memory().unwrap();
        let other = AppName::new("other").unwrap();
        save_app_priority(&db, &app(), AppPriority::High).unwrap();
        save_app_priority(&db, &other, AppPriority::Low).unwrap();

        let all = load_all_app_priorities(&db).unwrap();
        assert_eq!(all.get(&app()), Some(&AppPriority::High));
        assert_eq!(all.get(&other), Some(&AppPriority::Low));
        // An app with no stored decision is absent rather than present as
        // `normal`: the caller supplies the default, so "never set" and
        // "set to normal" stay distinguishable here.
        assert!(!all.contains_key(&AppName::new("unset").unwrap()));
    }

    #[test]
    fn parse_rejects_unknown_level() {
        assert!("urgent".parse::<AppPriority>().is_err());
        assert_eq!("high".parse::<AppPriority>().unwrap(), AppPriority::High);
    }
}
