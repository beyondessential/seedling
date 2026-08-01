//! Restart accounting: the durable record of every container restart, and the
//! rate derived from it.
//!
//! Seedling keeps the books even where it does not action the restart. On
//! Linux systemd restarts the unit and seedling reads its counter; on a
//! platform without a supervisor the runtime restarts the workload itself and
//! records the attempt firsthand. Either way the recorded rate — not the
//! supervisor's internal accounting — is what decides a crash loop.

use jiff::Timestamp;
use rusqlite::OptionalExtension;
use seedling_protocol::names::AppName;
use serde::Serialize;

use crate::runtime::db::Db;

/// Most-recent restart records kept per instance.
///
/// The bound is per instance rather than global, and applied on write rather
/// than by rate-limiting what gets recorded: a hard crash loop produces rows
/// fastest exactly when the per-attempt exit codes are the diagnostic.
// r[impl gc.restarts]
pub const RETAIN_PER_INSTANCE: usize = 50;

/// Why a restart happened.
///
/// Deliberately not "who performed it". On Linux systemd actions recovery
/// restarts and seedling actions deliberate ones, so the two splits coincide —
/// but only because of how this platform is put together. Where there is no
/// service supervisor the runtime performs both kinds, and a field recording
/// the actor would put every restart in one bucket and leave the crash-loop
/// rate permanently at zero.
// r[impl autonomous.restart.record]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cause {
    /// The workload exited unexpectedly and was brought back. Counts towards
    /// the crash-loop rate.
    Recovery,
    /// The runtime restarted the workload on purpose: a rolling update, a
    /// health-check replacement, an operator-requested restart. Recorded but
    /// excluded from the rate.
    Deliberate,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recovery => "recovery",
            Self::Deliberate => "deliberate",
        }
    }
}

/// How the previous run ended, as far as the platform reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExitKind {
    /// Exited of its own accord; the code is its exit status.
    Exited,
    /// Killed by a signal; the code is the signal number.
    Signalled,
    /// Killed by a signal and dumped core; the code is the signal number.
    Dumped,
}

impl ExitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exited => "exited",
            Self::Signalled => "signalled",
            Self::Dumped => "dumped",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "exited" => Some(Self::Exited),
            "signalled" => Some(Self::Signalled),
            "dumped" => Some(Self::Dumped),
            _ => None,
        }
    }
}

/// The exit status of the run that ended, where the platform reports one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus {
    pub kind: ExitKind,
    pub code: i32,
}

// i[impl restart.record]
#[derive(Debug, Clone, Serialize)]
pub struct RestartRecord {
    pub id: i64,
    pub app: AppName,
    pub instance_id: String,
    pub resource_type: Option<String>,
    pub resource_name: Option<String>,
    pub generation: Option<i64>,
    pub timestamp: Timestamp,
    pub cause: Cause,
    pub exit_code: Option<i32>,
    pub exit_kind: Option<ExitKind>,
}

/// Crash-loop rate parameters.
// r[impl autonomous.restart.rate.settings]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RestartSettings {
    pub threshold: i64,
    pub window_secs: i64,
}

/// Lower bounds on the settings. A threshold of one would file a crash loop on
/// the first restart, which every container that has ever been rescheduled
/// would trip; a window under a minute is shorter than the pacing the
/// supervisor already applies between attempts.
pub const MIN_THRESHOLD: i64 = 2;
pub const MIN_WINDOW_SECS: i64 = 60;

/// What the instance's restart history looks like right now, for the app
/// description surface.
// i[impl app.describe]
#[derive(Debug, Clone, Serialize)]
pub struct RestartSummary {
    /// Recovery restarts inside the current rate window.
    pub recent: i64,
    pub window_secs: i64,
    /// All retained records for the instance, of either cause.
    pub total: i64,
    pub last_at: Option<String>,
    pub last_exit_code: Option<i32>,
    pub last_exit_kind: Option<ExitKind>,
}

