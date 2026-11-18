//! The durable state the runtime keeps for gates: each gate's record, the
//! resources its run holds stopped, and whether the app is set up.

use std::collections::{BTreeMap, BTreeSet};

use jiff::Timestamp;
use rusqlite::{OptionalExtension, params};
use seedling_protocol::names::{AppName, GateName};

use crate::{
    defs::resource::ResourceId,
    runtime::{
        db::Db,
        history::{parse_resource_kind, resource_kind_str},
        secrets::KeyDigest,
    },
};

/// What the runtime holds of one gate.
// spec: GRUN#gate-record
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GateRecord {
    /// The digest of the key of the gate's last success.
    pub key_digest: Option<KeyDigest>,
    /// The generation of the gate's last success, kept when the success is
    /// invalidated, so that `last` can still be rebuilt from it.
    pub success_generation: Option<u64>,
    /// Whether the last success still stands.
    // spec: GRUN#terminology
    pub valid: bool,
    /// The runs that have failed since the last success, or since the last
    /// run that succeeded.
    pub consecutive_failures: u32,
    /// When the most recent run that did not succeed ended; kept while a
    /// later run is in progress, as the delay before the next is counted from
    /// it.
    pub last_failure_at: Option<Timestamp>,
    /// How the most recent run ended, when it did not succeed. Absent once
    /// the next run starts, and after a success.
    pub last_failure: Option<String>,
    /// The digest of the key the most recent failed run was for.
    pub failed_key_digest: Option<KeyDigest>,
    /// The digest of the key a cancelled run paused the gate for.
    // spec: GRUN#pauses
    pub paused_key_digest: Option<KeyDigest>,
    /// How many checks in a row could not be run.
    pub check_failures: u32,
    /// When the most recent check that could not be run was tried.
    pub check_unavailable_at: Option<Timestamp>,
    /// Why the most recent check could not be run.
    pub check_unavailable: Option<String>,
}

/// An app's gate records: each gate's, or why it cannot be read.
pub type Records = BTreeMap<GateName, Result<GateRecord, String>>;

/// The resources each gate holds stopped.
pub type Holds = BTreeMap<GateName, BTreeSet<ResourceId>>;

fn digest(column: &str, bytes: Option<Vec<u8>>) -> Result<Option<KeyDigest>, String> {
    match bytes {
        None => Ok(None),
        Some(bytes) => KeyDigest::from_bytes(&bytes)
            .map(Some)
            .ok_or_else(|| format!("the {column} is damaged")),
    }
}

fn timestamp(column: &str, text: Option<String>) -> Result<Option<Timestamp>, String> {
    match text {
        None => Ok(None),
        Some(text) => text
            .parse()
            .map(Some)
            .map_err(|e| format!("the {column} {text:?} is not a time: {e}")),
    }
}

type RawRow = (
    String,
    Option<Vec<u8>>,
    Option<i64>,
    i64,
    i64,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    i64,
    Option<String>,
    Option<String>,
);

fn parse_row(raw: RawRow) -> Result<GateRecord, String> {
    let (
        _,
        key_digest,
        success_generation,
        valid,
        consecutive_failures,
        last_failure_at,
        last_failure,
        failed_key_digest,
        paused_key_digest,
        check_failures,
        check_unavailable_at,
        check_unavailable,
    ) = raw;
    Ok(GateRecord {
        key_digest: digest("key digest", key_digest)?,
        success_generation: success_generation
            .map(|g| u64::try_from(g).map_err(|_| "the success generation is negative".to_owned()))
            .transpose()?,
        valid: valid != 0,
        consecutive_failures: u32::try_from(consecutive_failures)
            .map_err(|_| "the failure count is out of range".to_owned())?,
        last_failure_at: timestamp("last failure time", last_failure_at)?,
        last_failure,
        failed_key_digest: digest("failed key digest", failed_key_digest)?,
        paused_key_digest: digest("paused key digest", paused_key_digest)?,
        check_failures: u32::try_from(check_failures)
            .map_err(|_| "the check failure count is out of range".to_owned())?,
        check_unavailable_at: timestamp("check failure time", check_unavailable_at)?,
        check_unavailable,
    })
}

