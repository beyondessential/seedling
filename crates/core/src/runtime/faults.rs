use std::sync::OnceLock;

use jiff::Timestamp;
use rusqlite::OptionalExtension as _;
use seedling_protocol::events::EventSender;
use seedling_protocol::names::AppName;
use serde::Serialize;
use tracing::warn;

static EVENT_TX: OnceLock<EventSender> = OnceLock::new();

/// Install the broadcast sender used by fault operations.
/// Call once at startup before any faults are filed.
pub fn init(tx: EventSender) {
    EVENT_TX
        .set(tx)
        .expect("faults::init must be called exactly once");
}

// r[impl fault.surfacing]
fn emit_filed(record: &FaultRecord) {
    if let Some(tx) = EVENT_TX.get() {
        tx.fault_filed(
            &record.id,
            &record.app,
            record.resource_type.as_deref(),
            record.resource_name.as_deref(),
            record.instance_id.as_deref(),
            &record.kind,
            &record.description,
        );
    }
}

fn emit_cleared(id: &str, app: &AppName, kind: &str) {
    if let Some(tx) = EVENT_TX.get() {
        tx.fault_cleared(id, app, kind);
    }
}

// r[impl fault.definition]
#[derive(Debug, Clone, Serialize)]
pub struct FaultRecord {
    pub id: String,
    pub app: AppName,
    pub resource_type: Option<String>,
    pub resource_name: Option<String>,
    pub instance_id: Option<String>,
    pub kind: String,
    /// The faulty thing; see [`FaultKey`]. Empty for app-wide faults.
    pub subject: String,
    pub timestamp: Timestamp,
    pub description: String,
}

/// A fault's identity: the thing that is faulty, and in what way.
///
/// Before this existed, every site invented its own notion of sameness —
/// `(kind, instance_id)` here, `(kind, resource_name)` there, `(kind,
/// description)` elsewhere, bare `kind` in one place, and nothing at all in
/// another. Clears were then either too broad (a successful backup of one
/// volume cleared every volume's `backup_failed`) or too fragile (matching on
/// a `host:port` substring of the description).
///
/// `resource_type`/`resource_name`/`instance_id` remain display metadata.
/// Matching and clearing use this key alone.
// r[impl fault.lifecycle]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FaultKey {
    pub app: AppName,
    pub kind: String,
    /// The faulty thing: a volume id, an image ref, a `host:port`, an
    /// instance hex. Empty when the fault is about the app as a whole.
    pub subject: String,
}

impl FaultKey {
    pub fn new(app: &AppName, kind: impl Into<String>, subject: impl Into<String>) -> Self {
        Self {
            app: app.clone(),
            kind: kind.into(),
            subject: subject.into(),
        }
    }

    /// A fault about the app itself rather than any one thing within it.
    pub fn app_wide(app: &AppName, kind: impl Into<String>) -> Self {
        Self::new(app, kind, "")
    }
}

/// The display metadata that rides along with a fault but takes no part in
/// its identity.
#[derive(Debug, Clone, Default)]
pub struct FaultMeta {
    pub resource_type: Option<String>,
    pub resource_name: Option<String>,
    pub instance_id: Option<String>,
}

impl FaultMeta {
    pub fn instance(resource_type: &str, resource_name: &str, instance_id: &str) -> Self {
        Self {
            resource_type: Some(resource_type.to_owned()),
            resource_name: Some(resource_name.to_owned()),
            instance_id: Some(instance_id.to_owned()),
        }
    }

    pub fn resource(resource_type: &str, resource_name: &str) -> Self {
        Self {
            resource_type: Some(resource_type.to_owned()),
            resource_name: Some(resource_name.to_owned()),
            instance_id: None,
        }
    }
}