fn now_ms() -> i64 {
    Timestamp::now().as_millisecond()
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

/// Identity carried on every record. Kept as one argument so callers do not
/// thread five positional strings through the reconciler.
#[derive(Debug, Clone)]
pub struct RestartSubject {
    pub app: AppName,
    pub instance_id: String,
    pub resource_type: Option<String>,
    pub resource_name: Option<String>,
    pub generation: Option<i64>,
}

// r[impl autonomous.restart.record]
/// Record one restart. `at_ms` lets a caller recording a burst of counter
/// deltas stamp them apart rather than collapsing them onto one instant.
pub fn record(
    db: &Db,
    subject: &RestartSubject,
    cause: Cause,
    exit: Option<ExitStatus>,
    at_ms: i64,
) -> rusqlite::Result<i64> {
    db.conn.execute(
        "INSERT INTO instance_restarts
            (instance_id, app, resource_type, resource_name, generation,
             recorded_at, cause, exit_code, exit_kind)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            subject.instance_id,
            subject.app,
            subject.resource_type,
            subject.resource_name,
            subject.generation,
            at_ms,
            cause.as_str(),
            exit.map(|e| e.code),
            exit.map(|e| e.kind.as_str()),
        ],
    )?;
    let id = db.conn.last_insert_rowid();
    prune_instance(db, &subject.instance_id, RETAIN_PER_INSTANCE)?;
    Ok(id)
}

// r[impl gc.restarts]
/// Drop all but the `retain` most recent records for one instance.
pub fn prune_instance(db: &Db, instance_id: &str, retain: usize) -> rusqlite::Result<usize> {
    db.conn.execute(
        "DELETE FROM instance_restarts
         WHERE instance_id = ?1
           AND id NOT IN (
               SELECT id FROM instance_restarts
               WHERE instance_id = ?1
               ORDER BY recorded_at DESC, id DESC
               LIMIT ?2
           )",
        rusqlite::params![instance_id, retain as i64],
    )
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RestartRecord> {
    let recorded_at: i64 = row.get(6)?;
    let cause: String = row.get(7)?;
    let exit_kind: Option<String> = row.get(9)?;
    Ok(RestartRecord {
        id: row.get(0)?,
        instance_id: row.get(1)?,
        app: row.get(2)?,
        resource_type: row.get(3)?,
        resource_name: row.get(4)?,
        generation: row.get(5)?,
        timestamp: Timestamp::from_millisecond(recorded_at).unwrap_or_default(),
        cause: if cause == "deliberate" {
            Cause::Deliberate
        } else {
            Cause::Recovery
        },
        exit_code: row.get(8)?,
        exit_kind: exit_kind.as_deref().and_then(ExitKind::from_str),
    })
}

const SELECT_COLS: &str = "id, instance_id, app, resource_type, resource_name, generation, \
                           recorded_at, cause, exit_code, exit_kind";

// i[impl restart.list]
/// Restart records, most recent first, optionally narrowed to one app and/or
/// one instance.
pub fn list(
    db: &Db,
    app: Option<&AppName>,
    instance_id: Option<&str>,
    limit: usize,
) -> rusqlite::Result<Vec<RestartRecord>> {
    let sql = format!(
        "SELECT {SELECT_COLS} FROM instance_restarts
         WHERE (?1 IS NULL OR app = ?1)
           AND (?2 IS NULL OR instance_id = ?2)
         ORDER BY recorded_at DESC, id DESC
         LIMIT ?3"
    );
    let mut stmt = db.conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![app, instance_id, limit as i64],
        row_to_record,
    )?;
    rows.collect()
}

// r[impl autonomous.restart.rate]
/// Recovery restarts recorded for an instance within the last `window_secs`.
/// Deliberate restarts are excluded: a rolling update must not read as a crash
/// burst.
pub fn recent_recovery_count(
    db: &Db,
    instance_id: &str,
    window_secs: i64,
) -> rusqlite::Result<i64> {
    let cutoff = now_ms() - window_secs * 1000;
    db.conn.query_row(
        "SELECT COUNT(*) FROM instance_restarts
         WHERE instance_id = ?1 AND cause = 'recovery' AND recorded_at >= ?2",
        rusqlite::params![instance_id, cutoff],
        |r| r.get(0),
    )
}

