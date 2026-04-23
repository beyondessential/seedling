use rusqlite::params;
use seedling_protocol::names::AppName;

use crate::runtime::db::Db;

/// A single row in `image_pins`.
#[derive(Debug, Clone)]
pub struct ImagePin {
    pub app: AppName,
    pub reference: String,
    pub pinned_at: i64,
    /// Unix-millisecond expiration, or `None` for an indefinite pin. When
    /// set and in the past, [`drop_expired_pins`] removes the pin.
    // r[impl image.pin.expiry]
    pub expires_at: Option<i64>,
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// r[impl image.pin]
pub fn upsert_pin(db: &Db, app: &AppName, reference: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO image_pins (app, reference, pinned_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(app, reference) DO NOTHING",
        params![app, reference, now_ms()],
    )?;
    Ok(())
}

// r[impl image.pin]
pub fn clear_pin(db: &Db, app: &AppName, reference: &str) -> rusqlite::Result<bool> {
    let n = db.conn.execute(
        "DELETE FROM image_pins WHERE app = ?1 AND reference = ?2",
        params![app, reference],
    )?;
    Ok(n > 0)
}

// r[impl image.pin]
pub fn clear_pins_for_app(db: &Db, app: &AppName) -> rusqlite::Result<usize> {
    db.conn
        .execute("DELETE FROM image_pins WHERE app = ?1", params![app])
}

/// Clear every pin whose reference matches. Used when an operator removes an
/// image by reference and when the reconciler observes a running container
/// using the reference.
// r[impl image.pin]
pub fn clear_pins_by_reference(db: &Db, reference: &str) -> rusqlite::Result<usize> {
    db.conn.execute(
        "DELETE FROM image_pins WHERE reference = ?1",
        params![reference],
    )
}

/// Delete any pin whose `expires_at` has passed. Called once per reconcile tick.
// r[impl image.pin.expiry]
pub fn drop_expired_pins(db: &Db) -> rusqlite::Result<usize> {
    db.conn.execute(
        "DELETE FROM image_pins WHERE expires_at IS NOT NULL AND expires_at <= ?1",
        params![now_ms()],
    )
}

/// Apply the post-update pin reconciliation rule for `app`:
///
/// - Pins whose reference is in `safe_references` have their `expires_at`
///   cleared (the reference is still relevant).
/// - Pins whose reference is NOT in `safe_references`:
///   - If `probe_clean`: deleted immediately.
///   - Else: `expires_at` is set to `now + expiry_window_ms`, but only if
///     there isn't already an earlier expiration set.
///
/// Returns the number of pins deleted (only non-zero when `probe_clean`).
// r[impl image.pin.update-reconcile]
pub fn reconcile_pins_after_update(
    db: &Db,
    app: &AppName,
    safe_references: &[&str],
    probe_clean: bool,
    expiry_window_ms: i64,
) -> rusqlite::Result<usize> {
    let tx = db.conn.unchecked_transaction()?;

    // Clear expiry on pins that are in the safe set — they've been
    // re-validated by either the static AppDef or a clean probe output.
    if !safe_references.is_empty() {
        let placeholders = safe_references
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE image_pins SET expires_at = NULL
             WHERE app = ?1 AND reference IN ({placeholders})"
        );
        let mut stmt = tx.prepare(&sql)?;
        let mut bind: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + safe_references.len());
        bind.push(app);
        for r in safe_references {
            bind.push(r);
        }
        stmt.execute(bind.as_slice())?;
    }

    let deleted = if probe_clean {
        // Authoritative safe set → drop orphans outright.
        if safe_references.is_empty() {
            tx.execute("DELETE FROM image_pins WHERE app = ?1", params![app])?
        } else {
            let placeholders = safe_references
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "DELETE FROM image_pins
                 WHERE app = ?1 AND reference NOT IN ({placeholders})"
            );
            let mut stmt = tx.prepare(&sql)?;
            let mut bind: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + safe_references.len());
            bind.push(app);
            for r in safe_references {
                bind.push(r);
            }
            stmt.execute(bind.as_slice())?
        }
    } else {
        // Partial safe set → stamp an expiry on not-yet-expiring pins
        // whose reference isn't confirmed. An earlier expiration wins to
        // avoid repeated probes pushing the deadline out.
        let target_expiry = now_ms() + expiry_window_ms;
        if safe_references.is_empty() {
            tx.execute(
                "UPDATE image_pins SET expires_at = ?2
                 WHERE app = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
                params![app, target_expiry],
            )?;
        } else {
            let placeholders = safe_references
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "UPDATE image_pins SET expires_at = ?1
                 WHERE app = ?2 AND reference NOT IN ({placeholders})
                   AND (expires_at IS NULL OR expires_at > ?1)"
            );
            let mut stmt = tx.prepare(&sql)?;
            let mut bind: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(2 + safe_references.len());
            bind.push(&target_expiry);
            bind.push(app);
            for r in safe_references {
                bind.push(r);
            }
            stmt.execute(bind.as_slice())?;
        }
        0
    };

    tx.commit()?;
    Ok(deleted)
}

