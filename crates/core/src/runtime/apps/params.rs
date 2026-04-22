use std::collections::BTreeMap;

use seedling_protocol::names::{AppName, ParamName};

use super::AppEntry;
use crate::runtime::db::Db;

/// Read only the plaintext `params` table. Prefer `load_all_params_for_app`
/// in any reload path — secret params live in a separate table and this
/// function silently ignores them.
// i[param.store]
pub(super) fn load_params_for_app(
    db: &Db,
    app_name: &AppName,
) -> rusqlite::Result<BTreeMap<String, String>> {
    let mut stmt = db
        .conn
        .prepare("SELECT param_name, value FROM params WHERE app_name = ?1 ORDER BY param_name")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([app_name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().collect())
}

// i[param.store]
// i[param.set]
pub fn upsert_param(
    db: &Db,
    app_name: &AppName,
    param_name: &ParamName,
    value: &str,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR REPLACE INTO params (app_name, param_name, value) VALUES (?1, ?2, ?3)",
        rusqlite::params![app_name, param_name, value],
    )?;
    Ok(())
}

pub fn delete_app_params(db: &Db, app_name: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM params WHERE app_name = ?1", [app_name])?;
    Ok(())
}

// i[param.unset]
pub fn delete_one_param(
    db: &Db,
    app_name: &AppName,
    param_name: &ParamName,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM params WHERE app_name = ?1 AND param_name = ?2",
        rusqlite::params![app_name, param_name],
    )?;
    Ok(())
}

/// Synchronize the in-memory script_error state with the faults DB table.
/// Call after register/reload to persist fault changes.
pub fn sync_script_error_fault(db: &Db, entry: &AppEntry) {
    let existing: Vec<_> = crate::runtime::faults::list_active_faults(db, Some(&entry.name))
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.kind == "script_error")
        .collect();

    match &entry.script_error {
        Some((msg, _)) => {
            let dominated = existing.iter().any(|f| f.description == *msg);
            if !dominated {
                for f in &existing {
                    if let Err(e) = crate::runtime::faults::clear_fault(db, &f.id, &entry.name) {
                        tracing::warn!(app = %entry.name, fault_id = %f.id, "failed to clear stale script-error fault: {e}");
                    }
                }
                if let Err(e) = crate::runtime::faults::file_fault(
                    db,
                    &entry.name,
                    None,
                    None,
                    None,
                    "script_error",
                    msg,
                ) {
                    tracing::warn!(app = %entry.name, "failed to file script-error fault: {e}");
                }
            }
        }
        None => {
            for f in &existing {
                if let Err(e) = crate::runtime::faults::clear_fault(db, &f.id, &entry.name) {
                    tracing::warn!(app = %entry.name, fault_id = %f.id, "failed to clear script-error fault: {e}");
                }
            }
        }
    }
}