/// Per-instance summary for the app description surface. Returns `None` when
/// the instance has no records at all, so callers can omit the field entirely
/// rather than reporting a zeroed summary for a resource that never restarts.
pub fn summary(
    db: &Db,
    instance_id: &str,
    settings: RestartSettings,
) -> rusqlite::Result<Option<RestartSummary>> {
    let total: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM instance_restarts WHERE instance_id = ?1",
        rusqlite::params![instance_id],
        |r| r.get(0),
    )?;
    if total == 0 {
        return Ok(None);
    }
    let recent = recent_recovery_count(db, instance_id, settings.window_secs)?;
    let last: Option<(i64, Option<i32>, Option<String>)> = db
        .conn
        .query_row(
            "SELECT recorded_at, exit_code, exit_kind FROM instance_restarts
             WHERE instance_id = ?1
             ORDER BY recorded_at DESC, id DESC
             LIMIT 1",
            rusqlite::params![instance_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (last_at, last_exit_code, last_exit_kind) = match last {
        Some((at, code, kind)) => (
            Timestamp::from_millisecond(at).ok().map(|t| t.to_string()),
            code,
            kind.as_deref().and_then(ExitKind::from_str),
        ),
        None => (None, None, None),
    };
    Ok(Some(RestartSummary {
        recent,
        window_secs: settings.window_secs,
        total,
        last_at,
        last_exit_code,
        last_exit_kind,
    }))
}

// ---------------------------------------------------------------------------
// Counter baselines
// ---------------------------------------------------------------------------

/// The last restart counter seen for an instance's unit, if any.
pub fn baseline(db: &Db, instance_id: &str) -> rusqlite::Result<Option<i64>> {
    db.conn
        .query_row(
            "SELECT counter FROM instance_restart_counters WHERE instance_id = ?1",
            rusqlite::params![instance_id],
            |r| r.get(0),
        )
        .optional()
}

pub fn set_baseline(db: &Db, instance_id: &str, counter: i64) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO instance_restart_counters (instance_id, counter, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (instance_id) DO UPDATE
             SET counter = excluded.counter, updated_at = excluded.updated_at",
        rusqlite::params![instance_id, counter, now_ms()],
    )?;
    Ok(())
}

pub fn clear_baseline(db: &Db, instance_id: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM instance_restart_counters WHERE instance_id = ?1",
        rusqlite::params![instance_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

// r[impl autonomous.restart.rate.settings]
// i[impl restart.settings]
pub fn settings(db: &Db) -> rusqlite::Result<RestartSettings> {
    db.conn.query_row(
        "SELECT threshold, window_secs FROM restart_settings WHERE singleton = 1",
        [],
        |r| {
            Ok(RestartSettings {
                threshold: r.get(0)?,
                window_secs: r.get(1)?,
            })
        },
    )
}

/// Rejected values, so the caller can turn them into an interface error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    ThresholdTooLow,
    WindowTooShort,
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ThresholdTooLow => write!(f, "threshold must be at least {MIN_THRESHOLD}"),
            Self::WindowTooShort => write!(f, "window_secs must be at least {MIN_WINDOW_SECS}"),
        }
    }
}

// r[impl autonomous.restart.rate.settings]
// i[impl restart.settings]
/// Update either or both settings. Omitted fields are left as they are. The
/// reconciler reads the settings on each tick, so a change takes effect on the
/// next one without restarting the runtime.
pub fn set_settings(
    db: &Db,
    threshold: Option<i64>,
    window_secs: Option<i64>,
) -> Result<RestartSettings, SettingsError> {
    if let Some(t) = threshold
        && t < MIN_THRESHOLD
    {
        return Err(SettingsError::ThresholdTooLow);
    }
    if let Some(w) = window_secs
        && w < MIN_WINDOW_SECS
    {
        return Err(SettingsError::WindowTooShort);
    }
    let current = settings(db).unwrap_or(RestartSettings {
        threshold: 5,
        window_secs: 1800,
    });
    let next = RestartSettings {
        threshold: threshold.unwrap_or(current.threshold),
        window_secs: window_secs.unwrap_or(current.window_secs),
    };
    let _ = db.conn.execute(
        "UPDATE restart_settings
            SET threshold = ?1, window_secs = ?2, updated_at = ?3
          WHERE singleton = 1",
        rusqlite::params![next.threshold, next.window_secs, now_ms()],
    );
    Ok(next)
}

#[cfg(test)]
mod tests;