pub fn list_pins(db: &Db, app: Option<&AppName>) -> rusqlite::Result<Vec<ImagePin>> {
    match app {
        Some(a) => {
            let mut stmt = db.conn.prepare(
                "SELECT app, reference, pinned_at, expires_at FROM image_pins
                 WHERE app = ?1 ORDER BY reference",
            )?;
            collect_pins(stmt.query_map(params![a], parse_pin_row)?)
        }
        None => {
            let mut stmt = db.conn.prepare(
                "SELECT app, reference, pinned_at, expires_at FROM image_pins
                 ORDER BY app, reference",
            )?;
            collect_pins(stmt.query_map([], parse_pin_row)?)
        }
    }
}

pub fn list_pinned_apps_for_references(
    db: &Db,
    references: &[&str],
) -> rusqlite::Result<std::collections::HashMap<String, Vec<AppName>>> {
    let mut out: std::collections::HashMap<String, Vec<AppName>> = std::collections::HashMap::new();
    if references.is_empty() {
        return Ok(out);
    }
    let placeholders = references.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT app, reference FROM image_pins WHERE reference IN ({placeholders})");
    let mut stmt = db.conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> = references
        .iter()
        .map(|r| r as &dyn rusqlite::ToSql)
        .collect();
    let rows = stmt.query_map(params.as_slice(), |row| {
        Ok((row.get::<_, AppName>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (app, reference) = row?;
        out.entry(reference).or_default().push(app);
    }
    Ok(out)
}

fn parse_pin_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ImagePin> {
    Ok(ImagePin {
        app: row.get::<_, AppName>(0)?,
        reference: row.get(1)?,
        pinned_at: row.get(2)?,
        expires_at: row.get(3)?,
    })
}

fn collect_pins(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<ImagePin>>,
) -> rusqlite::Result<Vec<ImagePin>> {
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Single row in `image_tracking`.
#[derive(Debug, Clone)]
pub struct ImageTrackingRow {
    pub image_id: String,
    pub first_seen_at: i64,
    pub last_used_at: i64,
}

/// Observe that `image_id` is locally present. Inserts the tracking row if
/// missing. Does not update `last_used_at` for existing rows — use
/// `mark_used` for that.
// r[impl image.track]
pub fn note_present(db: &Db, image_id: &str) -> rusqlite::Result<()> {
    let now = now_ms();
    db.conn.execute(
        "INSERT INTO image_tracking (image_id, first_seen_at, last_used_at)
         VALUES (?1, ?2, ?2)
         ON CONFLICT(image_id) DO NOTHING",
        params![image_id, now],
    )?;
    Ok(())
}

// r[impl image.track]
pub fn mark_used(db: &Db, image_id: &str) -> rusqlite::Result<()> {
    let now = now_ms();
    db.conn.execute(
        "INSERT INTO image_tracking (image_id, first_seen_at, last_used_at)
         VALUES (?1, ?2, ?2)
         ON CONFLICT(image_id) DO UPDATE SET last_used_at = excluded.last_used_at",
        params![image_id, now],
    )?;
    Ok(())
}

pub fn get_tracking(db: &Db, image_id: &str) -> rusqlite::Result<Option<ImageTrackingRow>> {
    db.conn
        .query_row(
            "SELECT image_id, first_seen_at, last_used_at
             FROM image_tracking WHERE image_id = ?1",
            params![image_id],
            |row| {
                Ok(ImageTrackingRow {
                    image_id: row.get(0)?,
                    first_seen_at: row.get(1)?,
                    last_used_at: row.get(2)?,
                })
            },
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
}

/// Drop tracking rows whose `image_id` is not in `live`, so that the table
/// reflects what's actually in local storage after each reconcile pass.
pub fn prune_tracking_except(db: &Db, live: &[String]) -> rusqlite::Result<usize> {
    if live.is_empty() {
        return db.conn.execute("DELETE FROM image_tracking", []);
    }
    let placeholders = live.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("DELETE FROM image_tracking WHERE image_id NOT IN ({placeholders})");
    let mut stmt = db.conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> =
        live.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    stmt.execute(params.as_slice())
}

/// Return image IDs whose `last_used_at` is older than `older_than_ms` and
/// have no pin resolving through `resolve_id` in the given live reference
/// map, caller filters further by active-use.
pub fn gc_candidates(db: &Db, older_than_ms: i64) -> rusqlite::Result<Vec<ImageTrackingRow>> {
    let cutoff = now_ms() - older_than_ms;
    let mut stmt = db.conn.prepare(
        "SELECT image_id, first_seen_at, last_used_at
         FROM image_tracking WHERE last_used_at < ?1",
    )?;
    let rows = stmt.query_map(params![cutoff], |row| {
        Ok(ImageTrackingRow {
            image_id: row.get(0)?,
            first_seen_at: row.get(1)?,
            last_used_at: row.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Remove tracking row for an image that no longer exists locally.
pub fn drop_tracking(db: &Db, image_id: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM image_tracking WHERE image_id = ?1",
        params![image_id],
    )?;
    Ok(())
}

/// Refresh `image_references` from an authoritative `(reference, image_id)` list.
/// Rows not present in `refs` are deleted; present rows are inserted or updated.
// r[impl image.track]
pub fn refresh_references(db: &Db, refs: &[(String, String)]) -> rusqlite::Result<()> {
    let now = now_ms();
    let tx = db.conn.unchecked_transaction()?;
    tx.execute("DELETE FROM image_references", [])?;
    for (reference, image_id) in refs {
        tx.execute(
            "INSERT INTO image_references (reference, image_id, observed_at)
             VALUES (?1, ?2, ?3)",
            params![reference, image_id, now],
        )?;
    }
    tx.commit()
}

/// Resolve a reference to its currently-observed image_id, or `None` if the
/// reference is not present locally.
pub fn lookup_reference(db: &Db, reference: &str) -> rusqlite::Result<Option<String>> {
    db.conn
        .query_row(
            "SELECT image_id FROM image_references WHERE reference = ?1",
            params![reference],
            |row| row.get::<_, String>(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
}

/// Return true if any reference in `refs` is currently present locally.
pub fn reference_present(db: &Db, reference: &str) -> rusqlite::Result<bool> {
    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM image_references WHERE reference = ?1",
        params![reference],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// All references currently known to resolve to a given image_id.
pub fn references_for_image(db: &Db, image_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = db
        .conn
        .prepare("SELECT reference FROM image_references WHERE image_id = ?1")?;
    let rows = stmt.query_map(params![image_id], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use seedling_protocol::names::AppName;

    fn app(s: &str) -> AppName {
        AppName::new(s).unwrap()
    }

    // r[verify image.pin]
    // i[verify image.pin.list]
    // i[verify image.pin.clear]
    #[test]
    fn pin_upsert_and_clear() {
        let db = Db::open_in_memory().unwrap();
        upsert_pin(&db, &app("foo"), "ghcr.io/x:1").unwrap();
        upsert_pin(&db, &app("foo"), "ghcr.io/x:1").unwrap();
        let pins = list_pins(&db, Some(&app("foo"))).unwrap();
        assert_eq!(pins.len(), 1);

        let cleared = clear_pin(&db, &app("foo"), "ghcr.io/x:1").unwrap();
        assert!(cleared);

        let cleared_again = clear_pin(&db, &app("foo"), "ghcr.io/x:1").unwrap();
        assert!(!cleared_again);
    }

    // r[verify image.pin]
    #[test]
    fn pin_clear_by_reference() {
        let db = Db::open_in_memory().unwrap();
        upsert_pin(&db, &app("one"), "img:1").unwrap();
        upsert_pin(&db, &app("two"), "img:1").unwrap();
        upsert_pin(&db, &app("two"), "img:2").unwrap();
        let n = clear_pins_by_reference(&db, "img:1").unwrap();
        assert_eq!(n, 2);
        assert_eq!(list_pins(&db, None).unwrap().len(), 1);
    }

    // r[verify image.track]
    #[test]
    fn track_note_then_mark_used() {
        let db = Db::open_in_memory().unwrap();
        note_present(&db, "sha256:aaa").unwrap();
        let r1 = get_tracking(&db, "sha256:aaa").unwrap().unwrap();
        assert_eq!(r1.first_seen_at, r1.last_used_at);
        // Backdate to exercise the update path.
        db.conn
            .execute(
                "UPDATE image_tracking SET last_used_at = ?1 WHERE image_id = ?2",
                params![r1.last_used_at - 1000, "sha256:aaa"],
            )
            .unwrap();
        mark_used(&db, "sha256:aaa").unwrap();
        let r2 = get_tracking(&db, "sha256:aaa").unwrap().unwrap();
        assert!(r2.last_used_at > r1.last_used_at - 1000);
    }

    // r[verify image.pin.expiry]
    #[test]
    fn drop_expired_pins_removes_past_due() {
        let db = Db::open_in_memory().unwrap();
        upsert_pin(&db, &app("foo"), "ghcr.io/x:1").unwrap();
        upsert_pin(&db, &app("foo"), "ghcr.io/x:2").unwrap();

        // Backdate one pin's expiry to 10 minutes ago; leave the other with no expiry.
        let past = now_ms() - 10 * 60 * 1000;
        db.conn
            .execute(
                "UPDATE image_pins SET expires_at = ?1
                 WHERE app = ?2 AND reference = ?3",
                params![past, app("foo"), "ghcr.io/x:1"],
            )
            .unwrap();

        let removed = drop_expired_pins(&db).unwrap();
        assert_eq!(removed, 1);

        let remaining = list_pins(&db, Some(&app("foo"))).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].reference, "ghcr.io/x:2");
    }

    // r[verify image.pin.update-reconcile]
    #[test]
    fn reconcile_strict_deletes_orphan_pins() {
        let db = Db::open_in_memory().unwrap();
        upsert_pin(&db, &app("foo"), "ghcr.io/keep:1").unwrap();
        upsert_pin(&db, &app("foo"), "ghcr.io/drop:1").unwrap();

        let safe = vec!["ghcr.io/keep:1"];
        let deleted = reconcile_pins_after_update(
            &db,
            &app("foo"),
            &safe,
            /* probe_clean */ true,
            30 * 24 * 60 * 60 * 1000,
        )
        .unwrap();
        assert_eq!(deleted, 1);
        let remaining = list_pins(&db, Some(&app("foo"))).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].reference, "ghcr.io/keep:1");
        assert!(remaining[0].expires_at.is_none());
    }

    // r[verify image.pin.update-reconcile]
    #[test]
    fn reconcile_lenient_stamps_expiry_on_orphans() {
        let db = Db::open_in_memory().unwrap();
        upsert_pin(&db, &app("foo"), "ghcr.io/keep:1").unwrap();
        upsert_pin(&db, &app("foo"), "ghcr.io/maybe-orphan:1").unwrap();

        let safe = vec!["ghcr.io/keep:1"];
        let expiry_window = 30 * 24 * 60 * 60 * 1000_i64;
        let deleted = reconcile_pins_after_update(
            &db,
            &app("foo"),
            &safe,
            /* probe_clean */ false,
            expiry_window,
        )
        .unwrap();
        assert_eq!(deleted, 0);

        let pins = list_pins(&db, Some(&app("foo"))).unwrap();
        assert_eq!(pins.len(), 2);
        let keep = pins
            .iter()
            .find(|p| p.reference == "ghcr.io/keep:1")
            .unwrap();
        assert!(keep.expires_at.is_none());
        let orphan = pins
            .iter()
            .find(|p| p.reference == "ghcr.io/maybe-orphan:1")
            .unwrap();
        let exp = orphan.expires_at.expect("expiry should be set");
        let now = now_ms();
        assert!(exp > now && exp - now <= expiry_window + 1000);
    }

    // r[verify image.pin.update-reconcile]
    #[test]
    fn reconcile_clears_expiry_when_reference_becomes_safe_again() {
        let db = Db::open_in_memory().unwrap();
        upsert_pin(&db, &app("foo"), "ghcr.io/x:1").unwrap();

        // First pass: dirty probe, stamps expiry.
        let _ = reconcile_pins_after_update(&db, &app("foo"), &[], false, 30 * 24 * 60 * 60 * 1000)
            .unwrap();
        let pins = list_pins(&db, Some(&app("foo"))).unwrap();
        assert!(pins[0].expires_at.is_some());

        // Second pass: clean probe puts the reference back in the safe set.
        let _ = reconcile_pins_after_update(
            &db,
            &app("foo"),
            &["ghcr.io/x:1"],
            true,
            30 * 24 * 60 * 60 * 1000,
        )
        .unwrap();
        let pins = list_pins(&db, Some(&app("foo"))).unwrap();
        assert_eq!(pins.len(), 1);
        assert!(pins[0].expires_at.is_none());
    }

    // r[verify image.gc]
    #[test]
    fn gc_candidates_respects_cutoff() {
        let db = Db::open_in_memory().unwrap();
        note_present(&db, "sha256:old").unwrap();
        note_present(&db, "sha256:new").unwrap();

        // Backdate "old" to 45 days ago.
        let old_ts = now_ms() - 45 * 24 * 60 * 60 * 1000;
        db.conn
            .execute(
                "UPDATE image_tracking SET last_used_at = ?1 WHERE image_id = ?2",
                params![old_ts, "sha256:old"],
            )
            .unwrap();

        let thirty_days_ms: i64 = 30 * 24 * 60 * 60 * 1000;
        let cands = gc_candidates(&db, thirty_days_ms).unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].image_id, "sha256:old");
    }
}