// i[fault.record]
pub fn file_fault(
    db: &crate::runtime::db::Db,
    app: &AppName,
    resource_type: Option<&str>,
    resource_name: Option<&str>,
    instance_id: Option<&str>,
    kind: &str,
    description: &str,
) -> rusqlite::Result<String> {
    // Sites that have not yet been given an explicit subject derive one the
    // same way migration v53 backfilled the existing rows, so a fault filed
    // before the migration matches the key its site computes after it.
    let subject = instance_id.or(resource_name).unwrap_or("");
    file_keyed(
        db,
        &FaultKey::new(app, kind, subject),
        &FaultMeta {
            resource_type: resource_type.map(str::to_owned),
            resource_name: resource_name.map(str::to_owned),
            instance_id: instance_id.map(str::to_owned),
        },
        description,
    )
    .map(|(id, _)| id)
}

/// File `key` unless an active fault already holds it.
///
/// Returns the fault's id and whether it was newly filed. This is the dedup
/// that four sites hand-rolled as an `already_filed` scan of
/// `list_active_faults`, and that `audit_lag` omitted entirely — so it
/// duplicated without bound.
// r[impl fault.lifecycle]
pub fn file_once(
    db: &crate::runtime::db::Db,
    key: &FaultKey,
    meta: &FaultMeta,
    description: &str,
) -> rusqlite::Result<bool> {
    file_keyed(db, key, meta, description).map(|(_, filed)| filed)
}

/// Insert the fault, or return the id of the active one already holding the
/// key. The uniqueness is enforced by a partial unique index rather than by a
/// read-then-write, so concurrent filers cannot both win.
fn file_keyed(
    db: &crate::runtime::db::Db,
    key: &FaultKey,
    meta: &FaultMeta,
    description: &str,
) -> rusqlite::Result<(String, bool)> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Timestamp::now();
    let timestamp = now.to_string();
    let app = &key.app;
    let kind = key.kind.as_str();
    let resource_type = meta.resource_type.as_deref();
    let resource_name = meta.resource_name.as_deref();
    let instance_id = meta.instance_id.as_deref();
    let inserted = db.conn.execute(
        "INSERT INTO faults (id, app, resource_type, resource_name, instance_id, kind, timestamp, description, subject)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT DO NOTHING",
        rusqlite::params![id, app, resource_type, resource_name, instance_id, kind, timestamp, description, key.subject],
    )?;
    if inserted == 0 {
        // The row that won the conflict can be cleared before this reads it,
        // in which case the key is free again and the caller's fault should
        // be filed rather than reported as a duplicate.
        let existing: Option<String> = db
            .conn
            .query_row(
                "SELECT id FROM faults
                 WHERE app = ?1 AND kind = ?2 AND subject = ?3 AND cleared_at IS NULL",
                rusqlite::params![app, kind, key.subject],
                |row| row.get(0),
            )
            .optional()?;
        match existing {
            Some(id) => return Ok((id, false)),
            None => return file_keyed(db, key, meta, description),
        }
    }
    warn!(
        app = %app,
        kind, resource_type, resource_name, instance_id, "fault filed: {description}",
    );
    let record = FaultRecord {
        id: id.clone(),
        app: app.clone(),
        resource_type: resource_type.map(str::to_owned),
        resource_name: resource_name.map(str::to_owned),
        instance_id: instance_id.map(str::to_owned),
        kind: kind.to_owned(),
        subject: key.subject.clone(),
        timestamp: now,
        description: description.to_owned(),
    };
    emit_filed(&record);
    Ok((id, true))
}

/// Clear a single fault by ID. The `app` is needed for the event broadcast;
/// pass it from the context that looked up the fault record. The fault's
/// kind is read in the same statement and included in the emitted
/// `FaultCleared` event so clients can render a meaningful summary without
/// having to remember every fault ID they saw.
// i[impl fault.derived]
pub fn clear_fault(
    db: &crate::runtime::db::Db,
    fault_id: &str,
    app: &AppName,
) -> rusqlite::Result<()> {
    let now = Timestamp::now();
    // Read the kind before the UPDATE — after clearing, the row is no longer
    // "active" but still exists, so this also works post-clear, but reading
    // up-front keeps the happy path to a single pair of statements.
    let kind: Option<String> = db
        .conn
        .query_row("SELECT kind FROM faults WHERE id = ?1", [fault_id], |row| {
            row.get(0)
        })
        .ok();
    let changed = db.conn.execute(
        "UPDATE faults SET cleared_at = ?1 WHERE id = ?2 AND cleared_at IS NULL",
        rusqlite::params![now.to_string(), fault_id],
    )?;
    if changed > 0 {
        emit_cleared(fault_id, app, kind.as_deref().unwrap_or(""));
    }
    Ok(())
}

