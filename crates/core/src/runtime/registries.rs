use crate::runtime::db::Db;

pub fn list_allowed_registries(db: &Db) -> rusqlite::Result<Vec<String>> {
    let mut stmt = db
        .conn
        .prepare("SELECT registry FROM allowed_registries ORDER BY registry")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    rows.collect()
}

pub fn add_allowed_registry(db: &Db, registry: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR IGNORE INTO allowed_registries (registry) VALUES (?1)",
        [registry],
    )?;
    Ok(())
}

pub fn remove_allowed_registry(db: &Db, registry: &str) -> rusqlite::Result<bool> {
    let changed = db.conn.execute(
        "DELETE FROM allowed_registries WHERE registry = ?1",
        [registry],
    )?;
    Ok(changed > 0)
}

pub fn is_registry_allowed(db: &Db, registry: &str) -> rusqlite::Result<bool> {
    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM allowed_registries WHERE registry = ?1",
        [registry],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Migration v11 seeds `docker.io` and `ghcr.io` into the allowlist as
    /// defaults; tests use a registry name not in that default set to
    /// observe add/remove side effects in isolation.
    const TEST_REG: &str = "registry.example.invalid";

    // i[verify registry.list]
    #[test]
    fn list_returns_default_seeded_registries() {
        let db = Db::open_in_memory().unwrap();
        let list = list_allowed_registries(&db).unwrap();
        assert!(list.contains(&"docker.io".to_owned()));
        assert!(list.contains(&"ghcr.io".to_owned()));
    }

    // i[verify registry.add]
    // i[verify registry.list]
    #[test]
    fn add_then_list_includes_added_registry() {
        let db = Db::open_in_memory().unwrap();
        add_allowed_registry(&db, TEST_REG).unwrap();
        let list = list_allowed_registries(&db).unwrap();
        assert!(list.contains(&TEST_REG.to_owned()));
        // Ordering is ascending; verify the overall shape holds with the new entry.
        let mut sorted = list.clone();
        sorted.sort();
        assert_eq!(list, sorted);
    }

    // i[verify registry.add]
    #[test]
    fn add_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        let before = list_allowed_registries(&db).unwrap().len();
        add_allowed_registry(&db, TEST_REG).unwrap();
        add_allowed_registry(&db, TEST_REG).unwrap();
        let after = list_allowed_registries(&db).unwrap().len();
        assert_eq!(after, before + 1, "duplicate add should not double-insert");
    }

    // i[verify registry.remove]
    #[test]
    fn remove_returns_true_when_present() {
        let db = Db::open_in_memory().unwrap();
        add_allowed_registry(&db, TEST_REG).unwrap();
        assert!(remove_allowed_registry(&db, TEST_REG).unwrap());
        assert!(
            !list_allowed_registries(&db)
                .unwrap()
                .contains(&TEST_REG.to_owned())
        );
    }

    // i[verify registry.remove]
    #[test]
    fn remove_returns_false_when_absent() {
        let db = Db::open_in_memory().unwrap();
        assert!(!remove_allowed_registry(&db, TEST_REG).unwrap());
    }

    #[test]
    fn is_registry_allowed_reflects_state() {
        let db = Db::open_in_memory().unwrap();
        assert!(!is_registry_allowed(&db, TEST_REG).unwrap());
        add_allowed_registry(&db, TEST_REG).unwrap();
        assert!(is_registry_allowed(&db, TEST_REG).unwrap());
        remove_allowed_registry(&db, TEST_REG).unwrap();
        assert!(!is_registry_allowed(&db, TEST_REG).unwrap());
    }
}