/// Every gate record of `app`.
///
/// A row that cannot be understood is that gate's alone to be unreadable,
/// and a query that fails is every gate's: neither reads as no record.
// spec: GRUN#gate-state
pub fn load_records(db: &Db, app: &AppName) -> rusqlite::Result<Records> {
    let mut stmt = db.conn.prepare(
        "SELECT gate, key_digest, success_generation, valid, consecutive_failures,
                last_failure_at, last_failure, failed_key_digest, paused_key_digest,
                check_failures, check_unavailable_at, check_unavailable
         FROM gate_records WHERE app = ?1",
    )?;
    let rows = stmt.query_map(params![app], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
            row.get(11)?,
        ))
    })?;
    let mut records = Records::new();
    for row in rows {
        let raw = row?;
        let Ok(gate) = GateName::new(&raw.0) else {
            tracing::warn!(%app, gate = %raw.0, "a gate record names no valid gate; ignoring it");
            continue;
        };
        records.insert(gate, parse_row(raw));
    }
    Ok(records)
}

/// Start a run: the gate's last success no longer stands, and the way the
/// previous run ended is no longer the way its most recent one did.
// spec: GRUN#preparation
pub fn begin_run(db: &Db, app: &AppName, gate: &GateName) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO gate_records (app, gate, valid, last_failure)
         VALUES (?1, ?2, 0, NULL)
         ON CONFLICT(app, gate) DO UPDATE SET valid = 0, last_failure = NULL",
        params![app, gate],
    )?;
    Ok(())
}

/// The gate's last success no longer stands.
// spec: GRUN#checks
pub fn invalidate(db: &Db, app: &AppName, gate: &GateName) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO gate_records (app, gate, valid)
         VALUES (?1, ?2, 0)
         ON CONFLICT(app, gate) DO UPDATE SET valid = 0",
        params![app, gate],
    )?;
    Ok(())
}

/// A run succeeded for `key` at `generation`.
// spec: GRUN#outcome
pub fn record_success(
    db: &Db,
    app: &AppName,
    gate: &GateName,
    key: &KeyDigest,
    generation: u64,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO gate_records (app, gate, key_digest, success_generation, valid)
         VALUES (?1, ?2, ?3, ?4, 1)
         ON CONFLICT(app, gate) DO UPDATE SET
             key_digest = excluded.key_digest,
             success_generation = excluded.success_generation,
             valid = 1,
             consecutive_failures = 0,
             last_failure_at = NULL,
             last_failure = NULL,
             failed_key_digest = NULL,
             paused_key_digest = NULL,
             check_failures = 0,
             check_unavailable_at = NULL,
             check_unavailable = NULL",
        params![app, gate, key.as_bytes(), generation as i64],
    )?;
    Ok(())
}

/// A run for `key` did not succeed, and ended at `at` in the way `how` says.
/// `pause` is the key a cancelled run pauses the gate for.
// spec: GRUN#retries
// spec: GRUN#pauses
pub fn record_failure(
    db: &Db,
    app: &AppName,
    gate: &GateName,
    key: &KeyDigest,
    at: Timestamp,
    how: &str,
    pause: Option<&KeyDigest>,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO gate_records
             (app, gate, valid, consecutive_failures, last_failure_at, last_failure,
              failed_key_digest, paused_key_digest)
         VALUES (?1, ?2, 0, 1, ?3, ?4, ?5, ?6)
         ON CONFLICT(app, gate) DO UPDATE SET
             valid = 0,
             consecutive_failures = consecutive_failures + 1,
             last_failure_at = excluded.last_failure_at,
             last_failure = excluded.last_failure,
             failed_key_digest = excluded.failed_key_digest,
             paused_key_digest = COALESCE(excluded.paused_key_digest, paused_key_digest)",
        params![
            app,
            gate,
            at.to_string(),
            how,
            key.as_bytes(),
            pause.map(KeyDigest::as_bytes)
        ],
    )?;
    Ok(())
}