// i[fault.list]
pub fn list_active_faults(
    db: &crate::runtime::db::Db,
    app: Option<&AppName>,
) -> rusqlite::Result<Vec<FaultRecord>> {
    let mut records = Vec::new();
    match app {
        Some(app_name) => {
            let mut stmt = db.conn.prepare(
                "SELECT id, app, resource_type, resource_name, instance_id, kind, timestamp, description, subject
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
                "SELECT id, app, resource_type, resource_name, instance_id, kind, timestamp, description, subject
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
    let timestamp = ts_str
        .parse::<Timestamp>()
        .unwrap_or_else(|_| Timestamp::now());
    Ok(FaultRecord {
        id: row.get(0)?,
        app: row.get(1)?,
        resource_type: row.get(2)?,
        resource_name: row.get(3)?,
        instance_id: row.get(4)?,
        kind: row.get(5)?,
        subject: row.get(8)?,
        timestamp,
        description: row.get(7)?,
    })
}

/// Clear all active faults matching an app + kind. Returns how many were cleared.
// i[impl fault.derived]
pub fn clear_faults_by_kind(
    db: &crate::runtime::db::Db,
    app: &AppName,
    kind: &str,
) -> rusqlite::Result<u64> {
    let to_clear: Vec<_> = list_active_faults(db, Some(app))?
        .into_iter()
        .filter(|f| f.kind == kind)
        .collect();
    let count = to_clear.len() as u64;
    for f in &to_clear {
        clear_fault(db, &f.id, app)?;
    }
    Ok(count)
}

/// Clear all active faults for an app (used during deregistration).
pub fn clear_all_faults_for_app(
    db: &crate::runtime::db::Db,
    app: &AppName,
) -> rusqlite::Result<()> {
    let to_clear = list_active_faults(db, Some(app))?;
    for f in &to_clear {
        clear_fault(db, &f.id, app)?;
    }
    Ok(())
}

/// Clear every active fault tied to a specific instance_id. Called when the
/// instance is being torn down — either because the operation that created
/// an ephemeral dynamic resource has finished, or because the resource is
/// being retired. Without this, per-instance faults (image_pull_failed,
/// container_start_failed, …) filed against short-lived Job instances
/// linger forever because nothing ever references the dead instance id again.
// i[impl fault.derived]
pub fn clear_faults_for_instance(
    db: &crate::runtime::db::Db,
    app: &AppName,
    instance_id: &str,
) -> rusqlite::Result<()> {
    let to_clear: Vec<_> = list_active_faults(db, Some(app))?
        .into_iter()
        .filter(|f| f.instance_id.as_deref() == Some(instance_id))
        .collect();
    for f in &to_clear {
        clear_fault(db, &f.id, app)?;
    }
    Ok(())
}

pub fn count_active_faults_for_app(
    db: &crate::runtime::db::Db,
    app: &AppName,
) -> rusqlite::Result<i64> {
    db.conn.query_row(
        "SELECT COUNT(*) FROM faults WHERE app = ?1 AND cleared_at IS NULL",
        [app],
        |r| r.get(0),
    )
}

pub fn has_active_faults(db: &crate::runtime::db::Db, app: &AppName) -> rusqlite::Result<bool> {
    Ok(count_active_faults_for_app(db, app)? > 0)
}

pub fn count_active_faults(db: &crate::runtime::db::Db) -> rusqlite::Result<i64> {
    db.conn.query_row(
        "SELECT COUNT(*) FROM faults WHERE cleared_at IS NULL",
        [],
        |r| r.get(0),
    )
}

/// Which active faults a [`sync_faults`] call owns, and may therefore clear.
///
/// Scoping is what keeps a converge call from clearing kinds it knows nothing
/// about: a sweep that computes every currently-conflicting `(host, port)`
/// must not treat the absence of a `backup_failed` key as a reason to clear
/// one.
// r[impl fault.lifecycle]
#[derive(Debug, Clone)]
pub enum FaultScope {
    /// Every active fault of this kind, across all apps. For conditions
    /// computed globally each tick, such as ingress conflicts.
    Kind(String),
    /// Every active fault of this kind belonging to one app.
    AppKind(AppName, String),
}

impl FaultScope {
    fn owns(&self, record: &FaultRecord) -> bool {
        match self {
            Self::Kind(kind) => record.kind == *kind,
            Self::AppKind(app, kind) => record.app == *app && record.kind == *kind,
        }
    }

    fn kind(&self) -> &str {
        match self {
            Self::Kind(kind) | Self::AppKind(_, kind) => kind,
        }
    }
}

/// The active faults a scope owns, filtered in SQL.
///
/// A global sweep such as ingress conflicts runs every tick; reading every
/// active fault in the database and discarding all but one kind would make
/// that cost grow with the total fault count rather than with the kind's.
fn list_active_faults_in_scope(
    db: &crate::runtime::db::Db,
    scope: &FaultScope,
) -> rusqlite::Result<Vec<FaultRecord>> {
    const COLUMNS: &str =
        "id, app, resource_type, resource_name, instance_id, kind, timestamp, description, subject";
    match scope {
        FaultScope::Kind(kind) => {
            let mut stmt = db.conn.prepare(&format!(
                "SELECT {COLUMNS} FROM faults
                 WHERE cleared_at IS NULL AND kind = ?1
                 ORDER BY timestamp"
            ))?;
            stmt.query_map([kind], row_to_record)?.collect()
        }
        FaultScope::AppKind(app, kind) => {
            let mut stmt = db.conn.prepare(&format!(
                "SELECT {COLUMNS} FROM faults
                 WHERE cleared_at IS NULL AND app = ?1 AND kind = ?2
                 ORDER BY timestamp"
            ))?;
            stmt.query_map(rusqlite::params![app, kind], row_to_record)?
                .collect()
        }
    }
}

/// What a [`sync_faults`] call did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub filed: usize,
    pub cleared: usize,
}

/// Converge the active faults within `scope` to exactly `current`.
///
/// Files the keys in `current` that are not active, clears the active ones
/// that are no longer in `current`. `describe` supplies a description for each
/// newly-filed key.
///
/// This replaces the two in-memory prev-set diffs in the reconciler, which
/// cleared only `prior \ current` where `prior` was a `Reconciler` field that
/// starts empty on every daemon start — the faults are in the database, so one
/// filed before a restart could never clear. Comparing against the persisted
/// active set instead is restart-safe by construction: there is no warm-up
/// state to reconstruct and none to forget to persist.
// r[impl fault.lifecycle]
pub fn sync_faults(
    db: &crate::runtime::db::Db,
    scope: &FaultScope,
    current: &std::collections::BTreeMap<FaultKey, (FaultMeta, String)>,
) -> rusqlite::Result<SyncOutcome> {
    let active = list_active_faults_in_scope(db, scope)?;

    let mut outcome = SyncOutcome::default();

    for record in &active {
        let key = FaultKey::new(&record.app, record.kind.clone(), record.subject.clone());
        if !current.contains_key(&key) {
            clear_fault(db, &record.id, &record.app)?;
            outcome.cleared += 1;
        }
    }

    for (key, (meta, description)) in current {
        debug_assert!(
            scope.owns(&FaultRecord {
                id: String::new(),
                app: key.app.clone(),
                resource_type: None,
                resource_name: None,
                instance_id: None,
                kind: key.kind.clone(),
                subject: key.subject.clone(),
                timestamp: Timestamp::now(),
                description: String::new(),
            }),
            "sync_faults given a key outside its own scope: {key:?}"
        );
        if file_once(db, key, meta, description)? {
            outcome.filed += 1;
        }
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests;
