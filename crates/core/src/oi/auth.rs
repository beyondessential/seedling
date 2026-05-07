use std::{
    io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::runtime::db::Db;
use crate::transport::auth::TrustedKeys;

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

/// Load all authorized fingerprints from the DB into the in-memory set.
pub fn load_from_db(db: &Db, trusted: &TrustedKeys) -> rusqlite::Result<()> {
    let mut stmt = db.conn.prepare("SELECT fingerprint FROM authorized_keys")?;
    let fps: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut set = trusted.write();
    for fp in fps {
        set.insert(fp);
    }
    Ok(())
}

/// Read `$data_dir/authorized_keys` and import any entries not already in
/// the DB. Lines have the form `<fingerprint> <label>`; `#` and blank lines
/// are ignored.
// i[impl trust.bootstrap]
pub fn import_bootstrap_file(data_dir: &Path, db: &Db, trusted: &TrustedKeys) -> io::Result<()> {
    let path = data_dir.join("authorized_keys");
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&path)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut imported = 0u32;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let fp = match parts.next().filter(|s| !s.is_empty()) {
            Some(f) => f,
            None => continue,
        };
        let label = parts.next().unwrap_or("bootstrap").trim();

        let already: bool = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM authorized_keys WHERE fingerprint = ?1",
                [fp],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0;

        if !already {
            match db.conn.execute(
                "INSERT INTO authorized_keys (fingerprint, label, added_at) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![fp, label, now],
            ) {
                Ok(_) => {
                    trusted.write().insert(fp.to_owned());
                    imported += 1;
                }
                Err(e) => {
                    tracing::warn!(fingerprint = %fp, "failed to persist bootstrap key: {e}");
                }
            }
        }
    }

    if imported > 0 {
        tracing::info!(
            count = imported,
            "imported entries from bootstrap authorized_keys file"
        );
    }
    Ok(())
}

/// Look up the label for a fingerprint. Returns `None` if not found.
pub fn get_label(db: &Db, fingerprint: &str) -> rusqlite::Result<Option<String>> {
    let mut stmt = db
        .conn
        .prepare("SELECT label FROM authorized_keys WHERE fingerprint = ?1")?;
    let mut rows = stmt.query([fingerprint])?;
    rows.next()?.map(|r| r.get(0)).transpose()
}

// i[key.list]
pub fn list_keys(db: &Db) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut stmt = db.conn.prepare(
        "SELECT fingerprint, label, added_at \
         FROM authorized_keys ORDER BY added_at ASC",
    )?;
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect()
}

/// Insert a key, or update its label if it already exists.
// i[key.authorize]
pub fn authorize_key(
    db: &Db,
    trusted: &TrustedKeys,
    fp: &str,
    label: &str,
) -> rusqlite::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    db.conn.execute(
        "INSERT INTO authorized_keys (fingerprint, label, added_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(fingerprint) DO UPDATE SET label = excluded.label",
        rusqlite::params![fp, label, now],
    )?;
    trusted.write().insert(fp.to_owned());
    Ok(())
}

/// Remove a key. Returns `true` if it was present and removed.
// i[key.revoke]
pub fn revoke_key(db: &Db, trusted: &TrustedKeys, fp: &str) -> rusqlite::Result<bool> {
    let rows = db
        .conn
        .execute("DELETE FROM authorized_keys WHERE fingerprint = ?1", [fp])?;
    if rows > 0 {
        trusted.write().remove(fp);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::auth::new_trusted_keys;

    // i[verify key.list]
    #[test]
    fn list_empty_on_fresh_db() {
        let db = Db::open_in_memory().unwrap();
        assert!(list_keys(&db).unwrap().is_empty());
    }

    // i[verify key.authorize]
    // i[verify key.list]
    #[test]
    fn authorize_then_list_returns_inserted_key() {
        let db = Db::open_in_memory().unwrap();
        let trusted = new_trusted_keys();
        authorize_key(&db, &trusted, "fp-1", "laptop").unwrap();
        let list = list_keys(&db).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "fp-1");
        assert_eq!(list[0].1, "laptop");
        assert!(trusted.read().contains("fp-1"));
    }

    // i[verify key.authorize]
    #[test]
    fn authorize_updates_label_on_conflict() {
        let db = Db::open_in_memory().unwrap();
        let trusted = new_trusted_keys();
        authorize_key(&db, &trusted, "fp-1", "old-label").unwrap();
        authorize_key(&db, &trusted, "fp-1", "new-label").unwrap();
        let list = list_keys(&db).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1, "new-label");
    }

    // i[verify key.revoke]
    #[test]
    fn revoke_removes_authorized_key() {
        let db = Db::open_in_memory().unwrap();
        let trusted = new_trusted_keys();
        authorize_key(&db, &trusted, "fp-1", "laptop").unwrap();
        assert!(revoke_key(&db, &trusted, "fp-1").unwrap());
        assert!(list_keys(&db).unwrap().is_empty());
        assert!(!trusted.read().contains("fp-1"));
    }

    // i[verify key.revoke]
    #[test]
    fn revoke_returns_false_for_unknown_fingerprint() {
        let db = Db::open_in_memory().unwrap();
        let trusted = new_trusted_keys();
        assert!(!revoke_key(&db, &trusted, "unknown-fp").unwrap());
    }

    #[test]
    fn get_label_roundtrips() {
        let db = Db::open_in_memory().unwrap();
        let trusted = new_trusted_keys();
        authorize_key(&db, &trusted, "fp-x", "ci-bot").unwrap();
        assert_eq!(get_label(&db, "fp-x").unwrap().as_deref(), Some("ci-bot"));
        assert!(get_label(&db, "fp-other").unwrap().is_none());
    }
}