/// An operator ends the gate's pause.
// spec: GRUN#pauses
pub fn clear_pause(db: &Db, app: &AppName, gate: &GateName) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE gate_records SET paused_key_digest = NULL WHERE app = ?1 AND gate = ?2",
        params![app, gate],
    )?;
    Ok(())
}

/// The gate's check could not be run, at `at`, for the reason `why`.
// spec: GRUN#checks
pub fn record_check_unavailable(
    db: &Db,
    app: &AppName,
    gate: &GateName,
    at: Timestamp,
    why: &str,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO gate_records
             (app, gate, valid, check_failures, check_unavailable_at, check_unavailable)
         VALUES (?1, ?2, 0, 1, ?3, ?4)
         ON CONFLICT(app, gate) DO UPDATE SET
             check_failures = check_failures + 1,
             check_unavailable_at = excluded.check_unavailable_at,
             check_unavailable = excluded.check_unavailable",
        params![app, gate, at.to_string(), why],
    )?;
    Ok(())
}

/// The gate's check ran to an outcome.
// spec: GRUN#checks
pub fn clear_check_unavailable(db: &Db, app: &AppName, gate: &GateName) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE gate_records
         SET check_failures = 0, check_unavailable_at = NULL, check_unavailable = NULL
         WHERE app = ?1 AND gate = ?2",
        params![app, gate],
    )?;
    Ok(())
}

/// What each gate of `app` holds stopped.
// spec: RCN#gate-holds
pub fn load_holds(db: &Db, app: &AppName) -> rusqlite::Result<Holds> {
    let mut stmt = db
        .conn
        .prepare("SELECT gate, kind, name FROM gate_holds WHERE app = ?1")?;
    let rows = stmt.query_map(params![app], |row| {
        Ok((
            row.get::<_, GateName>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut holds = Holds::new();
    for row in rows {
        let (gate, kind, name) = row?;
        holds.entry(gate).or_default().insert(ResourceId {
            kind: parse_resource_kind(&kind)?,
            name: std::sync::Arc::new(name),
        });
    }
    Ok(holds)
}

/// Record that `gate` holds `resources`, replacing what it held.
// spec: RCN#gate-holds
pub fn set_hold(
    db: &Db,
    app: &AppName,
    gate: &GateName,
    resources: &BTreeSet<ResourceId>,
) -> rusqlite::Result<()> {
    db.atomically(|db| {
        db.conn.execute(
            "DELETE FROM gate_holds WHERE app = ?1 AND gate = ?2",
            params![app, gate],
        )?;
        for resource in resources {
            db.conn.execute(
                "INSERT INTO gate_holds (app, gate, kind, name) VALUES (?1, ?2, ?3, ?4)",
                params![app, gate, resource_kind_str(resource.kind), *resource.name],
            )?;
        }
        Ok(())
    })
}

/// `gate` no longer holds anything.
// spec: RCN#gate-holds
pub fn clear_hold(db: &Db, app: &AppName, gate: &GateName) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM gate_holds WHERE app = ?1 AND gate = ?2",
        params![app, gate],
    )?;
    Ok(())
}

/// Whether `app` has been set up.
// spec: GRUN#app-setup
pub fn is_set_up(db: &Db, app: &AppName) -> rusqlite::Result<bool> {
    db.conn
        .query_row(
            "SELECT set_up FROM registered_apps WHERE name = ?1",
            params![app],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(|set_up| set_up != 0)
        .ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// Record that `app` is set up.
// spec: GRUN#app-setup
pub fn mark_set_up(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE registered_apps SET set_up = 1 WHERE name = ?1",
        params![app],
    )?;
    Ok(())
}

/// Delete everything held of `app`'s gates: their records, their holds and
/// the record that the app is set up.
// spec: GRUN#uninstall
pub fn delete_app_state(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.atomically(|db| {
        db.conn
            .execute("DELETE FROM gate_records WHERE app = ?1", params![app])?;
        db.conn
            .execute("DELETE FROM gate_holds WHERE app = ?1", params![app])?;
        db.conn.execute(
            "UPDATE registered_apps SET set_up = 0 WHERE name = ?1",
            params![app],
        )?;
        Ok(())
    })
}
